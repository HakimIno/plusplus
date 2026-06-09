//! Persistence of the (non-secret) saved-connection list to a JSON file in the user's
//! config directory. Passwords are handled separately in [`crate::secrets`].

use std::path::PathBuf;

use crate::error::{CoreError, Result};
use crate::model::ConnectionConfig;

/// Directory where plusplus stores its config, e.g. `~/.config/plusplus` on Linux/macOS or
/// `%APPDATA%\plusplus` on Windows. We avoid an extra crate and derive it from env vars.
fn config_dir() -> Result<PathBuf> {
    // Honour XDG on unix, APPDATA on Windows, else fall back to ~/.config/plusplus.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("plusplus"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Ok(PathBuf::from(appdata).join("plusplus"));
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| CoreError::Config("could not determine home directory".into()))?;
    Ok(PathBuf::from(home).join(".config").join("plusplus"))
}

/// Path to the JSON file holding the list of saved connections.
pub fn connections_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("connections.json"))
}

/// Load saved connections. A missing file is not an error — it yields an empty list.
pub fn load_connections() -> Result<Vec<ConnectionConfig>> {
    let path = connections_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(CoreError::Config(format!(
            "reading {}: {e}",
            path.display()
        ))),
    }
}

/// Atomically persist the connection list (write to a temp file, then rename).
pub fn save_connections(conns: &[ConnectionConfig]) -> Result<()> {
    write_json_atomic(&connections_path()?, conns)
}

/// Path to the JSON file holding misc app settings (theme, …).
pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// User-facing application preferences that aren't tied to a specific connection.
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Stable key of the selected theme (see `ui`'s `ThemeId`). `None` = use the default.
    #[serde(default)]
    pub theme: Option<String>,
}

/// Load app settings. A missing or unreadable file yields defaults — settings are a
/// convenience, never load-bearing, so we don't surface an error.
pub fn load_settings() -> Settings {
    settings_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Atomically persist app settings.
pub fn save_settings(settings: &Settings) -> Result<()> {
    write_json_atomic(&settings_path()?, settings)
}

/// Serialise `value` to pretty JSON and write it to `path` atomically (temp file + rename),
/// creating the config directory if needed.
fn write_json_atomic<T: serde::Serialize + ?Sized>(
    path: &std::path::Path,
    value: &T,
) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoreError::Config(format!("creating {}: {e}", dir.display())))?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, &json)
        .map_err(|e| CoreError::Config(format!("writing {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| CoreError::Config(format!("renaming into {}: {e}", path.display())))?;
    Ok(())
}
