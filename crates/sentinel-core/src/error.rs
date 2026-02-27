use thiserror::Error;

#[derive(Debug, Error)]
pub enum SentinelError {
    #[error("credential not found: {0}")]
    CredentialNotFound(String),

    #[error("policy violation: {0}")]
    PolicyViolation(String),

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("integration error: {service}: {message}")]
    IntegrationError { service: String, message: String },

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}
