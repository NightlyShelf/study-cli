#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid TOML: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("cannot serialize TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("keyring: {0}")]
    Keyring(String),
}

pub type Result<T> = std::result::Result<T, Error>;
