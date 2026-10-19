#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The Moodle token is invalid or expired; the caller should re-login.
    #[error("moodle token expired: {0}")]
    TokenExpired(String),

    /// Moodle returned an `exception` payload.
    #[error("moodle error {code}: {message}")]
    Moodle { code: String, message: String },

    /// Launch-flow problems: bad redirect, signature mismatch, mobile service disabled.
    #[error("moodle login failed: {0}")]
    Launch(String),
}

pub type Result<T> = std::result::Result<T, Error>;
