//! Server activity: who is connected, what they are running, and how to stop it.
//!
//! Each backend exposes this differently (`pg_stat_activity`, `information_schema.PROCESSLIST`,
//! `sys.dm_exec_*`). The SQL below aliases every column to one fixed set of names so
//! [`parse_sessions`] stays backend-agnostic. Embedded engines (SQLite, DuckDB) have no other
//! sessions, and CQL has no equivalent, so [`sessions_sql`] is `None` for them.

use crate::model::{DbKind, QueryResult};

/// One server session as shown in the Activity monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Server-side session id (`pid` / process id / `spid`). Always a plain unsigned integer.
    pub id: String,
    pub user: String,
    pub database: String,
    /// Application name or client host, whichever the server reports.
    pub client: String,
    /// `active`, `idle`, or the backend's own state word (`idle in transaction`, `suspended`).
    pub state: String,
    /// How long the current statement has run, or how long an idle session has sat idle.
    pub seconds: f64,
    /// What it is waiting on, if the server says.
    pub waiting: String,
    pub sql: String,
}

impl Session {
    pub fn is_active(&self) -> bool {
        self.state == "active" || self.state == "running"
    }
}

/// How hard to stop a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
    /// Cancel the running statement; the connection stays open.
    Cancel,
    /// Close the whole connection, rolling back anything it had open.
    Terminate,
}

/// The query listing other sessions, or `None` when the backend has no such view.
/// The monitor's own connection is excluded.
pub fn sessions_sql(kind: DbKind) -> Option<&'static str> {
    Some(match kind {
        DbKind::Postgres => {
            "SELECT pid::text AS session_id, \
                    COALESCE(usename, '')::text AS login, \
                    COALESCE(datname, '')::text AS db_name, \
                    COALESCE(NULLIF(application_name, ''), client_addr::text, '')::text AS client, \
                    COALESCE(state, '')::text AS state, \
                    COALESCE(EXTRACT(EPOCH FROM (now() - CASE WHEN state = 'active' \
                        THEN query_start ELSE state_change END)), 0)::text AS seconds, \
                    COALESCE(wait_event_type || ': ' || wait_event, '')::text AS waiting, \
                    COALESCE(query, '')::text AS sql_text \
             FROM pg_stat_activity \
             WHERE pid <> pg_backend_pid() AND backend_type = 'client backend'"
        }
        DbKind::MySql | DbKind::MariaDb => {
            "SELECT CAST(ID AS CHAR) AS session_id, \
                    COALESCE(USER, '') AS login, \
                    COALESCE(DB, '') AS db_name, \
                    COALESCE(HOST, '') AS client, \
                    CASE COMMAND WHEN 'Sleep' THEN 'idle' WHEN 'Query' THEN 'active' \
                        ELSE LOWER(COMMAND) END AS state, \
                    CAST(TIME AS CHAR) AS seconds, \
                    COALESCE(STATE, '') AS waiting, \
                    COALESCE(INFO, '') AS sql_text \
             FROM information_schema.PROCESSLIST \
             WHERE ID <> CONNECTION_ID()"
        }
        DbKind::SqlServer => {
            "SELECT CAST(s.session_id AS varchar(12)) AS session_id, \
                    ISNULL(s.login_name, '') AS login, \
                    ISNULL(DB_NAME(s.database_id), '') AS db_name, \
                    ISNULL(s.host_name, '') AS client, \
                    CASE WHEN r.session_id IS NOT NULL THEN LOWER(r.status) \
                        ELSE 'idle' END AS state, \
                    CAST(ISNULL(r.total_elapsed_time / 1000.0, \
                        DATEDIFF(second, s.last_request_end_time, GETDATE())) \
                        AS varchar(32)) AS seconds, \
                    ISNULL(r.wait_type, '') AS waiting, \
                    ISNULL(t.text, '') AS sql_text \
             FROM sys.dm_exec_sessions s \
             LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
             OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
             WHERE s.is_user_process = 1 AND s.session_id <> @@SPID"
        }
        DbKind::Sqlite | DbKind::DuckDb | DbKind::Cassandra | DbKind::ScyllaDb => return None,
    })
}

/// Whether the backend can cancel a statement without dropping the connection.
/// SQL Server only has `KILL`, which always ends the session.
pub fn can_cancel(kind: DbKind) -> bool {
    matches!(kind, DbKind::Postgres | DbKind::MySql | DbKind::MariaDb)
}

/// The statement that stops session `id`. The id is re-parsed as an unsigned integer, so
/// nothing but digits ever reaches the SQL text.
pub fn stop_sql(kind: DbKind, id: &str, mode: StopMode) -> Result<String, String> {
    let id: u64 = id
        .trim()
        .parse()
        .map_err(|_| format!("'{id}' is not a valid session id"))?;
    match (kind, mode) {
        (DbKind::Postgres, StopMode::Cancel) => Ok(format!("SELECT pg_cancel_backend({id})")),
        (DbKind::Postgres, StopMode::Terminate) => Ok(format!("SELECT pg_terminate_backend({id})")),
        (DbKind::MySql | DbKind::MariaDb, StopMode::Cancel) => Ok(format!("KILL QUERY {id}")),
        (DbKind::MySql | DbKind::MariaDb, StopMode::Terminate) => {
            Ok(format!("KILL CONNECTION {id}"))
        }
        (DbKind::SqlServer, StopMode::Terminate) => Ok(format!("KILL {id}")),
        (DbKind::SqlServer, StopMode::Cancel) => {
            Err("SQL Server can only end the whole session, not just its query".into())
        }
        _ => Err("This database has no sessions to stop".into()),
    }
}

/// Read the rows of [`sessions_sql`] into sessions: running ones first, then longest-running.
pub fn parse_sessions(result: &QueryResult) -> Vec<Session> {
    let column = |name: &str| {
        result
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    };
    let Some(id_col) = column("session_id") else {
        return Vec::new();
    };
    let (login, db, client, state, secs, waiting, sql) = (
        column("login"),
        column("db_name"),
        column("client"),
        column("state"),
        column("seconds"),
        column("waiting"),
        column("sql_text"),
    );
    let text = |row: &[crate::Value], col: Option<usize>| {
        col.and_then(|i| row.get(i))
            .filter(|v| !v.is_null())
            .map(|v| v.display())
            .unwrap_or_default()
    };
    let mut sessions: Vec<Session> = result
        .rows
        .iter()
        .filter_map(|row| {
            let id = text(row, Some(id_col));
            // A non-numeric id could never be stopped; don't show a row we'd have to refuse.
            id.parse::<u64>().ok()?;
            Some(Session {
                id,
                user: text(row, login),
                database: text(row, db),
                client: text(row, client),
                state: text(row, state),
                seconds: text(row, secs).parse().unwrap_or(0.0),
                waiting: text(row, waiting),
                sql: text(row, sql),
            })
        })
        .collect();
    sessions.sort_by(|a, b| {
        b.is_active()
            .cmp(&a.is_active())
            .then(b.seconds.total_cmp(&a.seconds))
            .then_with(|| a.id.len().cmp(&b.id.len()).then(a.id.cmp(&b.id)))
    });
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ColumnMeta;
    use crate::Value;

    fn result(rows: Vec<Vec<&str>>) -> QueryResult {
        let names = [
            "session_id",
            "login",
            "db_name",
            "client",
            "state",
            "seconds",
            "waiting",
            "sql_text",
        ];
        QueryResult {
            columns: names
                .iter()
                .map(|n| ColumnMeta {
                    name: (*n).into(),
                    type_name: "text".into(),
                })
                .collect(),
            rows: rows
                .into_iter()
                .map(|r| r.into_iter().map(|c| Value::Text(c.into())).collect())
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn stop_sql_only_ever_embeds_digits() {
        assert_eq!(
            stop_sql(DbKind::Postgres, "42", StopMode::Terminate).unwrap(),
            "SELECT pg_terminate_backend(42)"
        );
        assert_eq!(
            stop_sql(DbKind::MariaDb, " 7 ", StopMode::Cancel).unwrap(),
            "KILL QUERY 7"
        );
        assert_eq!(
            stop_sql(DbKind::SqlServer, "55", StopMode::Terminate).unwrap(),
            "KILL 55"
        );
        for evil in ["1; DROP TABLE t", "1 OR 1=1", "-1", "", "0x1", "1.5"] {
            assert!(
                stop_sql(DbKind::Postgres, evil, StopMode::Cancel).is_err(),
                "{evil:?} must be refused"
            );
        }
    }

    #[test]
    fn unsupported_combinations_are_refused() {
        assert!(stop_sql(DbKind::SqlServer, "5", StopMode::Cancel).is_err());
        assert!(stop_sql(DbKind::Sqlite, "5", StopMode::Terminate).is_err());
        assert!(!can_cancel(DbKind::SqlServer));
        assert!(can_cancel(DbKind::Postgres));
    }

    #[test]
    fn embedded_and_cql_backends_have_no_session_list() {
        for kind in [
            DbKind::Sqlite,
            DbKind::DuckDb,
            DbKind::Cassandra,
            DbKind::ScyllaDb,
        ] {
            assert!(sessions_sql(kind).is_none());
        }
        for kind in [
            DbKind::Postgres,
            DbKind::MySql,
            DbKind::MariaDb,
            DbKind::SqlServer,
        ] {
            assert!(sessions_sql(kind).is_some());
        }
    }

    #[test]
    fn running_sessions_sort_first_then_by_duration() {
        let sessions = parse_sessions(&result(vec![
            vec!["1", "a", "d", "c", "idle", "900", "", ""],
            vec!["2", "b", "d", "c", "active", "3", "", "SELECT 1"],
            vec![
                "3",
                "c",
                "d",
                "c",
                "active",
                "40.5",
                "Lock: relation",
                "SELECT 2",
            ],
        ]));
        let ids: Vec<_> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["3", "2", "1"]);
        assert_eq!(sessions[0].waiting, "Lock: relation");
    }

    #[test]
    fn rows_with_unusable_ids_are_dropped_and_nulls_become_empty() {
        let mut r = result(vec![
            vec!["x; DROP", "a", "d", "c", "idle", "1", "", ""],
            vec!["9", "a", "d", "c", "idle", "oops", "", ""],
        ]);
        r.rows[1][7] = Value::Null;
        let sessions = parse_sessions(&r);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].sql, "");
        assert_eq!(sessions[0].seconds, 0.0);
    }
}
