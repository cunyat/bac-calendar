use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while talking to the FCBQ backend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request never got an answer. The message has the access key redacted.
    #[error("request to the FCBQ backend failed: {0}")]
    Transport(String),

    #[error("FCBQ backend answered HTTP {0} with a body that is not an envelope")]
    Status(u16),

    #[error("FCBQ response is neither base64 nor JSON")]
    Undecodable,

    #[error("FCBQ response is not the expected JSON shape")]
    Malformed(#[from] serde_json::Error),

    #[error(transparent)]
    Api(#[from] ApiError),
}

/// A `result: "ERROR"` envelope, the backend's way of reporting failure on an
/// HTTP 200 response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub error_code: String,
    pub message: String,
}

impl ApiError {
    /// Whether the backend rejected the access key, which happens when it is
    /// wrong or has been rotated.
    pub fn is_access_denied(&self) -> bool {
        self.message.eq_ignore_ascii_case("ACCESS DENIED")
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_access_denied() {
            write!(
                f,
                "FCBQ denied access ({}): the API key is wrong or has been rotated",
                self.error_code
            )
        } else {
            write!(f, "FCBQ error: {} ({})", self.message, self.error_code)
        }
    }
}

impl std::error::Error for ApiError {}
