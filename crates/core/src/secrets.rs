//! Password storage via the OS keychain (`keyring`). Passwords are **never** written to
//! the JSON config; they are keyed by the connection's id under a single service name.

use keyring::Entry;

use crate::error::{CoreError, Result};

const SERVICE: &str = "plusplus";

fn entry(connection_id: &str) -> Result<Entry> {
    Entry::new(SERVICE, connection_id).map_err(|e| CoreError::Keyring(e.to_string()))
}

/// Store (or replace) the password for a connection.
pub fn set_password(connection_id: &str, password: &str) -> Result<()> {
    entry(connection_id)?
        .set_password(password)
        .map_err(|e| CoreError::Keyring(e.to_string()))
}

/// Fetch the stored password, if any. A missing entry yields `Ok(None)` rather than an error.
pub fn get_password(connection_id: &str) -> Result<Option<String>> {
    match entry(connection_id)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Keyring(e.to_string())),
    }
}

/// Delete the stored password for a connection (e.g. when the connection is removed).
/// A missing entry is treated as success.
pub fn delete_password(connection_id: &str) -> Result<()> {
    match entry(connection_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CoreError::Keyring(e.to_string())),
    }
}

/// The SSH credential (bastion password, or key passphrase) lives in its own keychain
/// entry beside the database password, keyed by a suffixed account name.
fn ssh_account(connection_id: &str) -> String {
    format!("{connection_id}.ssh")
}

/// Store (or replace) the SSH password / key passphrase for a connection.
pub fn set_ssh_secret(connection_id: &str, secret: &str) -> Result<()> {
    set_password(&ssh_account(connection_id), secret)
}

/// Fetch the stored SSH password / key passphrase, if any.
pub fn get_ssh_secret(connection_id: &str) -> Result<Option<String>> {
    get_password(&ssh_account(connection_id))
}

/// Delete the stored SSH credential. A missing entry is treated as success.
pub fn delete_ssh_secret(connection_id: &str) -> Result<()> {
    delete_password(&ssh_account(connection_id))
}
