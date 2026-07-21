//! MySQL/MariaDB backend implemented on top of `sqlx`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Executor, Row, TypeInfo, ValueRef};
use tokio_util::sync::CancellationToken;

use crate::database::{returns_rows, split_statements, Database};
use crate::error::{CoreError, Result};
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, ForeignKeyInfo, IndexInfo, ParamMode,
    QueryResult, QueryStats, RoutineInfo, RoutineKind, RoutineParam, SchemaTree, SslMode,
    TableInfo, TriggerEvent, TriggerInfo, TriggerLevel, TriggerTiming, ViewInfo,
};
use crate::value::Value;

pub struct MySqlDb {
    pool: MySqlPool,
    kind: DbKind,
}

impl MySqlDb {
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> Result<Self> {
        use sqlx::mysql::MySqlSslMode;
        let ssl_mode = match cfg.ssl_mode {
            SslMode::Disable => MySqlSslMode::Disabled,
            SslMode::Prefer => MySqlSslMode::Preferred,
            SslMode::Require => MySqlSslMode::Required,
            SslMode::VerifyCa => MySqlSslMode::VerifyCa,
            SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
        };
        let mut opts = sqlx::mysql::MySqlConnectOptions::new()
            .host(&cfg.host)
            .port(cfg.port)
            .username(&cfg.user)
            .database(&cfg.database)
            .ssl_mode(ssl_mode);
        if !cfg.ssl_ca_cert.trim().is_empty() {
            opts = opts.ssl_ca(cfg.ssl_ca_cert.trim());
        }
        if !cfg.ssl_client_cert.trim().is_empty() {
            opts = opts.ssl_client_cert(cfg.ssl_client_cert.trim());
        }
        if !cfg.ssl_client_key.trim().is_empty() {
            opts = opts.ssl_client_key(cfg.ssl_client_key.trim());
        }
        if let Some(pw) = password {
            opts = opts.password(&pw);
        }
        opts = opts.disable_statement_logging();
        // Pool tuning that makes every query snappier:
        // - test_before_acquire(false): skip sqlx's default liveness PING before each
        //   checkout (an extra round trip on every query). idle_timeout (sqlx's 10 min
        //   default) reaps connections well before MySQL's wait_timeout closes them, so a
        //   pooled connection won't have gone stale server-side by the time we hand it out.
        // - min_connections(1): keep one connection warm so the first query after connecting
        //   (or after an idle gap) doesn't pay a fresh TCP + TLS + auth handshake.
        // - acquire_timeout(8s): fail a bad host/port fast instead of hanging on sqlx's 30s
        //   default (there is no separate connect timeout in sqlx-mysql).
        let mut pool_opts = MySqlPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .test_before_acquire(false)
            .acquire_timeout(Duration::from_secs(8));
        // Read-only connections pin the session's default transaction access mode, so
        // writes are rejected by the server itself — not just by the UI's lexical guard.
        // Applied per pooled connection as it is created.
        if cfg.read_only {
            pool_opts = pool_opts.after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute("SET SESSION TRANSACTION READ ONLY").await?;
                    Ok(())
                })
            });
        }
        let pool = pool_opts.connect_with(opts).await?;
        Ok(Self {
            pool,
            kind: cfg.kind,
        })
    }

    /// `KILL QUERY <id>` aborts the statement running on connection `id` while leaving the
    /// connection itself alive, issued from a fresh pooled connection. Best-effort.
    async fn kill_query(&self, conn_id: u64) {
        let _ = sqlx::query(AssertSqlSafe(format!("KILL QUERY {conn_id}")))
            .execute(&self.pool)
            .await;
    }

    /// Run the statements of a multi-statement batch sequentially on one connection.
    ///
    /// `sqlx` does not enable `CLIENT_MULTI_STATEMENTS`, so sending the raw batch to the
    /// server fails with a syntax error at the first `;` even when every statement is valid.
    /// Statements run in autocommit order: the grid shows the last row-returning statement's
    /// result (capped at `max_rows`), `rows_affected` sums the DML statements, and a failure
    /// stops the batch — reported with its 1-based statement number, keeping what already ran.
    async fn run_batch(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::MySql>,
        conn_id: u64,
        statements: &[&str],
        max_rows: usize,
        cancel: &CancellationToken,
    ) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        let start = Instant::now();
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut truncated = false;
        let mut affected: u64 = 0;
        let mut saw_dml = false;
        for (pos, stmt) in statements.iter().enumerate() {
            let fail = |e: CoreError| match e {
                CoreError::Canceled => CoreError::Canceled,
                other => CoreError::Statement(pos + 1, Box::new(other)),
            };
            if returns_rows(stmt) {
                let mut stmt_columns: Vec<ColumnMeta> = Vec::new();
                let mut types: Vec<String> = Vec::new();
                let mut stmt_rows: Vec<Vec<Value>> = Vec::new();
                let mut stmt_truncated = false;
                let mut stream = (&mut **conn).fetch(AssertSqlSafe(stmt.to_string()));
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            drop(stream);
                            self.kill_query(conn_id).await;
                            return Err(CoreError::Canceled);
                        }
                        row = stream.try_next() => {
                            let Some(row) = row.map_err(|e| fail(e.into()))? else { break };
                            if stmt_columns.is_empty() {
                                stmt_columns = column_meta(&row);
                                types = stmt_columns
                                    .iter()
                                    .map(|c| c.type_name.to_ascii_uppercase())
                                    .collect();
                            }
                            if stmt_rows.len() >= max_rows {
                                stmt_truncated = true;
                                break;
                            }
                            stmt_rows.push(
                                (0..stmt_columns.len()).map(|i| decode(&row, i, &types[i])).collect(),
                            );
                        }
                    }
                }
                columns = stmt_columns;
                rows = stmt_rows;
                truncated = stmt_truncated;
            } else {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        self.kill_query(conn_id).await;
                        return Err(CoreError::Canceled);
                    }
                    res = (&mut **conn).execute(AssertSqlSafe(stmt.to_string())) => {
                        affected += res.map_err(|e| fail(e.into()))?.rows_affected();
                        saw_dml = true;
                    }
                }
            }
        }
        Ok(QueryResult {
            columns,
            rows,
            stats: QueryStats {
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                rows_affected: saw_dml.then_some(affected),
            },
            truncated,
        })
    }
}

#[async_trait]
impl Database for MySqlDb {
    fn kind(&self) -> DbKind {
        self.kind
    }

    async fn introspect_overview(&self) -> Result<SchemaTree> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT DATABASE(), TABLE_NAME, TABLE_TYPE \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY TABLE_NAME",
        )
        .fetch_all(&self.pool)
        .await?;
        let database_name = rows.first().map(|r| r.0.clone()).unwrap_or_default();
        let mut tables = Vec::new();
        let mut views = Vec::new();
        for (_, name, ty) in rows {
            if ty.eq_ignore_ascii_case("VIEW") {
                views.push(ViewInfo {
                    schema: None,
                    name,
                    columns: Vec::new(),
                    definition: String::new(),
                    materialized: false,
                });
            } else {
                tables.push(TableInfo {
                    schema: None,
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                });
            }
        }
        Ok(SchemaTree {
            database_name,
            tables,
            views,
            routines: Vec::new(),
            triggers: Vec::new(),
        })
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let database_name: String = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(&self.pool)
            .await?;

        // Bucket base tables and views by name; columns below populate both.
        let mut tables: BTreeMap<String, TableInfo> = BTreeMap::new();
        let mut views: BTreeMap<String, ViewInfo> = BTreeMap::new();
        let obj_rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT TABLE_NAME, TABLE_TYPE \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY TABLE_NAME",
        )
        .fetch_all(&self.pool)
        .await?;

        for (name, ty) in obj_rows {
            if ty.eq_ignore_ascii_case("VIEW") {
                views.insert(
                    name.clone(),
                    ViewInfo {
                        schema: None,
                        name,
                        columns: Vec::new(),
                        definition: String::new(),
                        materialized: false,
                    },
                );
            } else {
                tables.insert(
                    name.clone(),
                    TableInfo {
                        schema: None,
                        name,
                        columns: Vec::new(),
                        indexes: Vec::new(),
                        foreign_keys: Vec::new(),
                    },
                );
            }
        }

        let col_rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, ORDINAL_POSITION",
        )
        .fetch_all(&self.pool)
        .await?;

        for (table, column, data_type, nullable, key) in col_rows {
            let col = ColumnInfo {
                name: column,
                data_type,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                primary_key: key.eq_ignore_ascii_case("PRI"),
            };
            if let Some(info) = tables.get_mut(&table) {
                info.columns.push(col);
            } else if let Some(view) = views.get_mut(&table) {
                view.columns.push(col);
            }
        }

        // CAST the numeric columns to SIGNED so the wire type is deterministic: across
        // MySQL/MariaDB versions information_schema reports these as INT/BIGINT UNSIGNED,
        // and sqlx's typed decoding is strict (an UNSIGNED column won't decode into a
        // signed Rust integer). Casting sidesteps version-specific type guessing.
        let idx_rows: Vec<(String, String, i64, String, i64)> = sqlx::query_as(
            "SELECT TABLE_NAME, INDEX_NAME, CAST(NON_UNIQUE AS SIGNED), \
                    COLUMN_NAME, CAST(SEQ_IN_INDEX AS SIGNED) \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut grouped: BTreeMap<(String, String), (bool, Vec<(i64, String)>)> = BTreeMap::new();
        for (table, index, non_unique, column, seq) in idx_rows {
            let entry = grouped
                .entry((table, index))
                .or_insert_with(|| (non_unique == 0, Vec::new()));
            entry.1.push((seq, column));
        }

        for ((table, name), (unique, mut columns)) in grouped {
            if let Some(info) = tables.get_mut(&table) {
                columns.sort_by_key(|(seq, _)| *seq);
                info.indexes.push(IndexInfo {
                    name,
                    unique,
                    columns: columns.into_iter().map(|(_, column)| column).collect(),
                });
            }
        }

        // Foreign keys: KEY_COLUMN_USAGE carries the column pairs, REFERENTIAL_CONSTRAINTS
        // the delete/update rules.
        type FkRow = (String, String, String, String, String, String, String);
        let fk_rows: Vec<FkRow> = sqlx::query_as(
            "SELECT kcu.TABLE_NAME, kcu.CONSTRAINT_NAME, kcu.COLUMN_NAME, \
                    kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME, \
                    rc.DELETE_RULE, rc.UPDATE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE kcu \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS rc \
               ON rc.CONSTRAINT_SCHEMA = kcu.CONSTRAINT_SCHEMA \
              AND rc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME \
              AND rc.TABLE_NAME = kcu.TABLE_NAME \
             WHERE kcu.TABLE_SCHEMA = DATABASE() \
               AND kcu.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY kcu.TABLE_NAME, kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
        )
        .fetch_all(&self.pool)
        .await?;
        for (table, constraint, column, ref_table, ref_column, del, upd) in fk_rows {
            if let Some(info) = tables.get_mut(&table) {
                match info
                    .foreign_keys
                    .iter_mut()
                    .find(|fk| fk.name == constraint)
                {
                    Some(fk) => {
                        fk.columns.push(column);
                        fk.ref_columns.push(ref_column);
                    }
                    None => info.foreign_keys.push(ForeignKeyInfo {
                        name: constraint,
                        columns: vec![column],
                        ref_schema: None,
                        ref_table,
                        ref_columns: vec![ref_column],
                        on_delete: del,
                        on_update: upd,
                    }),
                }
            }
        }

        // These optional metadata groups do not depend on one another. Fetch them together so
        // a remote connection pays one latency window instead of four sequential round trips.
        type RoutineRow = (String, String, Option<String>, Option<String>);
        type ParamRow = (String, Option<String>, String, Option<String>, i64);
        type TriggerRow = (String, String, String, String, String);
        let (view_defs, routine_rows, param_rows, trig_rows): (
            Vec<(String, String)>,
            Vec<RoutineRow>,
            Vec<ParamRow>,
            Vec<TriggerRow>,
        ) = tokio::join!(
            async {
                sqlx::query_as(
                    "SELECT TABLE_NAME, VIEW_DEFINITION FROM information_schema.VIEWS \
                     WHERE TABLE_SCHEMA = DATABASE()",
                )
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default()
            },
            async {
                sqlx::query_as(
                    "SELECT ROUTINE_NAME, ROUTINE_TYPE, DTD_IDENTIFIER, ROUTINE_DEFINITION \
                     FROM information_schema.ROUTINES \
                     WHERE ROUTINE_SCHEMA = DATABASE() \
                     ORDER BY ROUTINE_NAME",
                )
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default()
            },
            async {
                sqlx::query_as(
                    "SELECT SPECIFIC_NAME, PARAMETER_NAME, DTD_IDENTIFIER, PARAMETER_MODE, \
                            ORDINAL_POSITION \
                     FROM information_schema.PARAMETERS \
                     WHERE SPECIFIC_SCHEMA = DATABASE() \
                     ORDER BY SPECIFIC_NAME, ORDINAL_POSITION",
                )
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default()
            },
            async {
                sqlx::query_as(
                    "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, EVENT_OBJECT_TABLE, \
                            ACTION_STATEMENT \
                     FROM information_schema.TRIGGERS \
                     WHERE TRIGGER_SCHEMA = DATABASE() \
                     ORDER BY TRIGGER_NAME",
                )
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default()
            },
        );

        // View definitions. Tolerate failure (a restricted role may be denied) — an empty
        // body just means the editor opens blank, never that introspection fails.
        for (name, def) in view_defs {
            if let Some(view) = views.get_mut(&name) {
                view.definition = def;
            }
        }

        // Routines (functions + procedures), keyed by name; parameters attached below.
        let mut routines: BTreeMap<String, RoutineInfo> = BTreeMap::new();
        for (name, rtype, ret, body) in routine_rows {
            let kind = if rtype.eq_ignore_ascii_case("FUNCTION") {
                RoutineKind::Function
            } else {
                RoutineKind::Procedure
            };
            routines.insert(
                name.clone(),
                RoutineInfo {
                    schema: None,
                    name,
                    kind,
                    params: Vec::new(),
                    return_type: (kind == RoutineKind::Function).then_some(ret).flatten(),
                    language: String::new(),
                    body: body.unwrap_or_default(),
                },
            );
        }
        // Parameters: ORDINAL_POSITION 0 is a function's RETURN slot (NULL name/mode) — skip it.
        for (spec, pname, dtype, mode, ordinal) in param_rows {
            if ordinal == 0 {
                continue;
            }
            if let Some(routine) = routines.get_mut(&spec) {
                routine.params.push(RoutineParam {
                    name: pname.unwrap_or_default(),
                    data_type: dtype,
                    mode: ParamMode::from_keyword(mode.as_deref().unwrap_or("IN")),
                    default: None,
                });
            }
        }

        // Triggers: MySQL fires one event per trigger, always row-level, with no WHEN guard.
        let mut triggers = Vec::new();
        for (name, timing, event, table, body) in trig_rows {
            triggers.push(TriggerInfo {
                schema: None,
                name,
                table,
                timing: TriggerTiming::from_keyword(&timing).unwrap_or(TriggerTiming::Before),
                events: TriggerEvent::from_keyword(&event).into_iter().collect(),
                level: TriggerLevel::Row,
                when_condition: None,
                action: body,
            });
        }

        Ok(SchemaTree {
            database_name,
            tables: tables.into_values().collect(),
            views: views.into_values().collect(),
            routines: routines.into_values().collect(),
            triggers,
        })
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        // Multi-statement batches must run statement by statement (see `run_batch`). The
        // cancellation token is never fired here, so `conn_id` is only a placeholder.
        let statements = split_statements(sql);
        if statements.len() > 1 {
            let mut conn = self.pool.acquire().await?;
            return self
                .run_batch(
                    &mut conn,
                    0,
                    &statements,
                    max_rows,
                    &CancellationToken::new(),
                )
                .await;
        }
        let start = Instant::now();
        if returns_rows(sql) {
            // Stream rows instead of fetch_all: a SELECT over a huge table materializes at
            // most `max_rows` rows; dropping the stream early cancels the rest of the fetch.
            // `Executor::fetch` with an `AssertSqlSafe` string (no bind arguments) uses MySQL's
            // simple/text protocol — one round trip — instead of prepare + execute. Ad-hoc GUI
            // queries are almost never repeated, so preparing them just adds a round trip and
            // churns the statement cache.
            let mut stream = self.pool.fetch(AssertSqlSafe(sql.to_string()));
            let mut columns: Vec<ColumnMeta> = Vec::new();
            // Upper-cased type names resolved once per result; `decode` dispatches on
            // these instead of re-uppercasing the type name for every cell.
            let mut types: Vec<String> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if columns.is_empty() {
                    columns = column_meta(&row);
                    types = columns
                        .iter()
                        .map(|c| c.type_name.to_ascii_uppercase())
                        .collect();
                }
                if data.len() >= max_rows {
                    truncated = true;
                    break;
                }
                data.push(
                    (0..columns.len())
                        .map(|i| decode(&row, i, &types[i]))
                        .collect(),
                );
            }
            Ok(QueryResult {
                columns,
                rows: data,
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: None,
                },
                truncated,
            })
        } else {
            let res = self
                .pool
                .execute(AssertSqlSafe(sql.to_string()))
                .await?;
            Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: Some(res.rows_affected()),
                },
                truncated: false,
            })
        }
    }

    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: CancellationToken,
    ) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        let start = Instant::now();
        // Dedicated connection so we can target its thread id with KILL QUERY; the kill must
        // come from a different connection while this one is busy streaming.
        let mut conn = self.pool.acquire().await?;
        let conn_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
            .fetch_one(&mut *conn)
            .await?;

        // The driver can't send a multi-statement batch in one round trip; run it
        // statement by statement instead of surfacing a misleading syntax error.
        let statements = split_statements(sql);
        if statements.len() > 1 {
            return self
                .run_batch(&mut conn, conn_id, &statements, max_rows, &cancel)
                .await;
        }

        if returns_rows(sql) {
            let mut stream = (&mut *conn).fetch(AssertSqlSafe(sql.to_string()));
            let mut columns: Vec<ColumnMeta> = Vec::new();
            let mut types: Vec<String> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        drop(stream);
                        self.kill_query(conn_id).await;
                        return Err(CoreError::Canceled);
                    }
                    row = stream.try_next() => {
                        let Some(row) = row? else { break };
                        if columns.is_empty() {
                            columns = column_meta(&row);
                            types = columns
                                .iter()
                                .map(|c| c.type_name.to_ascii_uppercase())
                                .collect();
                        }
                        if data.len() >= max_rows {
                            truncated = true;
                            break;
                        }
                        data.push(
                            (0..columns.len()).map(|i| decode(&row, i, &types[i])).collect(),
                        );
                    }
                }
            }
            Ok(QueryResult {
                columns,
                rows: data,
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: None,
                },
                truncated,
            })
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.kill_query(conn_id).await;
                    Err(CoreError::Canceled)
                }
                res = (&mut *conn).execute(AssertSqlSafe(sql.to_string())) => {
                    let res = res?;
                    Ok(QueryResult {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        stats: QueryStats {
                            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                            rows_affected: Some(res.rows_affected()),
                        },
                        truncated: false,
                    })
                }
            }
        }
    }

    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        use futures_util::TryStreamExt;
        // Stream straight into the sink: rows are written to the file one at a time and never
        // collected, so the whole (possibly huge) table never sits in memory. `Executor::fetch`
        // (no bind arguments) runs it on the simple/text protocol — no prepare round trip.
        let mut stream = self.pool.fetch(AssertSqlSafe(sql.to_string()));
        let mut types: Vec<String> = Vec::new();
        let mut began = false;
        let mut count = 0u64;
        while let Some(row) = stream.try_next().await? {
            if !began {
                let columns = column_meta(&row);
                types = columns
                    .iter()
                    .map(|c| c.type_name.to_ascii_uppercase())
                    .collect();
                sink.begin(&columns)?;
                began = true;
            }
            let values: Vec<Value> = (0..types.len())
                .map(|i| decode(&row, i, &types[i]))
                .collect();
            sink.write_row(&values)?;
            count += 1;
        }
        sink.finish()?;
        Ok(count)
    }

    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        if stmts.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        for stmt in stmts {
            (&mut *tx).execute(AssertSqlSafe(stmt.as_str())).await?;
        }
        tx.commit().await?;
        Ok(stmts.len())
    }

    async fn list_databases(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SHOW DATABASES")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }
}

fn column_meta(row: &MySqlRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

/// Decode one MySQL cell, dispatching on the column's upper-cased type name (resolved
/// once per result by the caller).
fn decode(row: &MySqlRow, idx: usize, name: &str) -> Value {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
    // MySQL reports unsigned/zerofill integers with a suffix (e.g. "INT UNSIGNED",
    // "BIGINT UNSIGNED ZEROFILL"). Strip those modifiers so the base type drives dispatch;
    // the integer arms below already fall back to the unsigned Rust type when decoding.
    let name = name
        .trim_end_matches(" ZEROFILL")
        .trim_end_matches(" UNSIGNED");
    match name {
        "BOOLEAN" | "BOOL" => row
            .try_get::<bool, _>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "TINYINT" => row
            .try_get::<i8, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u8, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "SMALLINT" => row
            .try_get::<i16, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u16, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "MEDIUMINT" | "INT" | "INTEGER" => row
            .try_get::<i32, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u32, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "BIGINT" => row
            .try_get::<i64, _>(idx)
            .map(Value::Int)
            .or_else(|_| {
                row.try_get::<u64, _>(idx)
                    .map(|v| Value::Text(v.to_string()))
            })
            .unwrap_or(Value::Null),
        "FLOAT" => row
            .try_get::<f32, _>(idx)
            .map(|v| Value::Float(v as f64))
            .unwrap_or(Value::Null),
        "DOUBLE" | "REAL" => row
            .try_get::<f64, _>(idx)
            .map(Value::Float)
            .unwrap_or(Value::Null),
        "DECIMAL" | "NEWDECIMAL" => row
            .try_get::<sqlx::types::BigDecimal, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM" | "SET"
        | "JSON" => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "DATETIME" | "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => row
            .try_get::<Vec<u8>, _>(idx)
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        _ => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .or_else(|_| row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes))
            .unwrap_or_else(|_| Value::Text(format!("<{}>", name.to_lowercase()))),
    }
}
