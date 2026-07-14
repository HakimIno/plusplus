//! The two append-only trails: the security audit log and the query history.

use super::*;

impl DbGuiApp {
    /// Append one event to the append-only audit trail (`dbcore::audit`). Separate from
    /// history: audit also records connection events, rotates monthly instead of being
    /// compacted, and has no in-app clear. Best effort — never load-bearing.
    pub(super) fn record_audit(
        &self,
        action: dbcore::audit::AuditAction,
        conn_id: &str,
        sql: &str,
        ok: bool,
        error: Option<String>,
        rows: Option<u64>,
        elapsed_ms: f64,
    ) {
        if !self.audit_enabled || cfg!(test) {
            return;
        }
        let (conn_name, target) = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .map(|c| (c.name.clone(), c.target_summary()))
            .unwrap_or_default();
        let _ = dbcore::audit::append(&dbcore::audit::AuditEntry {
            at: dbcore::history::now_rfc3339(),
            action,
            conn_id: conn_id.to_string(),
            conn_name,
            target,
            sql: sql.to_string(),
            ok,
            error,
            rows,
            elapsed_ms,
        });
    }
    /// Append one executed statement to the on-disk query history and the audit trail.
    /// Best effort: history is never load-bearing, so failures are swallowed. History
    /// and audit honour their own settings toggles independently.
    pub(super) fn record_history(
        &mut self,
        action: dbcore::audit::AuditAction,
        conn_id: &str,
        sql: &str,
        ok: bool,
        error: Option<String>,
        rows: Option<u64>,
        elapsed_ms: f64,
    ) {
        self.record_audit(action, conn_id, sql, ok, error.clone(), rows, elapsed_ms);
        // Feed the editor's ghost-text pool with statements that ran cleanly (independent
        // of the history-logging setting, which only governs the on-disk audit log). Skip
        // under test so the pool stays deterministic.
        if ok && !cfg!(test) {
            self.suggest_pool.push(PooledQuery {
                conn_id: conn_id.to_string(),
                sql: sql.to_string(),
            });
            if self.suggest_pool.len() > dbcore::history::MAX_ENTRIES {
                self.suggest_pool.remove(0);
            }
        }
        if !self.history_enabled {
            return;
        }
        // Unit tests construct real apps and pump real messages; never let them write
        // into the user's actual history file.
        if cfg!(test) {
            return;
        }
        let conn_name = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let entry = dbcore::history::HistoryEntry {
            at: dbcore::history::now_rfc3339(),
            conn_id: conn_id.to_string(),
            conn_name,
            sql: sql.to_string(),
            ok,
            error,
            rows,
            elapsed_ms,
        };
        let _ = dbcore::history::append(&entry);
        // Keep an open History dialog live.
        if self.history_open {
            self.history_cache.push(entry);
        }
    }
}
