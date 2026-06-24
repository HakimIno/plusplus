//! Saved queries (favorites) — a permanent, user-managed collection of named SQL snippets,
//! stored as a single pretty-JSON array beside the other config files. Unlike
//! [`crate::history`] (an append-only, size-capped audit log), favorites are never pruned:
//! they are added, renamed, and deleted explicitly by the user and rewritten atomically on
//! every change. A torn/garbled file degrades to "no favorites" rather than failing the app.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{CoreError, Result};

/// One saved query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorite {
    /// Stable id, assigned on creation and preserved across renames so the UI can target a
    /// specific entry regardless of list order. See [`new_id`].
    pub id: String,
    /// User-facing name (defaults to the query's first line when saved from history).
    pub name: String,
    pub sql: String,
    /// Connection this query was saved against, if any (display only — a favorite can be
    /// loaded into a tab bound to any connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conn_name: Option<String>,
    /// RFC 3339 UTC timestamp of when the favorite was first saved.
    pub created_at: String,
}

/// Mint a process-unique id for a new favorite: the creation time plus a monotonic counter,
/// so two favorites saved in the same second still differ. Avoids pulling in a uuid crate.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        chrono::Utc::now().timestamp_micros(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// Path to the favorites file, e.g. `~/.config/plusplus/favorites.json`.
pub fn favorites_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("favorites.json"))
}

/// Load all saved favorites (oldest first, as stored). A missing or unreadable file yields
/// an empty list rather than an error.
pub fn load() -> Result<Vec<Favorite>> {
    load_at(&favorites_path()?)
}

/// Overwrite the favorites file with `items`, atomically.
pub fn save(items: &[Favorite]) -> Result<()> {
    config::write_json_atomic(&favorites_path()?, items)
}

fn load_at(path: &Path) -> Result<Vec<Favorite>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CoreError::Config(format!("favorites {}: {e}", path.display()))),
    };
    // A corrupt file shouldn't brick the feature — degrade to an empty list.
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(name: &str, sql: &str) -> Favorite {
        Favorite {
            id: new_id(),
            name: name.into(),
            sql: sql.into(),
            conn_id: Some("c1".into()),
            conn_name: Some("test".into()),
            created_at: crate::history::now_rfc3339(),
        }
    }

    fn temp_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plusplus-fav-test-{}-{}.json",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn ids_are_unique() {
        assert_ne!(new_id(), new_id());
    }

    #[test]
    fn missing_file_is_empty() {
        let path = temp_path();
        assert!(load_at(&path).unwrap().is_empty());
    }

    #[test]
    fn corrupt_file_degrades_to_empty() {
        let path = temp_path();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_at(&path).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_rename_delete_roundtrip() {
        let path = temp_path();
        let mut items = vec![fav("first", "SELECT 1"), fav("second", "SELECT 2")];
        config::write_json_atomic(&path, &items).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "first");

        // Rename keeps the id.
        let id1 = items[0].id.clone();
        items[0].name = "renamed".into();
        config::write_json_atomic(&path, &items).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded[0].name, "renamed");
        assert_eq!(loaded[0].id, id1);

        // Delete by id.
        items.retain(|f| f.id != id1);
        config::write_json_atomic(&path, &items).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "second");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn thai_sql_survives_the_roundtrip() {
        let path = temp_path();
        let items = vec![fav("ลูกค้า", "SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'")];
        config::write_json_atomic(&path, &items).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded[0].sql, "SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'");
        assert_eq!(loaded[0].name, "ลูกค้า");
        let _ = std::fs::remove_file(&path);
    }
}
