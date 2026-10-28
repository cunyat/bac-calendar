//! The only place that talks to the FCBQ backend behind basquetcatala.cat.
//!
//! The backend is undocumented: responses are a base64 envelope, some failures
//! arrive as plain JSON, and every reply is HTTP 200 whatever happened.

mod client;
mod envelope;
mod error;
mod model;

pub use client::{API_KEY_ENV, Client, DEFAULT_BASE_URL};
pub use envelope::decode_envelope;
pub use error::{ApiError, Error, Result};
pub use model::{Category, Club, ClubTeam, MatchRecord, Side};
