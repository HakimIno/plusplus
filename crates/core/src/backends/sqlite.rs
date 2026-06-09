//! SQLite backend implemented on top of `sqlx` (bundled libsqlite3).

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Row, TypeInfo, ValueRef};

use crate::database::{returns_rows, Database};
use crate::error::Result;
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, IndexInfo, QueryResult, QueryStats,
    SchemaTree, TableInfo,
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
        // Tables and views, excluding SQLite's internal bookkeeping tables.
        let names: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tables = Vec::with_capacity(names.len());
        for (table,) in names {
            let columns = self.introspect_columns(&table).await?;
            let indexes = self.introspect_indexes(&table).await?;
            tables.push(TableInfo {
                schema: None,
                name: table,
                columns,
                indexes,
            });
        }

        Ok(SchemaTree {
            database_name: self.name.clone(),
            tables,
        })
    }

    async fn execute(&self, sql: &str) -> Result<QueryResult> {
        let start = Instant::now();
        if returns_rows(sql) {
            let rows = sqlx::query(AssertSqlSafe(sql.to_string()))
                .fetch_all(&self.pool)
                .await?;
            let columns = rows.first().map(column_meta).unwrap_or_default();
            let data = rows
                .iter()
                .map(|row| (0..columns.len()).map(|i| decode(row, i)).collect())
                .collect();
            Ok(QueryResult {
                columns,
                rows: data,
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: None,
                },
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
            })
        }
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
            let col_rows = sqlx::query(AssertSqlSafe(cols_q)).fetch_all(&self.pool).await?;
            let columns = col_rows
                .iter()
                .filter_map(|c| c.try_get::<Option<String>, _>("name").ok().flatten())
                .collect();
            indexes.push(IndexInfo { name, unique, columns });
        }
        Ok(indexes)
    }
}

/// Quote an SQLite identifier for safe inlining into a PRAGMA.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
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

/// Decode one SQLite cell. SQLite is dynamically typed, so we probe storage classes in
/// order (integer → real → text → blob) rather than dispatching on a declared type.
fn decode(row: &SqliteRow, idx: usize) -> Value {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
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
