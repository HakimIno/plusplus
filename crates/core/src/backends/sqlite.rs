//! SQLite backend implemented on top of `sqlx` (bundled libsqlite3).

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Row, TypeInfo, ValueRef};

use crate::database::{returns_rows, Database};
use crate::error::Result;
use crate::model::{
    parse_trigger_header, select_body_after_as, ColumnInfo, ColumnMeta, ConnectionConfig, DbKind,
    ForeignKeyInfo, IndexInfo, QueryResult, QueryStats, SchemaTree, TableInfo, TriggerInfo,
    TriggerLevel, ViewInfo,
};
use crate::value::Value;

pub struct SqliteDb {
    pool: SqlitePool,
    /// Display name derived from the file (used as the database node in the tree).
    name: String,
}

impl SqliteDb {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&cfg.sqlite_path)
            // Create the file if absent so users can spin up a scratch database.
            .create_if_missing(true)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        let name = Path::new(&cfg.sqlite_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sqlite".to_string());
        Ok(Self { pool, name })
    }
}

#[async_trait]
impl Database for SqliteDb {
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        // Tables, views, and triggers in one sweep, excluding SQLite's internal bookkeeping
        // objects. `tbl_name` is the owning table (== name for tables/views); `sql` is the
        // original `CREATE` text (NULL for auto-created objects).
        let objects: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT type, name, tbl_name, sql FROM sqlite_master \
             WHERE type IN ('table', 'view', 'trigger') AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tables = Vec::new();
        let mut views = Vec::new();
        let mut triggers = Vec::new();
        for (ty, name, tbl_name, sql) in objects {
            match ty.as_str() {
                "table" => {
                    let columns = self.introspect_columns(&name).await?;
                    let indexes = self.introspect_indexes(&name).await?;
                    let foreign_keys = self.introspect_foreign_keys(&name).await?;
                    tables.push(TableInfo {
                        schema: None,
                        name,
                        columns,
                        indexes,
                        foreign_keys,
                    });
                }
                "view" => {
                    let columns = self.introspect_columns(&name).await?;
                    views.push(ViewInfo {
                        schema: None,
                        name,
                        columns,
                        definition: select_body_after_as(sql.as_deref().unwrap_or_default()),
                        materialized: false,
                    });
                }
                "trigger" => triggers.push(parse_sqlite_trigger(
                    &name,
                    tbl_name.as_deref().unwrap_or_default(),
                    sql.as_deref().unwrap_or_default(),
                )),
                _ => {}
            }
        }

        Ok(SchemaTree {
            database_name: self.name.clone(),
            tables,
            views,
            // SQLite has no stored functions or procedures.
            routines: Vec::new(),
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
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if columns.is_empty() {
                    columns = column_meta(&row);
                }
                if data.len() >= max_rows {
                    truncated = true;
                    break;
                }
                data.push((0..columns.len()).map(|i| decode(&row, i)).collect());
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

    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        use futures_util::TryStreamExt;
        // Stream straight into the sink: rows are written to the file one at a time and never
        // collected, so the whole (possibly huge) table never sits in memory.
        let mut stream = sqlx::query(AssertSqlSafe(sql.to_string())).fetch(&self.pool);
        let mut ncols = 0usize;
        let mut began = false;
        let mut count = 0u64;
        while let Some(row) = stream.try_next().await? {
            if !began {
                let columns = column_meta(&row);
                ncols = columns.len();
                sink.begin(&columns)?;
                began = true;
            }
            let values: Vec<Value> = (0..ncols).map(|i| decode(&row, i)).collect();
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
}

impl SqliteDb {
    async fn introspect_columns(&self, table: &str) -> Result<Vec<ColumnInfo>> {
        // PRAGMA cannot be parameterized, so the table name is inlined; quote-escape it.
        let q = format!("PRAGMA table_info({})", quote_ident(table));
        let rows = sqlx::query(AssertSqlSafe(q)).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| ColumnInfo {
                name: r.try_get::<String, _>("name").unwrap_or_default(),
                data_type: r.try_get::<String, _>("type").unwrap_or_default(),
                nullable: r.try_get::<i64, _>("notnull").unwrap_or(0) == 0,
                primary_key: r.try_get::<i64, _>("pk").unwrap_or(0) > 0,
            })
            .collect())
    }

    async fn introspect_indexes(&self, table: &str) -> Result<Vec<IndexInfo>> {
        let q = format!("PRAGMA index_list({})", quote_ident(table));
        let rows = sqlx::query(AssertSqlSafe(q)).fetch_all(&self.pool).await?;
        let mut indexes = Vec::new();
        for r in &rows {
            let name: String = r.try_get("name").unwrap_or_default();
            let unique = r.try_get::<i64, _>("unique").unwrap_or(0) == 1;
            let cols_q = format!("PRAGMA index_info({})", quote_ident(&name));
            let col_rows = sqlx::query(AssertSqlSafe(cols_q))
                .fetch_all(&self.pool)
                .await?;
            let columns = col_rows
                .iter()
                .filter_map(|c| c.try_get::<Option<String>, _>("name").ok().flatten())
                .collect();
            indexes.push(IndexInfo {
                name,
                unique,
                columns,
            });
        }
        Ok(indexes)
    }

    async fn introspect_foreign_keys(&self, table: &str) -> Result<Vec<ForeignKeyInfo>> {
        // One row per column pair, grouped by `id`; rows arrive ordered by (id, seq).
        let q = format!("PRAGMA foreign_key_list({})", quote_ident(table));
        let rows = sqlx::query(AssertSqlSafe(q)).fetch_all(&self.pool).await?;
        let mut fks: Vec<(i64, ForeignKeyInfo)> = Vec::new();
        for r in &rows {
            let id: i64 = r.try_get("id").unwrap_or(0);
            let column: String = r.try_get("from").unwrap_or_default();
            // `to` is NULL when the FK implicitly references the target's primary key.
            let ref_column: String = r
                .try_get::<Option<String>, _>("to")
                .ok()
                .flatten()
                .unwrap_or_default();
            match fks.iter_mut().find(|(fk_id, _)| *fk_id == id) {
                Some((_, fk)) => {
                    fk.columns.push(column);
                    if !ref_column.is_empty() {
                        fk.ref_columns.push(ref_column);
                    }
                }
                None => fks.push((
                    id,
                    ForeignKeyInfo {
                        // SQLite doesn't expose constraint names through the PRAGMA.
                        name: String::new(),
                        columns: vec![column],
                        ref_schema: None,
                        ref_table: r.try_get("table").unwrap_or_default(),
                        ref_columns: if ref_column.is_empty() {
                            Vec::new()
                        } else {
                            vec![ref_column]
                        },
                        on_delete: r.try_get("on_delete").unwrap_or_default(),
                        on_update: r.try_get("on_update").unwrap_or_default(),
                    },
                )),
            }
        }
        Ok(fks.into_iter().map(|(_, fk)| fk).collect())
    }
}

/// Quote an SQLite identifier for safe inlining into a PRAGMA.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Parse a SQLite `CREATE TRIGGER` statement into structured fields. SQLite only exposes the
/// original DDL text, so the full statement is kept verbatim in `action` for display; the
/// structured fields (parsed by the shared [`parse_trigger_header`]) drive the tree summary
/// and the visual editor.
fn parse_sqlite_trigger(name: &str, table: &str, sql: &str) -> TriggerInfo {
    let (timing, events, _level, when_condition) = parse_trigger_header(sql);
    TriggerInfo {
        schema: None,
        name: name.to_string(),
        table: table.to_string(),
        timing,
        events,
        // SQLite triggers are always row-level regardless of any FOR EACH clause.
        level: TriggerLevel::Row,
        when_condition,
        action: sql.trim().to_string(),
    }
}

fn column_meta(row: &SqliteRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

/// Decode one SQLite cell. SQLite is dynamically typed, so dispatch on the *value's*
/// storage class — known per cell from the raw value — instead of probing decoders in
/// order: each failed probe allocates a boxed decode error, which dominated the decode
/// cost on large text-heavy results.
fn decode(row: &SqliteRow, idx: usize) -> Value {
    let Ok(raw) = row.try_get_raw(idx) else {
        return Value::Null;
    };
    if raw.is_null() {
        return Value::Null;
    }
    let ti = raw.type_info();
    match ti.name() {
        "INTEGER" => row.try_get::<i64, _>(idx).map(Value::Int).unwrap_or(Value::Null),
        "REAL" => row.try_get::<f64, _>(idx).map(Value::Float).unwrap_or(Value::Null),
        "TEXT" => row.try_get::<String, _>(idx).map(Value::Text).unwrap_or(Value::Null),
        "BLOB" => row
            .try_get::<Vec<u8>, _>(idx)
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        // Unexpected storage class (e.g. an inferred BOOLEAN/DATETIME): fall back to the
        // old probing order so nothing ever decodes worse than before.
        _ => {
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(v);
            }
            Value::Null
        }
    }
}
