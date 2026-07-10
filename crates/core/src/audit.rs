//! Tamper-resistant audit trail, distinct from [`crate::history`] in three ways that
//! matter for compliance: it records *connection* events as well as statements, it is
//! rotated monthly instead of compacted (nothing is ever rewritten or dropped), and the
//! app offers no way to clear it. History is a user convenience; this is the record of
//! what touched which database, from where, and whether it worked.
//!
//! One JSON Lines file per month (`<config>/audit/audit-YYYY-MM.jsonl`), append-only.
//! Entries never contain secrets: passwords live in the OS keychain and are not part of
//! any event. Statement text *is* recorded (it can contain data values), so the audit
//! log can be disabled entirely in Settings for sensitive work.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{CoreError, Result};

/// What happened. Serialized as a lowercase string tag in the JSONL line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// A connection attempt (successful or not).
    Connect,
    /// A user-initiated statement or batch.
    Query,
    /// Staged in-grid edits committed as a transaction.
    EditCommit,
    /// A schema migration (DDL) applied from the structure editor.
    SchemaApply,
    /// Rows loaded into a table from a CSV/JSON file, as one transaction.
    Import,
}

/// One audited event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// RFC 3339 UTC timestamp.
    pub at: String,
    pub action: AuditAction,
    pub conn_id: String,
    /// Connection display name at the time (the id outlives renames).
    pub conn_name: String,
    /// Where it went: `user@host:port/database` or the SQLite file path.
    #[serde(default)]
    pub target: String,
    /// The statement(s), for Query/EditCommit/SchemaApply. Empty for Connect.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sql: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Rows returned or affected, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    #[serde(default)]
    pub elapsed_ms: f64,
}

/// Directory holding the monthly audit files.
pub fn audit_dir() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("audit"))
}

/// Path of the audit file for the current month.
pub fn current_path() -> Result<PathBuf> {
    let month = chrono::Utc::now().format("%Y-%m");
    Ok(audit_dir()?.join(format!("audit-{month}.jsonl")))
}

/// Append one entry to this month's audit file (creating dir/file on first use).
/// Unlike history there is no compaction: audit files are never rewritten. Rotation
/// happens naturally when the month changes and `current_path` moves on.
pub fn append(entry: &AuditEntry) -> Result<()> {
    append_at(&current_path()?, entry)
}

/// Load the newest `limit` entries of the current month, oldest first (for the viewer).
pub fn load_current_month(limit: usize) -> Result<Vec<AuditEntry>> {
    load_at(&current_path()?, limit)
}

fn io_err(path: &Path, e: std::io::Error) -> CoreError {
    CoreError::Config(format!("audit {}: {e}", path.display()))
}

fn append_at(path: &Path, entry: &AuditEntry) -> Result<()> {
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
    // Start on a fresh line even if the previous append was torn mid-write (crash),
    // so this entry never glues onto a broken line. Blank lines are skipped on load.
    if file.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
        file.write_all(b"\n").map_err(|e| io_err(path, e))?;
    }
    file.write_all(&line).map_err(|e| io_err(path, e))
}

fn load_at(path: &Path, limit: usize) -> Result<Vec<AuditEntry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(path, e)),
    };
    let mut entries: Vec<AuditEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        // One malformed line (torn write) never poisons the rest.
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_audit_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plusplus-audit-test-{}-{}.jsonl",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn entry(action: AuditAction, sql: &str, ok: bool) -> AuditEntry {
        AuditEntry {
            at: crate::history::now_rfc3339(),
            action,
            conn_id: "c1".into(),
            conn_name: "prod".into(),
            target: "app@db.internal:5432/main".into(),
            sql: sql.into(),
            ok,
            error: (!ok).then(|| "permission denied".into()),
            rows: ok.then_some(1),
            elapsed_ms: 0.4,
        }
    }

    #[test]
    fn append_load_roundtrip() {
        let path = temp_audit_path();
        assert!(load_at(&path, 10).unwrap().is_empty());

        append_at(&path, &entry(AuditAction::Connect, "", true)).unwrap();
        append_at(&path, &entry(AuditAction::Query, "SELECT 1", true)).unwrap();
        append_at(&path, &entry(AuditAction::EditCommit, "UPDATE t SET a=1", false)).unwrap();

        let all = load_at(&path, 100).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].action, AuditAction::Connect);
        assert!(all[0].sql.is_empty());
        assert_eq!(all[1].sql, "SELECT 1");
        assert!(!all[2].ok);
        assert_eq!(all[2].error.as_deref(), Some("permission denied"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn torn_line_is_skipped_not_fatal() {
        let path = temp_audit_path();
        append_at(&path, &entry(AuditAction::Query, "SELECT 1", true)).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"at\":\"torn").unwrap();
        drop(f);
        append_at(&path, &entry(AuditAction::Query, "SELECT 2", true)).unwrap();

        let all = load_at(&path, 10).unwrap();
        assert_eq!(
            all.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(),
            ["SELECT 1", "SELECT 2"]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn action_tags_are_stable_snake_case() {
        // The on-disk tag is a compatibility contract for anyone shipping these logs
        // to a SIEM — changing it silently would break their parsers.
        let line = serde_json::to_string(&entry(AuditAction::SchemaApply, "ALTER…", true)).unwrap();
        assert!(line.contains("\"action\":\"schema_apply\""));

        let line = serde_json::to_string(&entry(AuditAction::Import, "-- IMPORT…", true)).unwrap();
        assert!(line.contains("\"action\":\"import\""));
    }
}
