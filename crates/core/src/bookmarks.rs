//! Pinned tables (bookmarks) — a small, user-managed set of "favourite" tables per
//! connection, surfaced at the top of the schema explorer for one-click access. Stored as a
//! single pretty-JSON array beside the other config files, rewritten atomically on every
//! change. A torn/garbled file degrades to "no bookmarks" rather than failing the app.
//!
//! This is distinct from [`crate::favorites`], which saves named *SQL snippets*. A bookmark
//! just remembers that a given table (identified by connection + schema + name) is pinned.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{CoreError, Result};

/// One pinned table. Identity is the `(conn_id, schema, table)` triple — the same table name
/// under two schemas (or two connections) pins independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    /// Saved-connection id this bookmark belongs to.
    pub conn_id: String,
    /// Owning schema, when the backend has them (Postgres/SQL Server). `None` for SQLite/MySQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Bare table name (unqualified).
    pub table: String,
}

impl Bookmark {
    /// Whether this bookmark refers to the given table under the given connection.
    pub fn matches(&self, conn_id: &str, schema: Option<&str>, table: &str) -> bool {
        self.conn_id == conn_id && self.schema.as_deref() == schema && self.table == table
    }
}

/// Path to the bookmarks file, e.g. `~/.config/plusplus/bookmarks.json`.
pub fn bookmarks_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("bookmarks.json"))
}

/// Load all pinned tables (in saved order). A missing or unreadable file yields an empty list
/// rather than an error.
pub fn load() -> Result<Vec<Bookmark>> {
    load_at(&bookmarks_path()?)
}

/// Overwrite the bookmarks file with `items`, atomically.
pub fn save(items: &[Bookmark]) -> Result<()> {
    config::write_json_atomic(&bookmarks_path()?, items)
}

fn load_at(path: &Path) -> Result<Vec<Bookmark>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CoreError::Config(format!("bookmarks {}: {e}", path.display()))),
    };
    // A corrupt file shouldn't brick the feature — degrade to an empty list.
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

/// Toggle a table's pinned state within `items`: remove it if already present, otherwise
/// append it. Returns `true` if the table is pinned after the call.
pub fn toggle(
    items: &mut Vec<Bookmark>,
    conn_id: &str,
    schema: Option<&str>,
    table: &str,
) -> bool {
    if let Some(pos) = items.iter().position(|b| b.matches(conn_id, schema, table)) {
        items.remove(pos);
        false
    } else {
        items.push(Bookmark {
            conn_id: conn_id.to_string(),
            schema: schema.map(str::to_string),
            table: table.to_string(),
        });
        true
    }
}

/// Whether a given table is pinned within `items`.
pub fn is_pinned(
    items: &[Bookmark],
    conn_id: &str,
    schema: Option<&str>,
    table: &str,
) -> bool {
    items.iter().any(|b| b.matches(conn_id, schema, table))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plusplus-bm-test-{}-{}.json",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(load_at(&temp_path()).unwrap().is_empty());
    }

    #[test]
    fn corrupt_file_degrades_to_empty() {
        let path = temp_path();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_at(&path).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut items = Vec::new();
        assert!(toggle(&mut items, "c1", Some("public"), "users"));
        assert!(is_pinned(&items, "c1", Some("public"), "users"));
        assert_eq!(items.len(), 1);
        // Same triple toggles back off.
        assert!(!toggle(&mut items, "c1", Some("public"), "users"));
        assert!(items.is_empty());
    }

    #[test]
    fn identity_is_per_connection_and_schema() {
        let mut items = Vec::new();
        toggle(&mut items, "c1", Some("public"), "users");
        // Different schema / connection are distinct bookmarks.
        assert!(!is_pinned(&items, "c1", Some("sales"), "users"));
        assert!(!is_pinned(&items, "c2", Some("public"), "users"));
        // A schema-less (SQLite/MySQL) table is also distinct from a schema-qualified one.
        assert!(!is_pinned(&items, "c1", None, "users"));
    }

    #[test]
    fn round_trips_through_json() {
        let path = temp_path();
        let mut items = Vec::new();
        toggle(&mut items, "c1", Some("public"), "users");
        toggle(&mut items, "c1", None, "ลูกค้า");
        save_at(&path, &items).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded, items);
        let _ = std::fs::remove_file(&path);
    }

    fn save_at(path: &Path, items: &[Bookmark]) -> Result<()> {
        config::write_json_atomic(path, items)
    }
}
