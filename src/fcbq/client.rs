use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::envelope::decode_envelope;
use super::error::{Error, Result};
use super::model::{Club, MatchRecord};

/// Base URL of the ESB backend, without the key segment.
pub const DEFAULT_BASE_URL: &str = "https://esb.optimalwayconsulting.com/fcbq/1";

/// Environment variable holding the ESB access key.
pub const API_KEY_ENV: &str = "FCBQ_API_KEY";

/// Blocking client for the ESB backend.
///
/// The access key travels as a URL path segment, so it is redacted from every
/// error this client reports and from its `Debug` output.
pub struct Client {
    base_url: String,
    api_key: String,
    http: ureq::Agent,
}

impl Client {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            http,
        }
    }

    /// Every match of a team in the current season.
    ///
    /// Team ids are per season: a club's teams get new ids every September.
    pub fn team_season(&self, team_id: u64) -> Result<Vec<MatchRecord>> {
        self.get(&format!("Match/getByTeamAllSeason/{team_id}"))
    }

    /// A club's card with its teams for the current season.
    pub fn club(&self, club_id: u64) -> Result<Club> {
        self.get(&format!("FCBQWeb/fitxaClub/{club_id}"))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/{}/{}", self.base_url, self.api_key, path);
        let mut response = self
            .http
            .get(&url)
            .call()
            .map_err(|err| self.transport(err))?;
        let status = response.status();
        let body = response
            .body_mut()
            .read_to_vec()
            .map_err(|err| self.transport(err))?;
        decode_envelope(&body).map_err(|err| match err {
            Error::Undecodable if !status.is_success() => Error::Status(status.as_u16()),
            other => other,
        })
    }

    fn transport(&self, err: ureq::Error) -> Error {
        Error::Transport(err.to_string().replace(&self.api_key, "{KEY}"))
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}
