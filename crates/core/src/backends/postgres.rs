//! PostgreSQL backend implemented on top of `sqlx`.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Row, TypeInfo, ValueRef};
use tokio_util::sync::CancellationToken;

use crate::database::{returns_rows, Database};
use crate::error::{CoreError, Result};
use crate::model::{
    parse_trigger_header, ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, ForeignKeyInfo,
    IndexInfo, ParamMode, QueryResult, QueryStats, RoutineInfo, RoutineKind, RoutineParam,
    SchemaTree, SslMode, TableInfo, TriggerInfo, ViewInfo,
};
use crate::value::Value;

/// Schemas that are part of the server's machinery and not interesting to browse.
const SYSTEM_SCHEMAS: &str = "('pg_catalog', 'information_schema')";

pub struct PostgresDb {
    pool: PgPool,
}

impl PostgresDb {
    /// Open a pooled connection using the non-secret config plus an optional password
    /// (fetched from the keychain by the caller).
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> Result<Self> {
        use sqlx::postgres::PgSslMode;
        let ssl_mode = match cfg.ssl_mode {
            SslMode::Disable => PgSslMode::Disable,
            SslMode::Prefer => PgSslMode::Prefer,
            SslMode::Require => PgSslMode::Require,
            SslMode::VerifyCa => PgSslMode::VerifyCa,
            SslMode::VerifyFull => PgSslMode::VerifyFull,
        };
        let mut opts = sqlx::postgres::PgConnectOptions::new()
            .host(&cfg.host)
            .port(cfg.port)
            .username(&cfg.user)
            .database(&cfg.database)
            .ssl_mode(ssl_mode);
        if !cfg.ssl_ca_cert.trim().is_empty() {
            opts = opts.ssl_root_cert(cfg.ssl_ca_cert.trim());
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
        // Read-only connections pin every transaction read-only at the session level, so
        // even a write the lexical guard can't see (a side-effecting function, setval, …)
        // is rejected by the server itself.
        if cfg.read_only {
            opts = opts.options([("default_transaction_read_only", "on")]);
        }
        // Quieten sqlx's statement logging; the UI surfaces errors itself.
        opts = opts.disable_statement_logging();
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    /// Ask the server to cancel the query running on backend `pid`, from a fresh pooled
    /// connection. Best-effort: a failure here just means the cancel didn't land (the user
    /// can retry), so the error is swallowed.
    async fn cancel_backend(&self, pid: i32) {
        let _ = sqlx::query("SELECT pg_cancel_backend($1)")
            .bind(pid)
            .execute(&self.pool)
            .await;
    }
}

#[async_trait]
impl Database for PostgresDb {
    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let database_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&self.pool)
            .await?;

        // Build tables keyed by (schema, name) so we can attach columns/indexes by lookup.
        let mut tables: BTreeMap<(String, String), TableInfo> = BTreeMap::new();

        let table_rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name FROM information_schema.tables \
             WHERE table_type = 'BASE TABLE' AND table_schema NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY table_schema, table_name"
        )))
        .fetch_all(&self.pool)
        .await?;
        for (schema, name) in table_rows {
            tables.insert(
                (schema.clone(), name.clone()),
                TableInfo {
                    schema: Some(schema),
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                },
            );
        }

        // Views (regular + materialized). Materialized views are Postgres-specific and live
        // in pg_matviews, not information_schema. Regular views get their columns from the
        // column sweep below; both carry their defining SELECT. `unwrap_or_default` keeps a
        // privilege error from failing the whole introspection.
        let mut views: BTreeMap<(String, String), ViewInfo> = BTreeMap::new();
        let view_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(AssertSqlSafe(
            format!(
                "SELECT table_schema, table_name, view_definition FROM information_schema.views \
                 WHERE table_schema NOT IN {SYSTEM_SCHEMAS} \
                 ORDER BY table_schema, table_name"
            ),
        ))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        for (schema, name, def) in view_rows {
            views.insert(
                (schema.clone(), name.clone()),
                ViewInfo {
                    schema: Some(schema),
                    name,
                    columns: Vec::new(),
                    definition: def.unwrap_or_default().trim().to_string(),
                    materialized: false,
                },
            );
        }
        let matview_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(AssertSqlSafe(
            format!(
                "SELECT schemaname, matviewname, definition FROM pg_matviews \
                 WHERE schemaname NOT IN {SYSTEM_SCHEMAS} \
                 ORDER BY schemaname, matviewname"
            ),
        ))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        for (schema, name, def) in matview_rows {
            views.insert(
                (schema.clone(), name.clone()),
                ViewInfo {
                    schema: Some(schema),
                    name,
                    columns: Vec::new(),
                    definition: def.unwrap_or_default().trim().to_string(),
                    materialized: true,
                },
            );
        }

        // Columns (ordered by ordinal position).
        let col_rows: Vec<(String, String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name, column_name, data_type FROM information_schema.columns \
             WHERE table_schema NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY table_schema, table_name, ordinal_position"
        )))
        .fetch_all(&self.pool)
        .await?;

        // Nullability is kept separate to keep the tuple small; query both at once instead.
        let null_rows: Vec<(String, String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name, column_name, is_nullable FROM information_schema.columns \
             WHERE table_schema NOT IN {SYSTEM_SCHEMAS}"
        )))
        .fetch_all(&self.pool)
        .await?;
        let mut nullable: BTreeMap<(String, String, String), bool> = BTreeMap::new();
        for (s, t, c, n) in null_rows {
            nullable.insert((s, t, c), n.eq_ignore_ascii_case("YES"));
        }

        // Primary-key columns.
        let pk_rows: Vec<(String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT tc.table_schema, tc.table_name, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             WHERE tc.constraint_type = 'PRIMARY KEY' \
               AND tc.table_schema NOT IN {SYSTEM_SCHEMAS}"
        )))
        .fetch_all(&self.pool)
        .await?;
        let mut pk_set: BTreeMap<(String, String, String), ()> = BTreeMap::new();
        for (s, t, c) in pk_rows {
            pk_set.insert((s, t, c), ());
        }

        for (schema, table, column, data_type) in col_rows {
            let key = (schema.clone(), table.clone(), column.clone());
            let col = ColumnInfo {
                name: column,
                data_type,
                nullable: nullable.get(&key).copied().unwrap_or(true),
                primary_key: pk_set.contains_key(&key),
            };
            if let Some(info) = tables.get_mut(&(schema.clone(), table.clone())) {
                info.columns.push(col);
            } else if let Some(view) = views.get_mut(&(schema, table)) {
                view.columns.push(col);
            }
        }

        // Indexes (parsed best-effort from pg_indexes definitions).
        let idx_rows: Vec<(String, String, String, String)> =
            sqlx::query_as(AssertSqlSafe(format!(
                "SELECT schemaname, tablename, indexname, indexdef FROM pg_indexes \
             WHERE schemaname NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY schemaname, tablename, indexname"
            )))
            .fetch_all(&self.pool)
            .await?;
        for (schema, table, indexname, indexdef) in idx_rows {
            if let Some(info) = tables.get_mut(&(schema, table)) {
                info.indexes.push(IndexInfo {
                    name: indexname,
                    unique: indexdef.to_ascii_uppercase().contains("UNIQUE INDEX"),
                    columns: parse_index_columns(&indexdef),
                });
            }
        }

        // Foreign keys, from pg_catalog rather than information_schema: the standard views
        // hide a referential constraint unless the user holds non-SELECT privileges on
        // *both* of its tables (a plain read-only role sees none of them), and their
        // (schema, constraint-name) join breaks when two tables reuse a constraint name.
        // pg_constraint is visible to everyone and precise per constraint OID; unnesting
        // conkey/confkey together keeps composite-key column pairs aligned.
        const ACTION_CASE: &str = "WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' \
             WHEN 'd' THEN 'SET DEFAULT' WHEN 'r' THEN 'RESTRICT' ELSE 'NO ACTION'";
        type FkRow = (String, String, String, String, String, String, String, String, String);
        let fk_rows: Vec<FkRow> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT sch.nspname, tbl.relname, con.conname, \
                    fsch.nspname, ftbl.relname, \
                    att.attname, fatt.attname, \
                    CASE con.confdeltype {ACTION_CASE} END, \
                    CASE con.confupdtype {ACTION_CASE} END \
             FROM pg_constraint con \
             JOIN pg_class tbl ON tbl.oid = con.conrelid \
             JOIN pg_namespace sch ON sch.oid = tbl.relnamespace \
             JOIN pg_class ftbl ON ftbl.oid = con.confrelid \
             JOIN pg_namespace fsch ON fsch.oid = ftbl.relnamespace \
             CROSS JOIN LATERAL unnest(con.conkey, con.confkey) \
                  WITH ORDINALITY AS cols(attnum, fattnum, ord) \
             JOIN pg_attribute att \
               ON att.attrelid = con.conrelid AND att.attnum = cols.attnum \
             JOIN pg_attribute fatt \
               ON fatt.attrelid = con.confrelid AND fatt.attnum = cols.fattnum \
             WHERE con.contype = 'f' AND sch.nspname NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY sch.nspname, tbl.relname, con.conname, cols.ord"
        )))
        .fetch_all(&self.pool)
        .await?;
        for (schema, table, constraint, column, ref_schema, ref_table, ref_column, del, upd) in
            fk_rows
        {
            if let Some(info) = tables.get_mut(&(schema, table)) {
                match info.foreign_keys.iter_mut().find(|fk| fk.name == constraint) {
                    Some(fk) => {
                        fk.columns.push(column);
                        fk.ref_columns.push(ref_column);
                    }
                    None => info.foreign_keys.push(ForeignKeyInfo {
                        name: constraint,
                        columns: vec![column],
                        ref_schema: Some(ref_schema),
                        ref_table,
                        ref_columns: vec![ref_column],
                        on_delete: del,
                        on_update: upd,
                    }),
                }
            }
        }

        // Routines: functions ('f') and procedures ('p'); aggregates/window funcs excluded.
        // `pg_get_functiondef` yields the full `CREATE` text for display; arguments and the
        // return clause come pre-rendered and are parsed into structured params.
        let mut routines = Vec::new();
        type RoutineRow =
            (String, String, String, Option<String>, Option<String>, String, Option<String>);
        let routine_rows: Vec<RoutineRow> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT n.nspname, p.proname, p.prokind::text, \
                    pg_get_function_arguments(p.oid), pg_get_function_result(p.oid), \
                    l.lanname, pg_get_functiondef(p.oid) \
             FROM pg_proc p \
             JOIN pg_namespace n ON n.oid = p.pronamespace \
             JOIN pg_language l ON l.oid = p.prolang \
             WHERE n.nspname NOT IN {SYSTEM_SCHEMAS} AND p.prokind IN ('f', 'p') \
             ORDER BY n.nspname, p.proname"
        )))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        for (schema, name, prokind, args, result, lang, def) in routine_rows {
            let kind = if prokind == "p" {
                RoutineKind::Procedure
            } else {
                RoutineKind::Function
            };
            routines.push(RoutineInfo {
                schema: Some(schema),
                name,
                kind,
                params: parse_pg_args(args.as_deref().unwrap_or_default()),
                return_type: (kind == RoutineKind::Function).then_some(result).flatten(),
                language: lang,
                body: def.unwrap_or_default(),
            });
        }

        // Triggers: skip internal (FK-enforcement) triggers; parse the rendered def into
        // structured fields, keeping the full `CREATE TRIGGER … EXECUTE FUNCTION …` in `action`.
        let mut triggers = Vec::new();
        let trig_rows: Vec<(String, String, String, String)> = sqlx::query_as(AssertSqlSafe(
            format!(
                "SELECT n.nspname, c.relname, t.tgname, pg_get_triggerdef(t.oid) \
                 FROM pg_trigger t \
                 JOIN pg_class c ON c.oid = t.tgrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT t.tgisinternal AND n.nspname NOT IN {SYSTEM_SCHEMAS} \
                 ORDER BY n.nspname, c.relname, t.tgname"
            ),
        ))
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        for (schema, table, name, def) in trig_rows {
            let (timing, events, level, when_condition) = parse_trigger_header(&def);
            triggers.push(TriggerInfo {
                schema: Some(schema),
                name,
                table,
                timing,
                events,
                level,
                when_condition,
                action: def,
            });
        }

        Ok(SchemaTree {
            database_name,
            tables: tables.into_values().collect(),
            views: views.into_values().collect(),
            routines,
            triggers,
        })
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        let start = Instant::now();
        if returns_rows(sql) {
            // Stream rows instead of fetch_all: a SELECT over a huge table materializes at
            // most `max_rows` rows; dropping the stream early cancels the rest of the fetch.
            let mut stream = sqlx::query(AssertSqlSafe(sql.to_string())).fetch(&self.pool);
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
                data.push((0..columns.len()).map(|i| decode(&row, i, &types[i])).collect());
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
            let res = sqlx::query(AssertSqlSafe(sql.to_string()))
                .execute(&self.pool)
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
        // Run on a dedicated connection so we know exactly which backend to cancel; the kill
        // (pg_cancel_backend) must come from a *different* connection while this one is busy.
        let mut conn = self.pool.acquire().await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await?;

        if returns_rows(sql) {
            let mut stream = sqlx::query(AssertSqlSafe(sql.to_string())).fetch(&mut *conn);
            let mut columns: Vec<ColumnMeta> = Vec::new();
            let mut types: Vec<String> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        drop(stream);
                        self.cancel_backend(pid).await;
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
                    self.cancel_backend(pid).await;
                    Err(CoreError::Canceled)
                }
                res = sqlx::query(AssertSqlSafe(sql.to_string())).execute(&mut *conn) => {
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
        // collected, so the whole (possibly huge) table never sits in memory.
        let mut stream = sqlx::query(AssertSqlSafe(sql.to_string())).fetch(&self.pool);
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
            let values: Vec<Value> =
                (0..types.len()).map(|i| decode(&row, i, &types[i])).collect();
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
            sqlx::query(AssertSqlSafe(stmt.as_str())).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(stmts.len())
    }

    async fn list_databases(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT datname FROM pg_database \
             WHERE datistemplate = false AND datallowconn = true \
             ORDER BY datname",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }
}

/// Build result-column metadata from a sample row.
fn column_meta(row: &PgRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

/// Decode one Postgres cell into a backend-agnostic [`Value`], dispatching on the
/// column's upper-cased type name (resolved once per result by the caller) and falling
/// back gracefully for types we don't special-case.
fn decode(row: &PgRow, idx: usize, name: &str) -> Value {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
    match name {
        "BOOL" => row
            .try_get::<bool, _>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" => row
            .try_get::<i16, _>(idx)
            .map(|v| Value::Int(v as i64))
            .unwrap_or(Value::Null),
        "INT4" => row
            .try_get::<i32, _>(idx)
            .map(|v| Value::Int(v as i64))
            .unwrap_or(Value::Null),
        "INT8" => row
            .try_get::<i64, _>(idx)
            .map(Value::Int)
            .unwrap_or(Value::Null),
        "FLOAT4" => row
            .try_get::<f32, _>(idx)
            .map(|v| Value::Float(v as f64))
            .unwrap_or(Value::Null),
        "FLOAT8" => row
            .try_get::<f64, _>(idx)
            .map(Value::Float)
            .unwrap_or(Value::Null),
        "NUMERIC" => row
            .try_get::<sqlx::types::BigDecimal, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TEXT" | "VARCHAR" | "BPCHAR" | "CHAR" | "NAME" | "CITEXT" => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .unwrap_or(Value::Null),
        "UUID" => row
            .try_get::<sqlx::types::Uuid, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => row
            .try_get::<serde_json::Value, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(idx)
            .map(|v| Value::Text(v.to_rfc3339()))
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "BYTEA" => row
            .try_get::<Vec<u8>, _>(idx)
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        _ => {
            // Unknown/extension type: try text, then bytes, else show the type name.
            row.try_get::<String, _>(idx)
                .map(Value::Text)
                .or_else(|_| row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes))
                .unwrap_or_else(|_| Value::Text(format!("<{}>", name.to_lowercase())))
        }
    }
}

/// Extract column names from the `(...)` portion of a `CREATE INDEX` definition.
fn parse_index_columns(indexdef: &str) -> Vec<String> {
    let Some(start) = indexdef.find('(') else {
        return Vec::new();
    };
    let Some(end) = indexdef.rfind(')') else {
        return Vec::new();
    };
    if end <= start + 1 {
        return Vec::new();
    }
    indexdef[start + 1..end]
        .split(',')
        .map(|c| c.trim().trim_matches('"').to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

/// Parse the rendered output of `pg_get_function_arguments` (e.g.
/// `"a integer, b text DEFAULT 'x', OUT total numeric, VARIADIC nums integer[]"`) into
/// structured parameters. Splits on top-level commas (respecting parens/brackets/quotes),
/// then reads an optional leading mode keyword, the parameter name, and the remaining type
/// (minus any `DEFAULT`). Best-effort: unnamed parameters with multi-word types are rare and
/// may mis-split, which only affects the visual editor's pre-fill, never browsing.
fn parse_pg_args(s: &str) -> Vec<RoutineParam> {
    split_top_level_commas(s)
        .iter()
        .filter_map(|arg| parse_one_pg_arg(arg.trim()))
        .collect()
}

/// Split `s` on commas that are not nested inside parentheses/brackets or a string literal.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_quote = false;
    let mut start = 0usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'\'' => in_quote = !in_quote,
            b'(' | b'[' if !in_quote => depth += 1,
            b')' | b']' if !in_quote => depth -= 1,
            b',' if !in_quote && depth == 0 => {
                out.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(s[start..].to_string());
    }
    out.retain(|p| !p.trim().is_empty());
    out
}

/// Parse one argument declaration into a [`RoutineParam`].
fn parse_one_pg_arg(arg: &str) -> Option<RoutineParam> {
    if arg.is_empty() {
        return None;
    }
    // Optional leading mode keyword (INOUT before OUT so the prefix doesn't shadow it).
    let mut rest = arg;
    let mut mode = ParamMode::In;
    for (kw, m) in [
        ("INOUT", ParamMode::InOut),
        ("OUT", ParamMode::Out),
        ("VARIADIC", ParamMode::Variadic),
        ("IN", ParamMode::In),
    ] {
        if let Some(after) = strip_leading_word(rest, kw) {
            mode = m;
            rest = after;
            break;
        }
    }
    // Split off a trailing `DEFAULT expr`.
    let (decl, default) = match rest.to_ascii_uppercase().find(" DEFAULT ") {
        Some(pos) => {
            let d = rest[pos + " DEFAULT ".len()..].trim();
            (rest[..pos].trim(), (!d.is_empty()).then(|| d.to_string()))
        }
        None => (rest.trim(), None),
    };
    // pg emits "name type"; the first token is the name, the remainder the (possibly
    // multi-word) type.
    let (name, data_type) = decl.split_once(char::is_whitespace)?;
    Some(RoutineParam {
        name: name.to_string(),
        data_type: data_type.trim().to_string(),
        mode,
        default,
    })
}

/// Strip a leading whole word `word` (case-insensitive) off `s`, returning the trimmed
/// remainder, or `None` if `s` doesn't start with that exact word.
fn strip_leading_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let s = s.trim_start();
    let head = s.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = &s[word.len()..];
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest.trim_start()),
        _ => None,
    }
}
