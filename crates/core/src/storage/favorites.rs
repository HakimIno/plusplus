//! Saved queries (favorites) — a permanent, user-managed collection of named SQL snippets,
//! stored beside the other config files. Unlike [`crate::history`] (an append-only, size-capped
//! audit log), favorites are never pruned: they are added, renamed, grouped, and deleted
//! explicitly by the user and rewritten atomically on every change. A torn/garbled file
//! degrades to "no favorites" rather than failing the app.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{CoreError, Result};

/// Display name for queries that have no folder.
pub const UNGROUPED: &str = "Ungrouped";

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
    /// Folder this query lives in. `None` / empty is shown as [`UNGROUPED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// RFC 3339 UTC timestamp of when the favorite was first saved.
    pub created_at: String,
}

impl Favorite {
    /// Folder label used in the sidebar: a non-empty `folder`, otherwise [`UNGROUPED`].
    pub fn folder_key(&self) -> &str {
        folder_key(self.folder.as_deref())
    }
}

/// On-disk saved-query file: named folders plus the queries themselves. Older files were a
/// bare JSON array of queries; [`load`] still accepts that shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FavoritesStore {
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    pub queries: Vec<Favorite>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FavoritesDisk {
    Legacy(Vec<Favorite>),
    Store(FavoritesStore),
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

/// Load saved queries and folder names. A missing or unreadable file yields an empty store
/// rather than an error.
pub fn load() -> Result<FavoritesStore> {
    load_at(&favorites_path()?)
}

/// Overwrite the favorites file with `store`, atomically.
pub fn save(store: &FavoritesStore) -> Result<()> {
    config::write_json_atomic(&favorites_path()?, store)
}

/// Folder label for a stored `folder` value.
pub fn folder_key(folder: Option<&str>) -> &str {
    match folder.map(str::trim) {
        Some(name) if !name.is_empty() => name,
        _ => UNGROUPED,
    }
}

/// Groups query indices by folder. Named folders (saved order, then any extras alphabetically)
/// come first; [`UNGROUPED`] is last. Empty named folders are kept when `keep_empty` is set.
pub fn grouped(
    queries: &[Favorite],
    folders: &[String],
    filter: &str,
    keep_empty: bool,
) -> Vec<(String, Vec<usize>)> {
    let needle = filter.trim().to_lowercase();
    let matches = |query: &Favorite| {
        needle.is_empty()
            || query.name.to_lowercase().contains(&needle)
            || query.sql.to_lowercase().contains(&needle)
            || query.folder_key().to_lowercase().contains(&needle)
    };

    let mut seen = HashSet::new();
    let mut groups = Vec::new();

    for folder in folders {
        let name = folder.trim();
        if name.eq_ignore_ascii_case(UNGROUPED) {
            continue;
        }
        push_group(name, queries, &needle, keep_empty, &mut seen, &mut groups);
    }

    let mut extras: Vec<String> = queries
        .iter()
        .map(|query| query.folder_key().to_string())
        .filter(|name| name != UNGROUPED && !seen.contains(name))
        .collect();
    extras.sort();
    extras.dedup();
    for name in extras {
        push_group(&name, queries, &needle, keep_empty, &mut seen, &mut groups);
    }

    let ungrouped: Vec<usize> = queries
        .iter()
        .enumerate()
        .filter(|(_, query)| query.folder_key() == UNGROUPED && matches(query))
        .map(|(idx, _)| idx)
        .collect();
    if !ungrouped.is_empty() || (groups.is_empty() && needle.is_empty()) {
        groups.push((UNGROUPED.to_string(), ungrouped));
    }
    groups
}

/// Named-folder order used by the sidebar. [`UNGROUPED`] is not a real folder and is ignored.
pub fn reorder_folder(folders: &mut Vec<String>, source: &str, target: &str, after: bool) {
    let source = source.trim();
    let target = target.trim();
    if source.is_empty()
        || target.is_empty()
        || source.eq_ignore_ascii_case(UNGROUPED)
        || target.eq_ignore_ascii_case(UNGROUPED)
        || source.eq_ignore_ascii_case(target)
    {
        return;
    }
    let Some(source_index) = folders
        .iter()
        .position(|folder| folder.eq_ignore_ascii_case(source))
    else {
        return;
    };
    let source_name = folders.remove(source_index);
    let Some(mut target_index) = folders
        .iter()
        .position(|folder| folder.eq_ignore_ascii_case(target))
    else {
        folders.insert(source_index, source_name);
        return;
    };
    if after {
        target_index += 1;
    }
    folders.insert(target_index, source_name);
}

fn stored_folder(folder: Option<&str>) -> Option<String> {
    match folder.map(str::trim) {
        Some(name) if !name.is_empty() && !name.eq_ignore_ascii_case(UNGROUPED) => {
            Some(name.to_string())
        }
        _ => None,
    }
}

/// Move `source_id` into `folder` (`None` / Ungrouped), placing it after that folder's last query.
pub fn move_query_to_folder(queries: &mut Vec<Favorite>, source_id: &str, folder: Option<&str>) {
    let folder = stored_folder(folder);
    let Some(source_index) = queries.iter().position(|query| query.id == source_id) else {
        return;
    };
    let mut item = queries.remove(source_index);
    item.folder = folder.clone();
    let insert_at = queries
        .iter()
        .rposition(|query| query.folder == folder)
        .map(|idx| idx + 1)
        .unwrap_or(queries.len());
    queries.insert(insert_at, item);
}

/// Move `source_id` beside `target_id` and into the same folder.
pub fn reorder_query(queries: &mut Vec<Favorite>, source_id: &str, target_id: &str, after: bool) {
    if source_id == target_id {
        return;
    }
    let Some(source_index) = queries.iter().position(|query| query.id == source_id) else {
        return;
    };
    let mut item = queries.remove(source_index);
    let Some(target_index) = queries.iter().position(|query| query.id == target_id) else {
        queries.insert(source_index, item);
        return;
    };
    item.folder = queries[target_index].folder.clone();
    let insert_at = if after {
        target_index + 1
    } else {
        target_index
    };
    queries.insert(insert_at, item);
}

fn push_group(
    name: &str,
    queries: &[Favorite],
    needle: &str,
    keep_empty: bool,
    seen: &mut HashSet<String>,
    groups: &mut Vec<(String, Vec<usize>)>,
) {
    if name.is_empty() || !seen.insert(name.to_string()) {
        return;
    }
    let idxs: Vec<usize> = queries
        .iter()
        .enumerate()
        .filter(|(_, query)| {
            query.folder_key() == name
                && (needle.is_empty()
                    || query.name.to_lowercase().contains(needle)
                    || query.sql.to_lowercase().contains(needle)
                    || query.folder_key().to_lowercase().contains(needle))
        })
        .map(|(idx, _)| idx)
        .collect();
    let keep = !idxs.is_empty() || (keep_empty && needle.is_empty() && name != UNGROUPED);
    if keep {
        groups.push((name.to_string(), idxs));
    }
}

fn load_at(path: &Path) -> Result<FavoritesStore> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FavoritesStore::default()),
        Err(e) => {
            return Err(CoreError::Config(format!(
                "favorites {}: {e}",
                path.display()
            )))
        }
    };
    // A corrupt file shouldn't brick the feature — degrade to an empty store.
    Ok(match serde_json::from_str::<FavoritesDisk>(&text) {
        Ok(FavoritesDisk::Legacy(queries)) => FavoritesStore {
            folders: Vec::new(),
            queries,
        },
        Ok(FavoritesDisk::Store(store)) => store,
        Err(_) => FavoritesStore::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(name: &str, sql: &str, folder: Option<&str>) -> Favorite {
        Favorite {
            id: new_id(),
            name: name.into(),
            sql: sql.into(),
            conn_id: Some("c1".into()),
            conn_name: Some("test".into()),
            folder: folder.map(str::to_string),
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
        let store = load_at(&path).unwrap();
        assert!(store.queries.is_empty());
        assert!(store.folders.is_empty());
    }

    #[test]
    fn corrupt_file_degrades_to_empty() {
        let path = temp_path();
        std::fs::write(&path, b"{ not json").unwrap();
        let store = load_at(&path).unwrap();
        assert!(store.queries.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_array_file_still_loads() {
        let path = temp_path();
        let items = vec![fav("first", "SELECT 1", None)];
        config::write_json_atomic(&path, &items).unwrap();
        let store = load_at(&path).unwrap();
        assert_eq!(store.queries.len(), 1);
        assert_eq!(store.queries[0].name, "first");
        assert!(store.folders.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_rename_delete_roundtrip() {
        let path = temp_path();
        let mut store = FavoritesStore {
            folders: vec!["Reports".into()],
            queries: vec![
                fav("first", "SELECT 1", Some("Reports")),
                fav("second", "SELECT 2", None),
            ],
        };
        config::write_json_atomic(&path, &store).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.queries.len(), 2);
        assert_eq!(loaded.folders, ["Reports"]);
        assert_eq!(loaded.queries[0].folder.as_deref(), Some("Reports"));

        let id1 = store.queries[0].id.clone();
        store.queries[0].name = "renamed".into();
        config::write_json_atomic(&path, &store).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.queries[0].name, "renamed");
        assert_eq!(loaded.queries[0].id, id1);

        store.queries.retain(|f| f.id != id1);
        config::write_json_atomic(&path, &store).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.queries.len(), 1);
        assert_eq!(loaded.queries[0].name, "second");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn thai_sql_survives_the_roundtrip() {
        let path = temp_path();
        let store = FavoritesStore {
            folders: Vec::new(),
            queries: vec![fav("ลูกค้า", "SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'", None)],
        };
        config::write_json_atomic(&path, &store).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(
            loaded.queries[0].sql,
            "SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'"
        );
        assert_eq!(loaded.queries[0].name, "ลูกค้า");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn grouped_puts_named_folders_before_ungrouped() {
        let queries = vec![
            fav("a", "SELECT 1", None),
            fav("b", "SELECT 2", Some("Reports")),
            fav("c", "SELECT 3", Some("Admin")),
        ];
        let groups = grouped(&queries, &["Reports".into()], "", true);
        let names: Vec<&str> = groups.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Reports", "Admin", UNGROUPED]);
        assert_eq!(groups[0].1, vec![1]);
        assert_eq!(groups[1].1, vec![2]);
        assert_eq!(groups[2].1, vec![0]);
    }

    #[test]
    fn grouped_keeps_empty_named_folders_until_filtered() {
        let queries = vec![fav("a", "SELECT 1", None)];
        let folders = vec!["Empty".into()];
        let shown = grouped(&queries, &folders, "", true);
        assert_eq!(shown[0].0, "Empty");
        assert!(shown[0].1.is_empty());
        let filtered = grouped(&queries, &folders, "nope", true);
        assert!(filtered.is_empty());
    }

    #[test]
    fn grouped_filter_matches_name_without_requiring_sql_in_the_ui() {
        let queries = vec![
            fav("Slam", "SELECT * FROM documents", Some("Work")),
            fav("other", "SELECT 1", None),
        ];
        let groups = grouped(&queries, &["Work".into()], "slam", false);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "Work");
        assert_eq!(groups[0].1, vec![0]);
    }

    #[test]
    fn reorder_folder_inserts_before_or_after_the_target() {
        let mut folders = vec!["A".into(), "B".into(), "C".into()];
        reorder_folder(&mut folders, "A", "C", false);
        assert_eq!(folders, ["B", "A", "C"]);
        reorder_folder(&mut folders, "A", "C", true);
        assert_eq!(folders, ["B", "C", "A"]);
        reorder_folder(&mut folders, UNGROUPED, "B", true);
        assert_eq!(folders, ["B", "C", "A"]);
    }

    #[test]
    fn move_and_reorder_queries_follow_folder_then_drop_position() {
        let mut queries = vec![
            fav("a", "SELECT 1", Some("Reports")),
            fav("b", "SELECT 2", Some("Reports")),
            fav("c", "SELECT 3", None),
        ];
        let a = queries[0].id.clone();
        let b = queries[1].id.clone();
        let c = queries[2].id.clone();

        reorder_query(&mut queries, &a, &b, true);
        assert_eq!(
            queries.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );

        move_query_to_folder(&mut queries, &c, Some("Reports"));
        assert_eq!(queries[2].folder.as_deref(), Some("Reports"));
        assert_eq!(queries[2].id, c);

        reorder_query(&mut queries, &c, &b, false);
        assert_eq!(
            queries.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
        assert!(queries
            .iter()
            .all(|q| q.folder.as_deref() == Some("Reports")));
    }
}
