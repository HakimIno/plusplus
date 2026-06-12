//! Query history — a local audit log. Every user-initiated statement is appended with
//! its outcome to a JSON Lines file beside the other config files, so "what ran against
//! which database, when, and did it work" survives restarts. Append-only and size-capped;
//! one malformed line (say, a torn write at crash) never poisons the rest.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{CoreError, Result};

/// One executed statement (or transaction batch) and how it went.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// RFC 3339 UTC timestamp of completion.
    pub at: String,
    pub conn_id: String,
    /// Connection display name at the time of execution (the id outlives renames).
    pub conn_name: String,
    pub sql: String,
    pub ok: bool,
    /// Database error message when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Rows returned or affected, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    #[serde(default)]
    pub elapsed_ms: f64,
}

/// Entries kept after compaction, and the most [`load`] returns.
pub const MAX_ENTRIES: usize = 1000;

/// File size that triggers compaction on append. At a generous ~1 KiB per entry this
/// still holds well over `MAX_ENTRIES`, so compaction is rare.
const COMPACT_BYTES: u64 = 1024 * 1024;

/// The current time in the format history entries use.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Path to the history file, e.g. `~/.config/plusplus/history.jsonl`.
pub fn history_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("history.jsonl"))
}

/// Append one entry to the history file (creating it on first use).
pub fn append(entry: &HistoryEntry) -> Result<()> {
    append_at(&history_path()?, entry)
}

/// Load the newest `limit` entries, oldest first.
pub fn load(limit: usize) -> Result<Vec<HistoryEntry>> {
    load_at(&history_path()?, limit)
}

/// Delete the entire history.
pub fn clear() -> Result<()> {
    clear_at(&history_path()?)
}

fn io_err(path: &Path, e: std::io::Error) -> CoreError {
    CoreError::Config(format!("history {}: {e}", path.display()))
}

fn append_at(path: &Path, entry: &HistoryEntry) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err(path, e))?;
    }
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    // Start on a fresh line even if the previous append was torn mid-write (crash):
    // otherwise this entry would glue onto the broken line and be lost with it. The
    // extra blank line in the normal case is filtered out on load.
    if file.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
        file.write_all(b"\n").map_err(|e| io_err(path, e))?;
    }
    file.write_all(&line).map_err(|e| io_err(path, e))?;

    // Keep the file from growing without bound: once it passes the threshold, rewrite
    // it with only the newest entries (atomically, like the JSON configs).
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    drop(file);
    if len > COMPACT_BYTES {
        let keep = load_at(path, MAX_ENTRIES)?;
        let mut buf = Vec::new();
        for e in &keep {
            serde_json::to_writer(&mut buf, e)?;
            buf.push(b'\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, &buf).map_err(|e| io_err(path, e))?;
        std::fs::rename(&tmp, path).map_err(|e| io_err(path, e))?;
    }
    Ok(())
}

fn load_at(path: &Path, limit: usize) -> Result<Vec<HistoryEntry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(path, e)),
    };
    let mut entries: Vec<HistoryEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        // Skip lines that don't parse instead of failing the whole load.
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    Ok(entries)
}

fn clear_at(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_history_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plusplus-history-test-{}-{}.jsonl",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn entry(sql: &str, ok: bool) -> HistoryEntry {
        HistoryEntry {
            at: now_rfc3339(),
            conn_id: "c1".into(),
            conn_name: "test".into(),
            sql: sql.into(),
            ok,
            error: if ok { None } else { Some("boom".into()) },
            rows: ok.then_some(3),
            elapsed_ms: 1.5,
        }
    }

    #[test]
    fn append_load_roundtrip_and_limit() {
        let path = temp_history_path();
        assert!(load_at(&path, 10).unwrap().is_empty()); // missing file = empty

        for i in 0..5 {
            append_at(&path, &entry(&format!("SELECT {i}"), i % 2 == 0)).unwrap();
        }
        let all = load_at(&path, 100).unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].sql, "SELECT 0");
        assert_eq!(all[4].sql, "SELECT 4");
        assert!(!all[1].ok);
        assert_eq!(all[1].error.as_deref(), Some("boom"));

        // `limit` keeps the newest entries.
        let last_two = load_at(&path, 2).unwrap();
        assert_eq!(last_two[0].sql, "SELECT 3");
        assert_eq!(last_two[1].sql, "SELECT 4");

        clear_at(&path).unwrap();
        assert!(load_at(&path, 10).unwrap().is_empty());
        clear_at(&path).unwrap(); // clearing twice is fine
    }

    #[test]
    fn torn_line_is_skipped_not_fatal() {
        let path = temp_history_path();
        append_at(&path, &entry("SELECT 1", true)).unwrap();
        // Simulate a crash mid-append: garbage trailing line.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"at\":\"torn").unwrap();
        drop(f);
        append_at(&path, &entry("SELECT 2", true)).unwrap();

        let all = load_at(&path, 10).unwrap();
        assert_eq!(
            all.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(),
            ["SELECT 1", "SELECT 2"]
        );
        let _ = clear_at(&path);
    }

    #[test]
    fn thai_sql_survives_the_roundtrip() {
        let path = temp_history_path();
        append_at(&path, &entry("SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'", true)).unwrap();
        let all = load_at(&path, 10).unwrap();
        assert_eq!(all[0].sql, "SELECT * FROM ลูกค้า WHERE ชื่อ = 'สมชาย'");
        let _ = clear_at(&path);
    }
}
