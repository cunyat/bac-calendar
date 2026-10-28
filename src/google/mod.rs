//! Google OAuth 2.0 for installed apps and the Calendar REST API, spoken
//! directly over HTTP.

mod calendar;
pub mod oauth;

use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

pub use calendar::{
    CalendarClient, CalendarListEntry, DEFAULT_BASE_URL, Event, EventTime, ExtendedProperties,
};

/// Everything outside RFC 3986's unreserved characters.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// Turns a non-2xx Google response into an error carrying Google's own message.
fn check(mut response: ureq::http::Response<ureq::Body>, what: &str) -> anyhow::Result<ureq::Body> {
    let status = response.status();
    if status.is_success() {
        return Ok(response.into_body());
    }
    let body = response.body_mut().read_to_string().unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|json| {
            let error = json.get("error")?;
            error
                .get("message")
                .or_else(|| json.get("error_description"))
                .or(Some(error))
                .and_then(|message| message.as_str().map(str::to_owned))
        })
        .unwrap_or(body);
    anyhow::bail!(
        "{what} failed with HTTP {}: {}",
        status.as_u16(),
        message.trim()
    )
}
