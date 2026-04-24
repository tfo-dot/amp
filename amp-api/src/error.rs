use thiserror::Error;

#[derive(Error, Debug)]
pub enum AmpError {
    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<Box<dyn std::error::Error + Send + Sync>> for AmpError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        AmpError::Unknown(e.to_string())
    }
}

impl From<String> for AmpError {
    fn from(s: String) -> Self {
        AmpError::Unknown(s)
    }
}

impl From<&str> for AmpError {
    fn from(s: &str) -> Self {
        AmpError::Unknown(s.to_string())
    }
}
