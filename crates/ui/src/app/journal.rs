//! The two append-only trails: the security audit log and the query history.

use super::*;

impl DbGuiApp {
    /// Record the user's Production Guardian decision, including the risk and row evidence.
    pub(super) fn record_guard_decision(
        &mut self,
        pending: &ProductionGuardPending,
        decision: &str,
    ) -> bool {
        if cfg!(test) {
            return true;
        }
        let rows = pending.preflights.as_ref().and_then(|items| {
            items
                .iter()
                .map(|item| {
                    item.affected_rows
                        .or_else(|| item.plan.as_ref().and_then(|plan| plan.estimated_rows))
                })
                .try_fold(0u64, |sum, value| {
                    value.map(|value| sum.saturating_add(value))
                })
        });
        let target = self
            .connections
            .iter()
            .find(|config| config.id == pending.conn_id)
            .map(dbcore::ConnectionConfig::target_summary)
            .unwrap_or_default();
        let entry = dbcore::audit::AuditEntry {
            at: dbcore::history::now_rfc3339(),
            action: dbcore::audit::AuditAction::ProductionGuard,
            conn_id: pending.conn_id.clone(),
            conn_name: pending.connection_name.clone(),
            target,
            sql: pending.sql.clone(),
            ok: decision != "invalidated",
            error: None,
            details: Some(pending.audit_details(decision)),
            rows,
            elapsed_ms: 0.0,
        };
        self.handle_guard_audit_result(dbcore::audit::append(&entry))
    }

    pub(super) fn handle_guard_audit_result(&mut self, result: dbcore::Result<()>) -> bool {
        match result {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(format!(
                    "Production Guardian could not write its mandatory audit event: {error}"
                ));
                self.status_msg = "Blocked: audit trail unavailable".to_string();
                false
            }
        }
    }

    /// Append one event to the append-only audit trail (`dbcore::audit`). Separate from
    /// history: audit also records connection events, rotates monthly instead of being
    /// compacted, and has no in-app clear. Best effort — never load-bearing.
    #[allow(clippy::too_many_arguments)]
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
            details: None,
            rows,
            elapsed_ms,
        });
    }
    /// Append one executed statement to the on-disk query history and the audit trail.
    /// Best effort: history is never load-bearing, so failures are swallowed. History
    /// and audit honour their own settings toggles independently.
    #[allow(clippy::too_many_arguments)]
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
        // Keep the sidebar's History tab live while it's showing.
        if self.sidebar_tab == SidebarTab::History {
            self.history_cache.push(entry);
        }
    }
}
