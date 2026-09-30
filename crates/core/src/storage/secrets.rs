//! Password storage via the OS keychain (`keyring`). Passwords are **never** written to
//! the JSON config; they are keyed by the connection's id under a single service name.
//!
//! Reading the keychain can trigger an OS access prompt every time (notably on an unsigned
//! macOS build, where the keychain can't pin its "always allow" grant to a stable code
//! signature). To avoid prompting on every connect, a successful read is cached in memory
//! for the rest of the session, so each secret is fetched from the keychain at most once per
//! app launch. The cache is **memory-only** — it is never persisted — and is kept coherent
//! with the keychain by updating it on writes and evicting on deletes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use keyring::Entry;

use crate::error::{CoreError, Result};

const SERVICE: &str = "plusplus";

/// Process-global, in-memory secret cache (account id → secret). Lives only as long as the
/// process, i.e. one entry per secret per app launch. Never written to disk.
fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the cache, recovering from a poisoned mutex (a panic in another thread while it held
/// the lock) rather than propagating the panic — the map is plain data, so the worst case is
/// a slightly stale view, which the next keychain read/write corrects.
fn cache_lock() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    cache().lock().unwrap_or_else(PoisonError::into_inner)
}

fn entry(connection_id: &str) -> Result<Entry> {
    Entry::new(SERVICE, connection_id).map_err(|e| CoreError::Keyring(e.to_string()))
}

/// Store (or replace) the password for a connection, refreshing the in-memory cache so the
/// next read returns the new value without touching the keychain.
pub fn set_password(connection_id: &str, password: &str) -> Result<()> {
    entry(connection_id)?
        .set_password(password)
        .map_err(|e| CoreError::Keyring(e.to_string()))?;
    cache_lock().insert(connection_id.to_string(), password.to_string());
    Ok(())
}

/// Fetch the stored password, if any. A missing entry yields `Ok(None)` rather than an error.
/// The first successful read this session is cached; subsequent reads skip the keychain (and
/// thus any OS access prompt). A miss is not cached, so a later `set_password` is seen.
pub fn get_password(connection_id: &str) -> Result<Option<String>> {
    if let Some(cached) = cache_lock().get(connection_id) {
        return Ok(Some(cached.clone()));
    }
    match entry(connection_id)?.get_password() {
        Ok(p) => {
            cache_lock().insert(connection_id.to_string(), p.clone());
            Ok(Some(p))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Keyring(e.to_string())),
    }
}

/// Delete the stored password for a connection (e.g. when the connection is removed), also
/// evicting it from the cache. A missing entry is treated as success.
pub fn delete_password(connection_id: &str) -> Result<()> {
    cache_lock().remove(connection_id);
    match entry(connection_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CoreError::Keyring(e.to_string())),
    }
}

/// Drop every cached secret (e.g. to "lock" the app), forcing the next read to go back to the
/// keychain. The keychain entries themselves are untouched.
pub fn clear_cache() {
    cache_lock().clear();
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
