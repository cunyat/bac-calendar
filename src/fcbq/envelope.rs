use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::error::{ApiError, Error, Result};

/// The wrapper every ESB endpoint answers with.
///
/// `errorCode` is a number, a string or `null` depending on the failure, and
/// `messageData` changes shape per endpoint, so both stay raw until `result`
/// is known to be `OK`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    result: String,
    #[serde(default)]
    error_code: Value,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    message_data: Value,
}

impl Envelope {
    fn into_payload(self) -> Result<Value> {
        if self.result.eq_ignore_ascii_case("OK") {
            return Ok(self.message_data);
        }
        Err(Error::Api(ApiError {
            error_code: match self.error_code {
                Value::String(code) => code,
                other => other.to_string(),
            },
            message: self.message.unwrap_or_default(),
        }))
    }
}

/// Decodes a raw ESB response body into the payload of its envelope.
///
/// The body is base64-encoded JSON, except for failures served as plain JSON,
/// so both encodings are accepted. A `result` other than `OK` becomes
/// [`Error::Api`].
pub fn decode_envelope<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    let decoded = STANDARD.decode(body.trim_ascii()).ok();
    let mut malformed = None;

    for candidate in decoded.as_deref().into_iter().chain(std::iter::once(body)) {
        match serde_json::from_slice::<Envelope>(candidate) {
            Ok(envelope) => return Ok(serde_json::from_value(envelope.into_payload()?)?),
            Err(err) if serde_json::from_slice::<Value>(candidate).is_ok() => {
                malformed.get_or_insert(err);
            }
            Err(_) => {}
        }
    }

    Err(malformed.map_or(Error::Undecodable, Error::Malformed))
}
