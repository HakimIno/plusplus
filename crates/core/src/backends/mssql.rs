//! SQL Server backend, implemented on top of `tiberius` (a pure-Rust TDS driver).
//!
//! Unlike the other backends, this one can't ride on `sqlx`: sqlx dropped its MSSQL
//! driver after 0.6. `tiberius` has no built-in connection pool either, so we wrap it in
//! `bb8` to match the pooled, `Arc`-shareable shape the rest of the app expects.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use tiberius::{ColumnData, FromSqlOwned, Row};

use crate::database::{statements_return_rows, Database, ROW_KEYWORDS};
use crate::error::{CoreError, Result};
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, ForeignKeyInfo, IndexInfo, QueryResult,
    QueryStats, SchemaTree, SslMode, TableInfo,
};
use crate::value::Value;

type Pool = bb8::Pool<bb8_tiberius::ConnectionManager>;

/// Does this T-SQL batch return rows? Extends the shared classifier with `EXEC`/`EXECUTE`,
/// since a stored procedure invoked with `EXEC` can return result sets we want to display.
fn mssql_returns_rows(sql: &str) -> bool {
    statements_return_rows(sql, ROW_KEYWORDS) || statements_return_rows(sql, &["exec", "execute"])
}

pub struct MsSqlDb {
    pool: Pool,
}

impl MsSqlDb {
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> Result<Self> {
        // Probe TCP reachability first, with a short timeout. SQL Server deliberately
        // *delays* its "Login failed" response (anti-brute-force throttling), so a wrong
        // password can take several seconds to come back. If we relied on a single short
        // overall timeout, that delay would be clipped and misreported as an unreachable
        // host — highlighting Host/Port when the real problem is the credentials. Splitting
        // the two lets an unreachable host fail fast here (→ a clear host/port error), while
        // a reachable host gets a generous budget below for the TLS+login handshake so its
        // real error (e.g. "Login failed for user 'x'") can surface and flag User/Password.
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(CoreError::Pool(format!("could not reach the host: {e}"))),
            Err(_) => {
                return Err(CoreError::Pool(
                    "connection timed out — could not reach the host".into(),
                ))
            }
        }

        let mut config = tiberius::Config::new();
        config.host(&cfg.host);
        config.port(cfg.port);
        if !cfg.database.trim().is_empty() {
            config.database(&cfg.database);
        }
        config.authentication(tiberius::AuthMethod::sql_server(
            &cfg.user,
            password.as_deref().unwrap_or(""),
        ));
        // TDS negotiates encryption up front, so "prefer" can't fall back to plaintext
        // the way Postgres/MySQL can; it behaves like "require" here. The verify modes
        // both check the hostname too (rustls always does), so VerifyCa == VerifyFull.
        match cfg.ssl_mode {
            SslMode::Disable => {
                config.encryption(tiberius::EncryptionLevel::NotSupported);
            }
            SslMode::Prefer | SslMode::Require => {
                config.encryption(tiberius::EncryptionLevel::Required);
                config.trust_cert();
            }
            SslMode::VerifyCa | SslMode::VerifyFull => {
                config.encryption(tiberius::EncryptionLevel::Required);
                let ca = cfg.ssl_ca_cert.trim();
                if !ca.is_empty() {
                    config.trust_cert_ca(ca);
                }
            }
        }

        let mgr = bb8_tiberius::ConnectionManager::build(config)
            .map_err(|e| CoreError::Pool(e.to_string()))?;

        // Validate credentials by opening one real connection through the manager. We can't
        // rely on the pool for this: bb8 establishes connections in a background task and
        // funnels any failure (e.g. "Login failed") into its error sink, so `pool.get()`
        // only ever surfaces a generic `TimedOut` — never the real reason. `connect()` here
        // returns the actual tiberius error so the UI can show it and flag User/Password.
        // Bounded by a timeout in case TLS/login stalls; the host is already known reachable
        // from the TCP probe above, and the budget is generous because SQL Server
        // deliberately delays failed-login responses.
        use bb8::ManageConnection;
        match tokio::time::timeout(std::time::Duration::from_secs(15), mgr.connect()).await {
            Ok(Ok(_conn)) => {}
            Ok(Err(e)) => return Err(map_conn_err(e)),
            Err(_) => return Err(CoreError::Pool("timed out during the login handshake".into())),
        }

        let pool = bb8::Pool::builder()
            .max_size(5)
            // Don't let a transient runtime failure spin for the default 30s; surface it.
            .retry_connection(false)
            .connection_timeout(std::time::Duration::from_secs(15))
            .build(mgr)
            .await
            .map_err(|e| CoreError::Pool(e.to_string()))?;
        Ok(Self { pool })
    }

    async fn fetch(&self, sql: &str) -> Result<Vec<Row>> {
        let mut conn = self.pool.get().await.map_err(map_pool_err)?;
        let stream = conn.simple_query(sql.to_string()).await?;
        Ok(stream.into_first_result().await?)
    }
}

#[async_trait]
impl Database for MsSqlDb {
    fn kind(&self) -> DbKind {
        DbKind::SqlServer
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let database_name = self
            .fetch("SELECT DB_NAME()")
            .await?
            .first()
            .map(|r| get_str(r, 0))
            .unwrap_or_default();

        // Tables and views, keyed by (schema, name) so two schemas can share a table name.
        let mut tables: BTreeMap<(String, String), TableInfo> = BTreeMap::new();
        for row in self
            .fetch(
                "SELECT TABLE_SCHEMA, TABLE_NAME \
                 FROM INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME",
            )
            .await?
        {
            let schema = get_str(&row, 0);
            let name = get_str(&row, 1);
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

        // Primary-key columns, so we can flag them while loading columns below.
        let mut pk: std::collections::HashSet<(String, String, String)> =
            std::collections::HashSet::new();
        for row in self
            .fetch(
                "SELECT kcu.TABLE_SCHEMA, kcu.TABLE_NAME, kcu.COLUMN_NAME \
                 FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
                 JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
                   ON tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME \
                  AND tc.TABLE_SCHEMA = kcu.TABLE_SCHEMA \
                 WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY'",
            )
            .await?
        {
            pk.insert((get_str(&row, 0), get_str(&row, 1), get_str(&row, 2)));
        }

        for row in self
            .fetch(
                // Compose the full declared type (with length/precision) in T-SQL so it comes
                // back as one string — `char(10)`, `numeric(10,2)`, `nvarchar(max)`. Without the
                // length, an ALTER COLUMN built from this would silently shrink the column
                // (e.g. `char` defaults to `char(1)`), so the editor needs the real width.
                "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, \
                   DATA_TYPE + CASE \
                     WHEN DATA_TYPE IN ('char','varchar','nchar','nvarchar','binary','varbinary') \
                       THEN '(' + CASE WHEN CHARACTER_MAXIMUM_LENGTH = -1 THEN 'max' \
                                       ELSE CAST(CHARACTER_MAXIMUM_LENGTH AS varchar(11)) END + ')' \
                     WHEN DATA_TYPE IN ('decimal','numeric') \
                       THEN '(' + CAST(NUMERIC_PRECISION AS varchar(11)) + ',' \
                                + CAST(NUMERIC_SCALE AS varchar(11)) + ')' \
                     ELSE '' END AS FULL_TYPE, \
                   IS_NULLABLE \
                 FROM INFORMATION_SCHEMA.COLUMNS \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION",
            )
            .await?
        {
            let schema = get_str(&row, 0);
            let table = get_str(&row, 1);
            let column = get_str(&row, 2);
            if let Some(info) = tables.get_mut(&(schema.clone(), table.clone())) {
                let nullable = get_str(&row, 4).eq_ignore_ascii_case("YES");
                let primary_key = pk.contains(&(schema, table, column.clone()));
                info.columns.push(ColumnInfo {
                    name: column,
                    data_type: get_str(&row, 3),
                    nullable,
                    primary_key,
                });
            }
        }

        // Indexes live in the `sys` catalog views; INFORMATION_SCHEMA has no index metadata.
        let mut grouped: BTreeMap<(String, String, String), (bool, Vec<String>)> = BTreeMap::new();
        for row in self
            .fetch(
                "SELECT s.name, t.name, i.name, i.is_unique, c.name \
                 FROM sys.indexes i \
                 JOIN sys.tables t ON i.object_id = t.object_id \
                 JOIN sys.schemas s ON t.schema_id = s.schema_id \
                 JOIN sys.index_columns ic \
                   ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
                 JOIN sys.columns c \
                   ON ic.object_id = c.object_id AND ic.column_id = c.column_id \
                 WHERE i.name IS NOT NULL \
                 ORDER BY s.name, t.name, i.name, ic.key_ordinal",
            )
            .await?
        {
            let schema = get_str(&row, 0);
            let table = get_str(&row, 1);
            let index = get_str(&row, 2);
            let unique = row.try_get::<bool, _>(3).ok().flatten().unwrap_or(false);
            let column = get_str(&row, 4);
            grouped
                .entry((schema, table, index))
                .or_insert_with(|| (unique, Vec::new()))
                .1
                .push(column);
        }

        for ((schema, table, name), (unique, columns)) in grouped {
            if let Some(info) = tables.get_mut(&(schema, table)) {
                info.indexes.push(IndexInfo {
                    name,
                    unique,
                    columns,
                });
            }
        }

        // Foreign keys, from the sys catalog; column pairs ordered by constraint_column_id.
        for row in self
            .fetch(
                "SELECT s.name, t.name, fk.name, rs.name, rt.name, pc.name, rc.name, \
                        fk.delete_referential_action_desc, fk.update_referential_action_desc \
                 FROM sys.foreign_keys fk \
                 JOIN sys.tables t ON fk.parent_object_id = t.object_id \
                 JOIN sys.schemas s ON t.schema_id = s.schema_id \
                 JOIN sys.tables rt ON fk.referenced_object_id = rt.object_id \
                 JOIN sys.schemas rs ON rt.schema_id = rs.schema_id \
                 JOIN sys.foreign_key_columns fkc \
                   ON fkc.constraint_object_id = fk.object_id \
                 JOIN sys.columns pc \
                   ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN sys.columns rc \
                   ON rc.object_id = fkc.referenced_object_id \
                  AND rc.column_id = fkc.referenced_column_id \
                 ORDER BY s.name, t.name, fk.name, fkc.constraint_column_id",
            )
            .await?
        {
            let schema = get_str(&row, 0);
            let table = get_str(&row, 1);
            let constraint = get_str(&row, 2);
            if let Some(info) = tables.get_mut(&(schema, table)) {
                match info.foreign_keys.iter_mut().find(|fk| fk.name == constraint) {
                    Some(fk) => {
                        fk.columns.push(get_str(&row, 5));
                        fk.ref_columns.push(get_str(&row, 6));
                    }
                    None => info.foreign_keys.push(ForeignKeyInfo {
                        name: constraint,
                        columns: vec![get_str(&row, 5)],
                        ref_schema: Some(get_str(&row, 3)),
                        ref_table: get_str(&row, 4),
                        ref_columns: vec![get_str(&row, 6)],
                        // sys reports "NO_ACTION" / "SET_NULL"; normalize to SQL spelling.
                        on_delete: get_str(&row, 7).replace('_', " "),
                        on_update: get_str(&row, 8).replace('_', " "),
                    }),
                }
            }
        }

        Ok(SchemaTree {
            database_name,
            tables: tables.into_values().collect(),
        })
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        use tiberius::QueryItem;
        let start = Instant::now();
        let elapsed = |start: Instant| start.elapsed().as_secs_f64() * 1000.0;

        if mssql_returns_rows(sql) {
            let mut conn = self
                .pool
                .get()
                .await
                .map_err(map_pool_err)?;
            let mut stream = conn.simple_query(sql.to_string()).await?;
            // Stream row-by-row instead of `into_first_result` so at most `max_rows` rows
            // are materialized. The stream is still drained to its end — TDS gives no way
            // to abandon it mid-result without poisoning the pooled connection — but rows
            // past the cap (and any later result sets, as before) are dropped on the floor.
            let mut columns: Vec<ColumnMeta> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            let mut result_sets = 0usize;
            while let Some(item) = stream.try_next().await? {
                match item {
                    QueryItem::Metadata(meta) => {
                        result_sets += 1;
                        if result_sets == 1 {
                            columns = meta
                                .columns()
                                .iter()
                                .map(|c| ColumnMeta {
                                    name: c.name().to_string(),
                                    type_name: format!("{:?}", c.column_type()),
                                })
                                .collect();
                        }
                    }
                    QueryItem::Row(row) => {
                        if result_sets > 1 {
                            continue;
                        }
                        if data.len() >= max_rows {
                            truncated = true;
                            continue;
                        }
                        data.push(row.into_iter().map(|c| decode(&c)).collect());
                    }
                }
            }
            Ok(QueryResult {
                columns,
                rows: data,
                stats: QueryStats {
                    elapsed_ms: elapsed(start),
                    rows_affected: None,
                },
                truncated,
            })
        } else {
            // DML/DDL: `execute` reports rows affected (summed across statements).
            let mut conn = self
                .pool
                .get()
                .await
                .map_err(map_pool_err)?;
            let res = conn.execute(sql.to_string(), &[]).await?;
            Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                stats: QueryStats {
                    elapsed_ms: elapsed(start),
                    rows_affected: Some(res.total()),
                },
                truncated: false,
            })
        }
    }

    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        if stmts.is_empty() {
            return Ok(0);
        }
        let n = stmts.len();
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(map_pool_err)?;
        // SET XACT_ABORT ON: any runtime error automatically rolls back the transaction,
        // leaving the connection in a clean state when returned to the pool.
        conn.simple_query("SET XACT_ABORT ON; BEGIN TRANSACTION;")
            .await?
            .into_results()
            .await?;
        for stmt in stmts {
            conn.simple_query(stmt.as_str())
                .await?
                .into_results()
                .await?;
        }
        conn.simple_query("COMMIT TRANSACTION;")
            .await?
            .into_results()
            .await?;
        Ok(n)
    }

    async fn list_databases(&self) -> Result<Vec<String>> {
        let rows = self
            .fetch(
                "SELECT name FROM sys.databases \
                 WHERE state_desc = 'ONLINE' AND database_id > 4 \
                 ORDER BY name",
            )
            .await?;
        Ok(rows.iter().map(|r| get_str(r, 0)).filter(|s| !s.is_empty()).collect())
    }
}

/// Turn a bb8 pool error into a clean [`CoreError`]. `RunError::User` carries the real
/// tiberius error (e.g. "Login failed for user 'x'."), which we surface as a `Tiberius`
/// error so the UI shows the actual reason and can highlight the offending field. A
/// `TimedOut` means we never reached the server within the connection timeout.
fn map_conn_err(err: bb8_tiberius::Error) -> CoreError {
    match err {
        bb8_tiberius::Error::Tiberius(e) => CoreError::Tiberius(e),
        bb8_tiberius::Error::Io(e) => CoreError::Pool(e.to_string()),
    }
}

fn map_pool_err(err: bb8::RunError<bb8_tiberius::Error>) -> CoreError {
    match err {
        bb8::RunError::User(e) => map_conn_err(e),
        bb8::RunError::TimedOut => {
            CoreError::Pool("connection timed out — could not reach the host".into())
        }
    }
}

/// Read a column as a string, treating decode failures and NULLs as the empty string.
/// Used only for introspection queries whose columns are all `nvarchar`.
fn get_str(row: &Row, idx: usize) -> String {
    row.try_get::<&str, _>(idx)
        .ok()
        .flatten()
        .unwrap_or_default()
        .to_string()
}

/// Decode one cell. By the time a [`Row`] reaches us, tiberius has already resolved each
/// value to a concrete [`ColumnData`] variant (e.g. an `intn` arrives as `I32`), so we can
/// match on the data rather than on the declared column type.
fn decode(cell: &ColumnData<'static>) -> Value {
    match cell {
        ColumnData::U8(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I16(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I32(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I64(v) => v.map(Value::Int).unwrap_or(Value::Null),
        ColumnData::F32(v) => v.map(|x| Value::Float(x as f64)).unwrap_or(Value::Null),
        ColumnData::F64(v) => v.map(Value::Float).unwrap_or(Value::Null),
        ColumnData::Bit(v) => v.map(Value::Bool).unwrap_or(Value::Null),
        ColumnData::String(v) => v
            .as_ref()
            .map(|s| Value::Text(s.to_string()))
            .unwrap_or(Value::Null),
        ColumnData::Guid(v) => v.map(|g| Value::Text(g.to_string())).unwrap_or(Value::Null),
        ColumnData::Numeric(v) => v.map(|n| Value::Text(n.to_string())).unwrap_or(Value::Null),
        ColumnData::Xml(v) => v
            .as_ref()
            .map(|x| Value::Text(x.to_string()))
            .unwrap_or(Value::Null),
        ColumnData::Binary(v) => v
            .as_ref()
            .map(|b| Value::Bytes(b.to_vec()))
            .unwrap_or(Value::Null),
        // Temporal types decode through tiberius' chrono `FromSqlOwned` impls.
        ColumnData::Date(_) => temporal::<chrono::NaiveDate>(cell),
        ColumnData::Time(_) => temporal::<chrono::NaiveTime>(cell),
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            temporal::<chrono::NaiveDateTime>(cell)
        }
        ColumnData::DateTimeOffset(_) => temporal::<chrono::DateTime<chrono::Utc>>(cell),
    }
}

/// Decode a temporal cell into text via tiberius' chrono `FromSqlOwned` conversion.
fn temporal<T>(cell: &ColumnData<'static>) -> Value
where
    T: FromSqlOwned + ToString,
{
    match T::from_sql_owned(cell.clone()) {
        Ok(Some(v)) => Value::Text(v.to_string()),
        _ => Value::Null,
    }
}
