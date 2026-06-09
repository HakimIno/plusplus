//! Error types for the data layer. We never panic on a fallible runtime path;
//! everything bubbles up as a `CoreError`.

use thiserror::Error;

/// Errors that can occur while connecting to, introspecting, or querying a database.
#[derive(Debug, Error)]
pub enum CoreError {
    /// The underlying `sqlx` driver returned an error (connection, protocol, SQL, ...).
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Failed to read or write the on-disk JSON config of saved connections.
    #[error("config error: {0}")]
    Config(String),

    /// Failed to (de)serialize config data.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Reading/writing a secret to the OS keychain failed.
    #[error("keychain error: {0}")]
    Keyring(String),

    /// A connection config was malformed (missing host, bad path, ...).
    #[error("invalid connection: {0}")]
    InvalidConfig(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CoreError>;
