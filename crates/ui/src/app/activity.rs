//! Activity monitor: the state of an Activity tab and the background queries behind it.
//! What the SQL is and how rows are read lives in [`dbcore::activity`].

use super::*;
use dbcore::activity as act;

/// How often a monitor refreshes itself. It only ticks while its tab is the one on screen, so
/// a background or closed monitor costs nothing.
pub(super) const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// Rows kept from the server's session list; far above any realistic count.
const MAX_SESSIONS: usize = 2000;

/// The state of one Activity tab.
pub(super) struct ActivityMonitor {
    pub conn_id: String,
    pub conn_name: String,
    pub kind: DbKind,
    pub read_only: bool,
    pub production: bool,
    pub sessions: Vec<act::Session>,
    pub error: Option<String>,
    /// A refresh is in flight; the next one waits for it.
    pub loading: bool,
    pub last_refresh: Option<std::time::Instant>,
    pub auto_refresh: bool,
    pub hide_idle: bool,
    pub filter: String,
    /// A stop awaiting the second click: `(session id, how)`.
    pub confirm: Option<(String, act::StopMode)>,
    /// The row last clicked, by session id so it survives refreshes.
    pub selected: Option<String>,
    /// The outcome of the last stop, shown above the list.
    pub notice: Option<std::result::Result<String, String>>,
}

impl ActivityMonitor {
    /// Sessions matching the filter box and the idle toggle.
    pub fn visible(&self) -> impl Iterator<Item = &act::Session> {
        let needle = self.filter.trim().to_lowercase();
        self.sessions.iter().filter(move |s| {
            (!self.hide_idle || s.is_active())
                && (needle.is_empty()
                    || [&s.id, &s.user, &s.database, &s.client, &s.sql]
                        .iter()
                        .any(|field| field.to_lowercase().contains(&needle)))
        })
    }

    pub fn active_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.is_active()).count()
    }

    /// Time left before the next automatic refresh, or `None` when it is off or one is in flight.
    pub fn next_refresh_in(&self) -> Option<std::time::Duration> {
        if !self.auto_refresh || self.loading {
            return None;
        }
        Some(match self.last_refresh {
            Some(at) => REFRESH_EVERY.saturating_sub(at.elapsed()),
            None => std::time::Duration::ZERO,
        })
    }
}

impl DbGuiApp {
    /// Open (or return to) the Activity tab of the saved connection `conn_idx`.
    pub(super) fn open_activity_monitor(&mut self, conn_idx: usize) {
        let Some(cfg) = self.connections.get(conn_idx).cloned() else {
            return;
        };
        if !self
            .active_connections
            .iter()
            .any(|c| c.config_id == cfg.id)
        {
            self.error = Some(format!("Connect to {} first.", cfg.name));
            return;
        }
        if act::sessions_sql(cfg.kind).is_none() {
            self.error = Some(format!(
                "{} has no other sessions to monitor.",
                cfg.kind.label()
            ));
            return;
        }
        let existing = self.tabs.iter().position(|tab| {
            tab.kind == crate::components::QueryTabKind::Activity
                && tab.conn_id.as_deref() == Some(cfg.id.as_str())
        });
        if let Some(idx) = existing {
            self.select_tab(idx);
            self.refresh_activity();
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new(id, "Activity".to_string());
        tab.kind = crate::components::QueryTabKind::Activity;
        tab.conn_id = Some(cfg.id.clone());
        tab.activity = Some(ActivityMonitor {
            conn_id: cfg.id.clone(),
            conn_name: cfg.name.clone(),
            kind: cfg.kind,
            read_only: cfg.is_read_only(),
            production: cfg.is_production(),
            sessions: Vec::new(),
            error: None,
            loading: false,
            last_refresh: None,
            auto_refresh: true,
            hide_idle: false,
            filter: String::new(),
            confirm: None,
            selected: None,
            notice: None,
        });
        self.tabs.push(tab);
        self.select_tab(self.tabs.len() - 1);
        self.refresh_activity();
    }

    fn activity_db(&self, conn_id: &str) -> Option<std::sync::Arc<dyn dbcore::database::Database>> {
        self.active_connections
            .iter()
            .find(|c| c.config_id == conn_id)
            .map(|c| c.db.clone())
    }

    /// Ask the server for the active Activity tab's session list; answered by
    /// `ActivitySessions`.
    pub(super) fn refresh_activity(&mut self) {
        let Some((tab_id, conn_id, kind, loading)) = self
            .tab()
            .activity
            .as_ref()
            .map(|m| (self.tab().id, m.conn_id.clone(), m.kind, m.loading))
        else {
            return;
        };
        let Some(sql) = act::sessions_sql(kind).filter(|_| !loading) else {
            return;
        };
        let db = self.activity_db(&conn_id);
        let Some(monitor) = self.tab_mut().activity.as_mut() else {
            return;
        };
        let Some(db) = db else {
            monitor.error = Some("The connection is closed.".into());
            monitor.auto_refresh = false;
            return;
        };
        monitor.loading = true;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = db
                .execute_capped(sql, MAX_SESSIONS)
                .await
                .map(|rows| act::parse_sessions(&rows))
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::ActivitySessions { tab_id, result });
        });
    }

    pub(super) fn apply_activity_sessions(
        &mut self,
        tab_id: u64,
        result: std::result::Result<Vec<act::Session>, String>,
    ) {
        let Some(monitor) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.activity.as_mut())
        else {
            return;
        };
        monitor.loading = false;
        monitor.last_refresh = Some(std::time::Instant::now());
        match result {
            Ok(sessions) => {
                monitor.error = None;
                monitor.sessions = sessions;
            }
            Err(error) => {
                // Keep the last good list on screen; a permissions error (SQL Server's
                // VIEW SERVER STATE) would otherwise just look like an empty server.
                monitor.error = Some(error);
                monitor.auto_refresh = false;
            }
        }
    }

    /// Cancel a session's statement or terminate it. The caller has already confirmed.
    pub(super) fn stop_session(&mut self, id: String, mode: act::StopMode) {
        let tab_id = self.tab().id;
        let Some(monitor) = self.tab_mut().activity.as_mut() else {
            return;
        };
        monitor.confirm = None;
        if monitor.read_only {
            monitor.notice = Some(Err("This connection is read-only.".into()));
            return;
        }
        let sql = match act::stop_sql(monitor.kind, &id, mode) {
            Ok(sql) => sql,
            Err(error) => {
                monitor.notice = Some(Err(error));
                return;
            }
        };
        let conn_id = monitor.conn_id.clone();
        let Some(db) = self.activity_db(&conn_id) else {
            return;
        };
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let started = std::time::Instant::now();
            let result = db
                .execute(&sql)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::SessionStopped {
                tab_id,
                conn_id,
                id,
                mode,
                sql,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                result,
            });
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_session_stop(
        &mut self,
        tab_id: u64,
        conn_id: &str,
        id: &str,
        mode: act::StopMode,
        sql: &str,
        elapsed_ms: f64,
        result: std::result::Result<(), String>,
    ) {
        self.record_audit(
            dbcore::audit::AuditAction::SessionStop,
            conn_id,
            sql,
            result.is_ok(),
            result.as_ref().err().cloned(),
            None,
            elapsed_ms,
        );
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(monitor) = self.tabs[idx].activity.as_mut() else {
            return;
        };
        monitor.notice = Some(result.map(|()| match mode {
            act::StopMode::Cancel => format!("Cancelled the query on session {id}."),
            act::StopMode::Terminate => format!("Terminated session {id}."),
        }));
        // Refresh now if this tab is on screen; otherwise its next view does it.
        if idx == self.active_query_tab {
            self.refresh_activity();
        } else if let Some(monitor) = self.tabs[idx].activity.as_mut() {
            monitor.last_refresh = None;
        }
    }
}
