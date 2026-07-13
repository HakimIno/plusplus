//! SQL Server backend, implemented on top of `tiberius` (a pure-Rust TDS driver).
//!
//! Unlike the other backends, this one can't ride on `sqlx`: sqlx dropped its MSSQL
//! driver after 0.6. `tiberius` has no built-in connection pool either, so we wrap it in
//! `bb8` to match the pooled, `Arc`-shareable shape the rest of the app expects.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use tiberius::{ColumnData, FromSqlOwned, Row};
use tokio_util::sync::CancellationToken;

use crate::database::{statements_return_rows, Database, ROW_KEYWORDS};
use crate::error::{CoreError, Result};
use crate::model::{
    parse_trigger_header, select_body_after_as, ColumnInfo, ColumnMeta, ConnectionConfig, DbKind,
    ForeignKeyInfo, IndexInfo, ParamMode, QueryResult, QueryStats, RoutineInfo, RoutineKind,
    RoutineParam, SchemaTree, SslMode, TableInfo, TriggerInfo, TriggerLevel, TriggerTiming,
    ViewInfo,
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
        // Best effort only: ApplicationIntent=ReadOnly is enforced by readable secondaries
        // in an availability group, but a primary ignores it. On SQL Server the UI's
        // lexical guard is the effective read-only layer.
        if cfg.read_only {
            config.readonly(true);
        }
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
            Err(_) => {
                return Err(CoreError::Pool(
                    "timed out during the login handshake".into(),
                ))
            }
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

    /// `KILL <spid>` terminates the session running the query, from a fresh pooled
    /// connection. SQL Server has no "cancel just this statement" SQL (that's the TDS
    /// attention signal, which tiberius doesn't expose), so this drops the whole session;
    /// the connection it was on is discarded by the pool. Best-effort.
    async fn kill_spid(&self, spid: i32) {
        if let Ok(mut conn) = self.pool.get().await {
            let _ = conn.simple_query(format!("KILL {spid}")).await;
        }
    }
}

#[async_trait]
impl Database for MsSqlDb {
    fn kind(&self) -> DbKind {
        DbKind::SqlServer
    }

    async fn introspect_overview(&self) -> Result<SchemaTree> {
        let (database_rows, object_rows) = tokio::try_join!(
            self.fetch("SELECT DB_NAME()"),
            self.fetch(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME",
            ),
        )?;
        let database_name = database_rows
            .first()
            .map(|r| get_str(r, 0))
            .unwrap_or_default();
        let mut tables = Vec::new();
        let mut views = Vec::new();
        for row in object_rows {
            let schema = get_str(&row, 0);
            let name = get_str(&row, 1);
            if get_str(&row, 2).eq_ignore_ascii_case("VIEW") {
                views.push(ViewInfo {
                    schema: Some(schema),
                    name,
                    columns: Vec::new(),
                    definition: String::new(),
                    materialized: false,
                });
            } else {
                tables.push(TableInfo {
                    schema: Some(schema),
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
        let (database_rows, object_rows) = tokio::try_join!(
            self.fetch("SELECT DB_NAME()"),
            self.fetch(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME",
            ),
        )?;
        let database_name = database_rows
            .first()
            .map(|r| get_str(r, 0))
            .unwrap_or_default();

        // Base tables and views, keyed by (schema, name) so two schemas can share a name.
        // Columns below populate both buckets.
        let mut tables: BTreeMap<(String, String), TableInfo> = BTreeMap::new();
        let mut views: BTreeMap<(String, String), ViewInfo> = BTreeMap::new();
        for row in object_rows {
            let schema = get_str(&row, 0);
            let name = get_str(&row, 1);
            if get_str(&row, 2).eq_ignore_ascii_case("VIEW") {
                views.insert(
                    (schema.clone(), name.clone()),
                    ViewInfo {
                        schema: Some(schema),
                        name,
                        columns: Vec::new(),
                        definition: String::new(),
                        materialized: false,
                    },
                );
            } else {
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
            let col = ColumnInfo {
                name: column.clone(),
                data_type: get_str(&row, 3),
                nullable: get_str(&row, 4).eq_ignore_ascii_case("YES"),
                primary_key: pk.contains(&(schema.clone(), table.clone(), column)),
            };
            if let Some(info) = tables.get_mut(&(schema.clone(), table.clone())) {
                info.columns.push(col);
            } else if let Some(view) = views.get_mut(&(schema, table)) {
                view.columns.push(col);
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
                match info
                    .foreign_keys
                    .iter_mut()
                    .find(|fk| fk.name == constraint)
                {
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

        // View definitions (OBJECT_DEFINITION returns the full CREATE; keep just the SELECT).
        // The object queries degrade gracefully: a privilege error yields no rows, never a
        // failed introspection.
        for row in self
            .fetch(
                "SELECT s.name, v.name, OBJECT_DEFINITION(v.object_id) \
                 FROM sys.views v JOIN sys.schemas s ON v.schema_id = s.schema_id",
            )
            .await
            .unwrap_or_default()
        {
            let key = (get_str(&row, 0), get_str(&row, 1));
            if let Some(view) = views.get_mut(&key) {
                view.definition = select_body_after_as(&get_str(&row, 2));
            }
        }

        // Routines: scalar/inline/table functions and procedures; parameters attached below.
        let mut routines: BTreeMap<(String, String), RoutineInfo> = BTreeMap::new();
        for row in self
            .fetch(
                "SELECT s.name, o.name, o.type, OBJECT_DEFINITION(o.object_id) \
                 FROM sys.objects o JOIN sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN', 'IF', 'TF', 'P') \
                 ORDER BY s.name, o.name",
            )
            .await
            .unwrap_or_default()
        {
            let schema = get_str(&row, 0);
            let name = get_str(&row, 1);
            let kind = if get_str(&row, 2).trim().eq_ignore_ascii_case("P") {
                RoutineKind::Procedure
            } else {
                RoutineKind::Function
            };
            routines.insert(
                (schema.clone(), name.clone()),
                RoutineInfo {
                    schema: Some(schema),
                    name,
                    kind,
                    params: Vec::new(),
                    return_type: None,
                    language: String::new(),
                    body: get_str(&row, 3),
                },
            );
        }
        // Parameters: parameter_id 0 (empty name) is a scalar function's return type.
        for row in self
            .fetch(
                "SELECT s.name, o.name, p.name, TYPE_NAME(p.user_type_id), p.is_output, \
                        p.parameter_id \
                 FROM sys.parameters p \
                 JOIN sys.objects o ON p.object_id = o.object_id \
                 JOIN sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN', 'IF', 'TF', 'P') \
                 ORDER BY s.name, o.name, p.parameter_id",
            )
            .await
            .unwrap_or_default()
        {
            let key = (get_str(&row, 0), get_str(&row, 1));
            let Some(routine) = routines.get_mut(&key) else {
                continue;
            };
            let param_id = row.try_get::<i32, _>(5).ok().flatten().unwrap_or(0);
            if param_id == 0 {
                // The implicit return parameter of a scalar function.
                routine.return_type = Some(get_str(&row, 3));
            } else {
                let is_output = row.try_get::<bool, _>(4).ok().flatten().unwrap_or(false);
                routine.params.push(RoutineParam {
                    name: get_str(&row, 2),
                    data_type: get_str(&row, 3),
                    mode: if is_output {
                        ParamMode::Out
                    } else {
                        ParamMode::In
                    },
                    default: None,
                });
            }
        }

        // Triggers: DML triggers only (parent_class = 1). SQL Server fires AFTER / INSTEAD OF,
        // always statement-level; the events/condition are parsed from the definition.
        let mut triggers = Vec::new();
        for row in self
            .fetch(
                "SELECT s.name, tb.name, tr.name, OBJECT_DEFINITION(tr.object_id) \
                 FROM sys.triggers tr \
                 JOIN sys.tables tb ON tr.parent_id = tb.object_id \
                 JOIN sys.schemas s ON tb.schema_id = s.schema_id \
                 WHERE tr.is_ms_shipped = 0 AND tr.parent_class = 1 \
                 ORDER BY s.name, tb.name, tr.name",
            )
            .await
            .unwrap_or_default()
        {
            let def = get_str(&row, 3);
            let (mut timing, events, _level, when_condition) = parse_trigger_header(&def);
            // SQL Server has no BEFORE triggers; a non-INSTEAD-OF one is AFTER (incl. `FOR`).
            if timing != TriggerTiming::InsteadOf {
                timing = TriggerTiming::After;
            }
            triggers.push(TriggerInfo {
                schema: Some(get_str(&row, 0)),
                name: get_str(&row, 2),
                table: get_str(&row, 1),
                timing,
                events,
                level: TriggerLevel::Statement,
                when_condition,
                action: def,
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
        use tiberius::QueryItem;
        let start = Instant::now();
        let elapsed = |start: Instant| start.elapsed().as_secs_f64() * 1000.0;

        if mssql_returns_rows(sql) {
            let mut conn = self.pool.get().await.map_err(map_pool_err)?;
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
            let mut conn = self.pool.get().await.map_err(map_pool_err)?;
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

    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: CancellationToken,
    ) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        use tiberius::QueryItem;
        let start = Instant::now();
        let elapsed = |start: Instant| start.elapsed().as_secs_f64() * 1000.0;

        let mut conn = self.pool.get().await.map_err(map_pool_err)?;
        // Capture this session's id up front so a cancel can target it with KILL from a
        // separate connection. @@SPID is a smallint.
        let spid: i32 = {
            let rows = conn
                .simple_query("SELECT @@SPID")
                .await?
                .into_first_result()
                .await?;
            rows.first()
                .and_then(|r| r.try_get::<i16, _>(0).ok().flatten())
                .map(|v| v as i32)
                .ok_or_else(|| CoreError::Pool("could not read @@SPID".into()))?
        };

        if mssql_returns_rows(sql) {
            let mut stream = conn.simple_query(sql.to_string()).await?;
            let mut columns: Vec<ColumnMeta> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            let mut result_sets = 0usize;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        drop(stream);
                        self.kill_spid(spid).await;
                        return Err(CoreError::Canceled);
                    }
                    item = stream.try_next() => {
                        let Some(item) = item? else { break };
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
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.kill_spid(spid).await;
                    Err(CoreError::Canceled)
                }
                res = conn.execute(sql.to_string(), &[]) => {
                    let res = res?;
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
        }
    }

    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        use futures_util::TryStreamExt;
        use tiberius::QueryItem;
        // Stream straight into the sink: rows are written one at a time and never collected,
        // so the whole (possibly huge) table never sits in memory. As in execute_capped, only
        // the first result set is exported; the stream is still drained fully because TDS
        // can't be abandoned mid-result without poisoning the pooled connection.
        let mut conn = self.pool.get().await.map_err(map_pool_err)?;
        let mut stream = conn.simple_query(sql.to_string()).await?;
        let mut result_sets = 0usize;
        let mut count = 0u64;
        while let Some(item) = stream.try_next().await? {
            match item {
                QueryItem::Metadata(meta) => {
                    result_sets += 1;
                    if result_sets == 1 {
                        let columns: Vec<ColumnMeta> = meta
                            .columns()
                            .iter()
                            .map(|c| ColumnMeta {
                                name: c.name().to_string(),
                                type_name: format!("{:?}", c.column_type()),
                            })
                            .collect();
                        sink.begin(&columns)?;
                    }
                }
                QueryItem::Row(row) => {
                    if result_sets > 1 {
                        continue;
                    }
                    let values: Vec<Value> = row.into_iter().map(|c| decode(&c)).collect();
                    sink.write_row(&values)?;
                    count += 1;
                }
            }
        }
        sink.finish()?;
        Ok(count)
    }

    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        if stmts.is_empty() {
            return Ok(0);
        }
        let n = stmts.len();
        let mut conn = self.pool.get().await.map_err(map_pool_err)?;
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
        Ok(rows
            .iter()
            .map(|r| get_str(r, 0))
            .filter(|s| !s.is_empty())
            .collect())
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
