//! OAuth 2.0 for installed apps: a loopback redirect with PKCE, against a
//! "Desktop app" client the user creates in their own Google Cloud project.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{percent_decode_str, utf8_percent_encode};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

/// The narrowest scopes that cover the tool: reading the calendar list to pick
/// calendar ids, and managing events on calendars the user owns.
pub const SCOPES: [&str; 2] = [
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.events.owned",
];

/// The `installed` section of the client JSON downloaded from the Cloud console.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClientSecrets {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
}

impl ClientSecrets {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read the Google client file {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid Google client file {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct File {
            installed: Option<ClientSecrets>,
        }
        serde_json::from_str::<File>(text)?
            .installed
            .context("not a Desktop app client: create an OAuth client of type \"Desktop app\"")
    }
}

#[derive(Serialize, Deserialize)]
struct StoredToken {
    refresh_token: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

/// Runs the interactive sign-in and stores the refresh token at `token_path`.
///
/// Prints the consent URL, tries to open it in the browser, and waits for
/// Google to redirect back to a one-shot listener on 127.0.0.1.
pub fn sign_in(secrets: &ClientSecrets, token_path: &Path) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("cannot open a loopback port")?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = random_token(32)?;
    let state = random_token(16)?;

    let url = authorization_url(secrets, &redirect_uri, &verifier, &state);
    println!("Open this URL in your browser to grant access:\n\n{url}\n");
    let _ = std::process::Command::new("xdg-open")
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    let code = wait_for_code(&listener, &state)?;
    let token: TokenResponse = token_request(
        secrets,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ],
    )
    .context("exchanging the authorization code failed")?;

    let granted = token.scope.unwrap_or_default();
    let missing: Vec<_> = SCOPES
        .iter()
        .filter(|scope| !granted.split(' ').any(|g| g == **scope))
        .collect();
    ensure!(
        missing.is_empty(),
        "Google did not grant {missing:?}; run auth again and allow every requested permission"
    );
    let refresh_token = token
        .refresh_token
        .context("Google returned no refresh token; run auth again")?;
    save_refresh_token(token_path, &refresh_token)
}

/// Exchanges the stored refresh token for a fresh access token.
pub fn access_token(secrets: &ClientSecrets, token_path: &Path) -> Result<String> {
    let refresh_token = load_refresh_token(token_path)?;
    let token: TokenResponse = token_request(
        secrets,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
        ],
    )
    .context(
        "refreshing the Google access token failed; if the sign-in expired or was revoked, \
         run `bac-calendar auth` again",
    )?;
    Ok(token.access_token)
}

fn token_request(secrets: &ClientSecrets, params: &[(&str, &str)]) -> Result<TokenResponse> {
    let form = params.iter().copied().chain([
        ("client_id", secrets.client_id.as_str()),
        ("client_secret", secrets.client_secret.as_str()),
    ]);
    let response = super::agent().post(&secrets.token_uri).send_form(form)?;
    Ok(super::check(response, "Google token request")?.read_json()?)
}

pub fn authorization_url(
    secrets: &ClientSecrets,
    redirect_uri: &str,
    verifier: &str,
    state: &str,
) -> String {
    let scope = SCOPES.join(" ");
    let challenge = pkce_challenge(verifier);
    let params = [
        ("client_id", secrets.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", &scope),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", utf8_percent_encode(value, super::UNRESERVED)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", secrets.auth_uri)
}

/// The S256 PKCE challenge of RFC 7636.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(
        &ring::digest::SHA256,
        verifier.as_bytes(),
    ))
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0; bytes];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| anyhow::anyhow!("no system randomness"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// Serves loopback requests until Google's redirect carries a code or an error.
fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String> {
    for stream in listener.incoming() {
        let mut stream = stream?;
        let target = read_request_target(&stream)?;
        if !target.starts_with("/?") {
            respond(&mut stream, "404 Not Found", "Not found.")?;
            continue;
        }
        let outcome = parse_redirect(&target, state);
        let page = match &outcome {
            Ok(_) => "Signed in to bac-calendar. You can close this tab.",
            Err(_) => "Sign-in failed. Check the terminal for details.",
        };
        respond(&mut stream, "200 OK", page)?;
        return outcome;
    }
    bail!("the loopback listener closed before Google redirected back")
}

fn read_request_target(stream: &TcpStream) -> Result<String> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut header = String::new();
    while reader.read_line(&mut header)? > 2 {
        header.clear();
    }
    Ok(request_line
        .split(' ')
        .nth(1)
        .unwrap_or_default()
        .to_owned())
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) -> Result<()> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>bac-calendar</title><p>{message}</p>"
    );
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

/// Extracts the authorization code from the redirect target `/?code=...&state=...`.
pub fn parse_redirect(target: &str, expected_state: &str) -> Result<String> {
    let query = target.split_once('?').map_or("", |(_, query)| query);
    let param = |name: &str| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| {
                percent_decode_str(&value.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            })
        })
    };
    if let Some(error) = param("error") {
        bail!("Google sign-in was not completed: {error}");
    }
    ensure!(
        param("state").as_deref() == Some(expected_state),
        "the redirect's state does not match this sign-in; run auth again"
    );
    param("code")
        .filter(|code| !code.is_empty())
        .context("the redirect carries no code")
}

/// Writes the refresh token atomically, readable only by the user.
pub fn save_refresh_token(path: &Path, refresh_token: &str) -> Result<()> {
    let dir = path.parent().context("the token path has no directory")?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;

    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    let _ = fs::remove_file(&temp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .with_context(|| format!("cannot create {}", temp.display()))?;
    serde_json::to_writer(
        &mut file,
        &StoredToken {
            refresh_token: refresh_token.to_owned(),
        },
    )?;
    file.sync_all()?;
    fs::rename(&temp, path).with_context(|| format!("cannot write {}", path.display()))
}

pub fn load_refresh_token(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path).with_context(|| {
        format!(
            "no Google sign-in at {}; run `bac-calendar auth` first",
            path.display()
        )
    })?;
    let token: StoredToken = serde_json::from_str(&text)
        .with_context(|| format!("unreadable token file {}", path.display()))?;
    Ok(token.refresh_token)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const CLIENT_JSON: &str = r#"{"installed":{
        "client_id":"123.apps.googleusercontent.com",
        "project_id":"bac",
        "auth_uri":"https://accounts.google.com/o/oauth2/auth",
        "token_uri":"https://oauth2.googleapis.com/token",
        "client_secret":"shh",
        "redirect_uris":["http://localhost"]
    }}"#;

    #[test]
    fn reads_a_desktop_client_file() {
        let secrets = ClientSecrets::parse(CLIENT_JSON).unwrap();
        assert_eq!(secrets.client_id, "123.apps.googleusercontent.com");
        assert_eq!(secrets.token_uri, "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn rejects_a_web_client_file() {
        let err = ClientSecrets::parse(r#"{"web":{"client_id":"x"}}"#).unwrap_err();
        assert!(err.to_string().contains("Desktop app"), "{err}");
    }

    #[test]
    fn computes_the_rfc_7636_challenge() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_authorization_url_asks_for_offline_access_with_pkce() {
        let secrets = ClientSecrets::parse(CLIENT_JSON).unwrap();
        let url = authorization_url(&secrets, "http://127.0.0.1:4242", "verifier", "xyz");

        assert!(url.starts_with(
            "https://accounts.google.com/o/oauth2/auth?client_id=123.apps.googleusercontent.com&"
        ));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4242&"));
        assert!(url.contains(
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.calendarlist.readonly%20\
             https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.events.owned&"
        ));
        assert!(url.contains(&format!("code_challenge={}", pkce_challenge("verifier"))));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("state=xyz"));
    }

    #[test]
    fn takes_the_code_from_the_redirect() {
        let code = parse_redirect("/?state=xyz&code=4%2F0Ab&scope=a+b", "xyz").unwrap();
        assert_eq!(code, "4/0Ab");
    }

    #[test]
    fn rejects_a_redirect_with_another_state() {
        assert!(parse_redirect("/?state=evil&code=abc", "xyz").is_err());
    }

    #[test]
    fn reports_a_denied_consent() {
        let err = parse_redirect("/?error=access_denied&state=xyz", "xyz").unwrap_err();
        assert!(err.to_string().contains("access_denied"), "{err}");
    }

    #[test]
    fn stores_the_refresh_token_readable_only_by_the_user() {
        let dir = std::env::temp_dir().join(format!("bac-calendar-token-{}", std::process::id()));
        let path = dir.join("nested").join("google-token.json");

        save_refresh_token(&path, "first").unwrap();
        save_refresh_token(&path, "second").unwrap();

        assert_eq!(load_refresh_token(&path).unwrap(), "second");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        fs::remove_dir_all(dir).unwrap();
    }
}
