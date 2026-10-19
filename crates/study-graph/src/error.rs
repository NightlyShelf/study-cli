#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The refresh token is dead; the caller should re-login.
    #[error("microsoft login expired: {0}")]
    AuthExpired(String),

    /// An `AADSTS…` or other OAuth error, passed through verbatim so the user
    /// can look it up.
    #[error("microsoft auth error {code}: {description}")]
    Auth { code: String, description: String },

    /// Graph answered with an `error` object.
    #[error("graph error {status} {code}: {message}")]
    Graph {
        status: u16,
        code: String,
        message: String,
    },

    /// The device-code flow timed out or was declined.
    #[error("microsoft login failed: {0}")]
    Login(String),
}

pub type Result<T> = std::result::Result<T, Error>;
