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
    /// SQL beautifier: convert reserved keywords to ALL CAPS. `None` = the default (on).
    #[serde(default)]
    pub beautify_uppercase: Option<bool>,
    /// SQL beautifier: indent width in spaces. `None` = the default (2).
    #[serde(default)]
    pub beautify_indent: Option<u8>,
    /// Whether the user has completed the first-run welcome flow. `None` = not yet.
    #[serde(default)]
    pub welcomed: Option<bool>,
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

/// Path to the JSON file holding the saved workspace (open query tabs + their state).
pub fn workspace_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("workspace.json"))
}

/// The table a tab's result was read from, persisted so a restored tab can re-run its
/// query and stay editable. Mirrors the UI's `EditSource` without depending on it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSource {
    #[serde(default)]
    pub schema: Option<String>,
    pub table: String,
    #[serde(default)]
    pub pk_cols: Vec<String>,
}

/// One saved query tab. Only non-transient state is kept — never the result rows, which are
/// re-fetched on demand when the user re-runs the query.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceTab {
    #[serde(default)]
    pub title: String,
    /// Id of the connection this tab runs against (`None` = unbound).
    #[serde(default)]
    pub conn_id: Option<String>,
    #[serde(default)]
    pub sql: String,
    /// The table this tab represents, if it was opened from the schema sidebar.
    #[serde(default)]
    pub source: Option<WorkspaceSource>,
}

/// The persisted workspace: the open query tabs and which one was active.
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub tabs: Vec<WorkspaceTab>,
}

/// Load the saved workspace. A missing or unreadable file yields the default (empty) — the
/// workspace is a convenience, never load-bearing, so we don't surface an error.
pub fn load_workspace() -> Workspace {
    workspace_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Atomically persist the workspace.
pub fn save_workspace(workspace: &Workspace) -> Result<()> {
    write_json_atomic(&workspace_path()?, workspace)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace survives a JSON serialise/deserialise round-trip with all fields intact.
    #[test]
    fn workspace_round_trips_through_json() {
        let ws = Workspace {
            active_tab: 1,
            tabs: vec![
                WorkspaceTab {
                    title: "Query 1".into(),
                    conn_id: Some("conn-abc".into()),
                    sql: "SELECT * FROM users;".into(),
                    source: Some(WorkspaceSource {
                        schema: Some("public".into()),
                        table: "users".into(),
                        pk_cols: vec!["id".into()],
                    }),
                },
                WorkspaceTab {
                    title: "scratch".into(),
                    conn_id: None,
                    sql: "SELECT 1;".into(),
                    source: None,
                },
            ],
        };

        let json = serde_json::to_vec(&ws).unwrap();
        let back: Workspace = serde_json::from_slice(&json).unwrap();

        assert_eq!(back.active_tab, 1);
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.tabs[0].conn_id.as_deref(), Some("conn-abc"));
        assert_eq!(back.tabs[0].sql, "SELECT * FROM users;");
        let src = back.tabs[0].source.as_ref().unwrap();
        assert_eq!(src.table, "users");
        assert_eq!(src.pk_cols, vec!["id".to_string()]);
        assert!(back.tabs[1].conn_id.is_none());
        assert!(back.tabs[1].source.is_none());
    }

    /// Missing/empty fields fall back to defaults (forward-compatible with older files).
    #[test]
    fn workspace_tolerates_missing_fields() {
        let json = br#"{"tabs":[{"sql":"SELECT 1;"}]}"#;
        let ws: Workspace = serde_json::from_slice(json).unwrap();
        assert_eq!(ws.active_tab, 0);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].sql, "SELECT 1;");
        assert_eq!(ws.tabs[0].title, "");
        assert!(ws.tabs[0].conn_id.is_none());
    }
}
