//! Error types for the data layer. We never panic on a fallible runtime path;
//! everything bubbles up as a `CoreError`.

use thiserror::Error;

/// Errors that can occur while connecting to, introspecting, or querying a database.
#[derive(Debug, Error)]
pub enum CoreError {
    /// The underlying `sqlx` driver returned an error (connection, protocol, SQL, ...).
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// The `tiberius` SQL Server (TDS) driver returned an error.
    #[error("sql server error: {0}")]
    Tiberius(#[from] tiberius::error::Error),

    /// A connection pool failed to hand out a connection (timeout, manager error, ...).
    #[error("connection pool error: {0}")]
    Pool(String),

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

    /// Establishing or operating the SSH tunnel failed (connect, auth, forward, ...).
    #[error("ssh tunnel error: {0}")]
    Ssh(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CoreError>;
