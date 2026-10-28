use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

const APP_DIR: &str = "bac-calendar";

pub const DEFAULT_EVENT_DURATION_MINUTES: u32 = 120;

/// The user's configuration, read from `config.toml` in the XDG config directory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The OAuth client JSON downloaded from the Google Cloud console.
    pub google_client_file: PathBuf,
    /// A file holding the FCBQ access key, used when `FCBQ_API_KEY` is unset.
    pub fcbq_api_key_file: Option<PathBuf>,
    #[serde(default = "default_event_duration")]
    pub event_duration_minutes: u32,
    #[serde(default)]
    pub teams: Vec<Team>,
}

/// A team whose matches are synced into its own Google calendar.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Team {
    /// How the team appears in event titles.
    pub label: String,
    /// This season's FCBQ team id, from `bac-calendar teams <club-id>`.
    pub fcbq_team_id: u64,
    /// The Google calendar id, from `bac-calendar calendars`.
    pub calendar_id: String,
}

fn default_event_duration() -> u32 {
    DEFAULT_EVENT_DURATION_MINUTES
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "cannot read the config file {}; see the README to create it",
                path.display()
            )
        })?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        Self::parse(&text, base_dir).with_context(|| format!("invalid config {}", path.display()))
    }

    /// Parses a config, resolving relative file paths against `base_dir`.
    pub fn parse(text: &str, base_dir: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(text)?;
        config.google_client_file = resolve(&config.google_client_file, base_dir)?;
        if let Some(key_file) = &config.fcbq_api_key_file {
            config.fcbq_api_key_file = Some(resolve(key_file, base_dir)?);
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=24 * 60).contains(&self.event_duration_minutes),
            "event_duration_minutes must be between 1 and 1440"
        );
        let mut team_ids = HashSet::new();
        for team in &self.teams {
            ensure!(
                !team.label.trim().is_empty(),
                "team {} has an empty label",
                team.fcbq_team_id
            );
            ensure!(
                !team.calendar_id.trim().is_empty(),
                "team {} has an empty calendar_id",
                team.label
            );
            ensure!(
                team_ids.insert(team.fcbq_team_id),
                "fcbq_team_id {} is configured more than once",
                team.fcbq_team_id
            );
        }
        Ok(())
    }

    pub fn event_duration(&self) -> chrono::Duration {
        chrono::Duration::minutes(self.event_duration_minutes.into())
    }
}

/// The FCBQ access key: the environment value when set, else the configured key file.
pub fn fcbq_api_key(env_value: Option<String>, config: Option<&Config>) -> Result<String> {
    if let Some(key) = env_value
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
    {
        return Ok(key);
    }
    let Some(path) = config.and_then(|config| config.fcbq_api_key_file.as_ref()) else {
        bail!(
            "set {} or fcbq_api_key_file in the config",
            crate::fcbq::API_KEY_ENV
        );
    };
    let key = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the FCBQ key file {}", path.display()))?;
    let key = key.trim();
    ensure!(
        !key.is_empty(),
        "the FCBQ key file {} is empty",
        path.display()
    );
    Ok(key.to_owned())
}

/// `$XDG_CONFIG_HOME/bac-calendar/config.toml`.
pub fn default_config_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?
        .join(APP_DIR)
        .join("config.toml"))
}

/// `$XDG_DATA_HOME/bac-calendar/google-token.json`, where the refresh token lives.
pub fn default_token_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?
        .join(APP_DIR)
        .join("google-token.json"))
}

fn xdg_dir(var: &str, home_fallback: &str) -> Result<PathBuf> {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Ok(dir),
        _ => Ok(home()?.join(home_fallback)),
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .context("HOME is not set")
}

fn resolve(path: &Path, base_dir: &Path) -> Result<PathBuf> {
    if let Ok(rest) = path.strip_prefix("~") {
        return Ok(home()?.join(rest));
    }
    Ok(base_dir.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "/home/user/.config/bac-calendar";

    fn parse(text: &str) -> Result<Config> {
        Config::parse(text, Path::new(BASE))
    }

    #[test]
    fn parses_teams_and_resolves_relative_paths() {
        let config = parse(
            r#"
            google_client_file = "client_secret.json"
            fcbq_api_key_file = "/run/secrets/fcbq"

            [[teams]]
            label = "BAC Sènior M"
            fcbq_team_id = 92430
            calendar_id = "abc@group.calendar.google.com"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.google_client_file,
            Path::new(BASE).join("client_secret.json")
        );
        assert_eq!(
            config.fcbq_api_key_file.as_deref(),
            Some(Path::new("/run/secrets/fcbq"))
        );
        assert_eq!(
            config.event_duration_minutes,
            DEFAULT_EVENT_DURATION_MINUTES
        );
        assert_eq!(
            config.teams,
            [Team {
                label: "BAC Sènior M".into(),
                fcbq_team_id: 92430,
                calendar_id: "abc@group.calendar.google.com".into(),
            }]
        );
    }

    #[test]
    fn a_config_without_teams_is_valid() {
        let config = parse(r#"google_client_file = "/c.json""#).unwrap();
        assert!(config.teams.is_empty());
    }

    #[test]
    fn rejects_a_team_configured_twice() {
        let err = parse(
            r#"
            google_client_file = "/c.json"
            [[teams]]
            label = "A"
            fcbq_team_id = 1
            calendar_id = "a"
            [[teams]]
            label = "B"
            fcbq_team_id = 1
            calendar_id = "b"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("more than once"), "{err}");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("google_client_file = \"/c.json\"\nfcbq_api_key = \"secret\"").is_err());
    }

    #[test]
    fn rejects_a_zero_duration() {
        assert!(parse("google_client_file = \"/c.json\"\nevent_duration_minutes = 0").is_err());
    }

    #[test]
    fn the_environment_key_wins_over_the_key_file() {
        let config = parse("google_client_file = \"/c.json\"\nfcbq_api_key_file = \"/nope\"");
        let key = fcbq_api_key(Some(" from-env \n".into()), config.as_ref().ok()).unwrap();
        assert_eq!(key, "from-env");
    }

    #[test]
    fn reads_the_key_file_when_the_environment_is_empty() {
        let dir = std::env::temp_dir().join(format!("bac-calendar-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("key"), "file-key\n").unwrap();
        let config = Config::parse(
            "google_client_file = \"c.json\"\nfcbq_api_key_file = \"key\"",
            &dir,
        )
        .unwrap();

        assert_eq!(
            fcbq_api_key(Some(String::new()), Some(&config)).unwrap(),
            "file-key"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asks_for_a_key_when_none_is_configured() {
        let err = fcbq_api_key(None, None).unwrap_err();
        assert!(err.to_string().contains("FCBQ_API_KEY"), "{err}");
    }
}
