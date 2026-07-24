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

    /// The `scylla` Cassandra/ScyllaDB (CQL) driver returned an error. Carried as the
    /// driver's rendered message: scylla's error tree is deep and versioned, so we keep
    /// the string rather than binding its types into our public error enum.
    #[error("cassandra error: {0}")]
    Cql(String),

    /// A connection pool failed to hand out a connection (timeout, manager error, ...).
    #[error("connection pool error: {0}")]
    Pool(String),

    /// Failed to read or write the on-disk JSON config of saved connections.
    #[error("config error: {0}")]
    Config(String),

    /// Failed to (de)serialize config data.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Writing an export to disk (or any other filesystem I/O) failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// Reading/writing a secret to the OS keychain failed.
    #[error("keychain error: {0}")]
    Keyring(String),

    /// A connection config was malformed (missing host, bad path, ...).
    #[error("invalid connection: {0}")]
    InvalidConfig(String),

    /// Establishing or operating the SSH tunnel failed (connect, auth, forward, ...).
    #[error("ssh tunnel error: {0}")]
    Ssh(String),

    /// A file being imported was malformed: a ragged CSV row, a nested JSON value, a top-level
    /// document that is not an array of objects. Carries a message naming the offending row.
    #[error("import error: {0}")]
    Import(String),

    /// One statement of a multi-statement batch failed. Carries the 1-based position of the
    /// failing statement so the user can locate it in the editor; statements before it have
    /// already executed (the batch runs in autocommit, statement by statement).
    #[error("statement {0} failed: {1}")]
    Statement(usize, Box<CoreError>),

    /// The query was cancelled by the user (Cancel button). Carried as a distinct variant —
    /// not a generic failure — so the UI can show "Query cancelled" instead of a red error
    /// and skip recording it as a failed statement.
    #[error("query cancelled")]
    Canceled,
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CoreError>;
