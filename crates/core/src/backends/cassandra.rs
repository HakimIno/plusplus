//! Cassandra / ScyllaDB backend on the `scylla` driver (CQL native protocol).
//!
//! One implementation serves both [`DbKind::Cassandra`] and [`DbKind::ScyllaDb`] — ScyllaDB
//! is wire-compatible and the driver speaks to either. The driver's `Session` maintains a
//! per-node connection pool internally, so unlike the sqlx/tiberius backends there is no
//! extra pooling layer here.
//!
//! CQL looks like SQL but is not: there are no joins, no `EXPLAIN`, no transactions, and
//! `INSERT` takes exactly one row. The places where those differences surface — batching,
//! [`Database::execute_transaction`], safety analysis — are documented at each method.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures_util::StreamExt;
use scylla::client::session::{Session, TlsContext};
use scylla::client::session_builder::SessionBuilder;
use scylla::cluster::metadata::{CollectionType, ColumnType, NativeType};
use scylla::errors::TranslationError;
use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
use scylla::statement::Statement;
use scylla::value::{CqlValue, Row};
use tokio_util::sync::CancellationToken;

use crate::database::{returns_rows, Database};
use crate::error::{CoreError, Result};
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, IndexInfo, ParamMode, QueryResult,
    QueryStats, RoutineInfo, RoutineKind, RoutineParam, SchemaTree, SslMode, TableInfo, ViewInfo,
};
use crate::value::Value;

/// Rows fetched per page when streaming a result. Large enough to amortize round trips,
/// small enough that abandoning a capped query never buffers much beyond the cap.
const PAGE_SIZE: i32 = 2000;

/// Keyspaces that belong to Cassandra/ScyllaDB itself, hidden from the schema browser and
/// the database switcher. Exact names — a user keyspace that merely starts with "system"
/// stays visible.
const SYSTEM_KEYSPACES: &[&str] = &[
    "system",
    "system_schema",
    "system_auth",
    "system_distributed",
    "system_distributed_everywhere",
    "system_replicated_keys",
    "system_traces",
    "system_views",
    "system_virtual_schema",
];

pub struct CassandraDb {
    session: Session,
    /// Cassandra or ScyllaDb — whichever the user picked; behaviour is identical.
    kind: DbKind,
    /// The keyspace this connection is scoped to (`ConnectionConfig::database`), if any.
    keyspace: Option<String>,
}

/// Maps every peer the cluster broadcasts to the single address we can actually reach —
/// the local end of the SSH tunnel. Peer discovery would otherwise make the driver dial
/// nodes' private addresses directly, bypassing (and defeating) the tunnel. With every
/// node translated to the tunnel, all connections land on the one bastion-reachable node;
/// token-aware routing degrades gracefully while queries stay correct.
#[derive(Debug)]
struct FixedAddress(SocketAddr);

#[async_trait]
impl AddressTranslator for FixedAddress {
    async fn translate_address(
        &self,
        _peer: &UntranslatedPeer,
    ) -> std::result::Result<SocketAddr, TranslationError> {
        Ok(self.0)
    }
}

impl CassandraDb {
    pub async fn connect(
        cfg: &ConnectionConfig,
        password: Option<String>,
        via_tunnel: bool,
    ) -> Result<Self> {
        // Probe TCP reachability first with a short timeout, so an unreachable host fails
        // fast with a clear host/port error instead of the driver's slower multi-attempt
        // metadata error.
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

        // "Prefer" means encrypt when the server can, plaintext when it can't. CQL has no
        // in-protocol TLS negotiation (a TLS listener is TLS-only), so prefer is: try TLS,
        // and if the session can't be established, try again without.
        match Self::connect_tls(cfg, password.clone(), via_tunnel).await {
            Ok(db) => Ok(db),
            Err(first_err) if cfg.ssl_mode == SslMode::Prefer => {
                match Self::build_session(cfg, password, via_tunnel, None).await {
                    Ok(db) => Ok(db),
                    // The plaintext retry failing too usually means credentials/keyspace;
                    // its error is the meaningful one. TLS-specific failures already
                    // surfaced via first_err on the Require/Verify modes.
                    Err(_) => Err(first_err),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Connect honouring `cfg.ssl_mode` literally (no plaintext fallback).
    async fn connect_tls(
        cfg: &ConnectionConfig,
        password: Option<String>,
        via_tunnel: bool,
    ) -> Result<Self> {
        let tls = match cfg.ssl_mode {
            SslMode::Disable => None,
            // Encrypted, but the server certificate is not verified (the same trade
            // tiberius makes with trust_cert). rustls needs an explicit "danger" verifier
            // for this; see NoCertVerification below.
            SslMode::Prefer | SslMode::Require => Some(tls_config_no_verify()?),
            // rustls always checks the hostname too, so VerifyCa behaves like VerifyFull
            // (same note as the SQL Server backend).
            SslMode::VerifyCa | SslMode::VerifyFull => {
                Some(tls_config_verify(cfg.ssl_ca_cert.trim())?)
            }
        };
        Self::build_session(cfg, password, via_tunnel, tls).await
    }

    async fn build_session(
        cfg: &ConnectionConfig,
        password: Option<String>,
        via_tunnel: bool,
        tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Self> {
        let mut builder = SessionBuilder::new()
            .known_node(format!("{}:{}", cfg.host, cfg.port))
            .connection_timeout(std::time::Duration::from_secs(15))
            .tls_context(tls.map(TlsContext::Rustls023));
        if !cfg.user.trim().is_empty() {
            builder = builder.user(cfg.user.clone(), password.unwrap_or_default());
        }
        if via_tunnel {
            let local: SocketAddr = format!("{}:{}", cfg.host, cfg.port)
                .parse()
                .map_err(|e| CoreError::Pool(format!("bad tunnel address: {e}")))?;
            builder = builder.address_translator(Arc::new(FixedAddress(local)));
        }
        let keyspace = Some(cfg.database.trim())
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        if let Some(ks) = &keyspace {
            // case_sensitive=false matches cqlsh: unquoted keyspace names are lowercased.
            builder = builder.use_keyspace(ks.clone(), false);
        }
        let session = tokio::time::timeout(std::time::Duration::from_secs(20), builder.build())
            .await
            .map_err(|_| CoreError::Pool("timed out establishing the session".into()))?
            .map_err(|e| CoreError::Cql(e.to_string()))?;
        Ok(Self {
            session,
            kind: cfg.kind,
            keyspace,
        })
    }

    /// Run a small internal metadata query and return its rows (empty when the result
    /// carries none). Used only for introspection, which is known-small.
    async fn fetch(&self, cql: &str) -> Result<Vec<Row>> {
        let result = self
            .session
            .query_unpaged(cql, &[])
            .await
            .map_err(|e| CoreError::Cql(e.to_string()))?;
        if result.is_rows() {
            let rows_result = result
                .into_rows_result()
                .map_err(|e| CoreError::Cql(e.to_string()))?;
            let rows = rows_result
                .rows::<Row>()
                .map_err(|e| CoreError::Cql(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Cql(e.to_string()))?;
            Ok(rows)
        } else {
            Ok(Vec::new())
        }
    }

    /// The keyspaces to introspect: the configured one, or every non-system keyspace.
    async fn target_keyspaces(&self) -> Result<Vec<String>> {
        match &self.keyspace {
            Some(ks) => Ok(vec![ks.to_ascii_lowercase()]),
            None => Ok(self
                .fetch("SELECT keyspace_name FROM system_schema.keyspaces")
                .await?
                .iter()
                .filter_map(|r| row_str(r, 0))
                .filter(|ks| !SYSTEM_KEYSPACES.contains(&ks.as_str()))
                .collect()),
        }
    }

    /// Shared body of `execute_capped` / `execute_capped_cancellable`. CQL paging is
    /// pull-based, so cancellation simply stops pulling and drops the pager — there is no
    /// server-side statement to kill (at most one in-flight page completes and is dropped).
    async fn run_capped(
        &self,
        cql: &str,
        max_rows: usize,
        cancel: Option<CancellationToken>,
    ) -> Result<QueryResult> {
        let start = Instant::now();
        let elapsed = |start: Instant| start.elapsed().as_secs_f64() * 1000.0;

        if returns_rows(cql) {
            let mut statement = Statement::new(cql);
            statement.set_page_size(PAGE_SIZE);
            let pager_fut = self.session.query_iter(statement, &[]);
            let pager = match &cancel {
                Some(token) => tokio::select! {
                    biased;
                    _ = token.cancelled() => return Err(CoreError::Canceled),
                    pager = pager_fut => pager,
                },
                None => pager_fut.await,
            }
            .map_err(|e| CoreError::Cql(e.to_string()))?;

            let columns: Vec<ColumnMeta> = pager
                .column_specs()
                .iter()
                .map(|spec| ColumnMeta {
                    name: spec.name().to_string(),
                    type_name: type_name(spec.typ()),
                })
                .collect();

            let mut stream = pager
                .rows_stream::<Row>()
                .map_err(|e| CoreError::Cql(e.to_string()))?;
            let mut rows: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            loop {
                let next = match &cancel {
                    Some(token) => tokio::select! {
                        biased;
                        _ = token.cancelled() => return Err(CoreError::Canceled),
                        next = stream.next() => next,
                    },
                    None => stream.next().await,
                };
                let Some(row) = next else { break };
                let row = row.map_err(|e| CoreError::Cql(e.to_string()))?;
                if rows.len() >= max_rows {
                    // Dropping the stream abandons the remaining pages — nothing more is
                    // fetched, unlike TDS there is no stream to drain.
                    truncated = true;
                    break;
                }
                rows.push(row.columns.iter().map(decode).collect());
            }
            Ok(QueryResult {
                columns,
                rows,
                stats: QueryStats {
                    elapsed_ms: elapsed(start),
                    rows_affected: None,
                },
                truncated,
            })
        } else {
            // DML/DDL. CQL reports no affected-row count (a write is an upsert that
            // always "succeeds"), so rows_affected stays None rather than lying with 0.
            // Conditional writes (`IF NOT EXISTS` / `IF …`) do return a result set — the
            // `[applied]` row — which we surface like any other rows.
            let exec_fut = self.session.query_unpaged(cql, &[]);
            let result = match &cancel {
                Some(token) => tokio::select! {
                    biased;
                    _ = token.cancelled() => return Err(CoreError::Canceled),
                    result = exec_fut => result,
                },
                None => exec_fut.await,
            }
            .map_err(|e| CoreError::Cql(e.to_string()))?;

            if result.is_rows() {
                let rows_result = result
                    .into_rows_result()
                    .map_err(|e| CoreError::Cql(e.to_string()))?;
                let columns: Vec<ColumnMeta> = rows_result
                    .column_specs()
                    .iter()
                    .map(|spec| ColumnMeta {
                        name: spec.name().to_string(),
                        type_name: type_name(spec.typ()),
                    })
                    .collect();
                let mut rows: Vec<Vec<Value>> = Vec::new();
                let mut truncated = false;
                for row in rows_result
                    .rows::<Row>()
                    .map_err(|e| CoreError::Cql(e.to_string()))?
                {
                    let row = row.map_err(|e| CoreError::Cql(e.to_string()))?;
                    if rows.len() >= max_rows {
                        truncated = true;
                        break;
                    }
                    rows.push(row.columns.iter().map(decode).collect());
                }
                Ok(QueryResult {
                    columns,
                    rows,
                    stats: QueryStats {
                        elapsed_ms: elapsed(start),
                        rows_affected: None,
                    },
                    truncated,
                })
            } else {
                Ok(QueryResult {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    stats: QueryStats {
                        elapsed_ms: elapsed(start),
                        rows_affected: None,
                    },
                    truncated: false,
                })
            }
        }
    }
}

#[async_trait]
impl Database for CassandraDb {
    fn kind(&self) -> DbKind {
        self.kind
    }

    async fn introspect_overview(&self) -> Result<SchemaTree> {
        let keyspaces = self.target_keyspaces().await?;
        let database_name = match &self.keyspace {
            Some(ks) => ks.clone(),
            None => self
                .fetch("SELECT cluster_name FROM system.local")
                .await?
                .first()
                .and_then(|r| row_str(r, 0))
                .unwrap_or_default(),
        };
        let mut tables = Vec::new();
        for row in self
            .fetch("SELECT keyspace_name, table_name FROM system_schema.tables")
            .await?
        {
            let (Some(ks), Some(name)) = (row_str(&row, 0), row_str(&row, 1)) else {
                continue;
            };
            if keyspaces.contains(&ks) {
                tables.push(TableInfo {
                    schema: Some(ks),
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                });
            }
        }
        let mut views = Vec::new();
        for row in self
            .fetch("SELECT keyspace_name, view_name FROM system_schema.views")
            .await?
        {
            let (Some(ks), Some(name)) = (row_str(&row, 0), row_str(&row, 1)) else {
                continue;
            };
            if keyspaces.contains(&ks) {
                views.push(ViewInfo {
                    schema: Some(ks),
                    name,
                    columns: Vec::new(),
                    definition: String::new(),
                    materialized: true,
                });
            }
        }
        tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
        views.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
        Ok(SchemaTree {
            database_name,
            tables,
            views,
            routines: Vec::new(),
            triggers: Vec::new(),
        })
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let mut tree = self.introspect_overview().await?;
        let keyspaces = self.target_keyspaces().await?;

        // Columns for tables *and* materialized views — both live in
        // system_schema.columns keyed by (keyspace_name, table_name).
        // `kind` is partition_key / clustering / static / regular; primary-key parts are
        // ordered by `position`, the rest come back alphabetical from the server.
        #[allow(clippy::type_complexity)]
        let mut cols: std::collections::HashMap<(String, String), Vec<(i8, i32, ColumnInfo)>> =
            std::collections::HashMap::new();
        for row in self
            .fetch(
                "SELECT keyspace_name, table_name, column_name, kind, position, type \
                 FROM system_schema.columns",
            )
            .await?
        {
            let (Some(ks), Some(table), Some(column)) =
                (row_str(&row, 0), row_str(&row, 1), row_str(&row, 2))
            else {
                continue;
            };
            if !keyspaces.contains(&ks) {
                continue;
            }
            let kind = row_str(&row, 3).unwrap_or_default();
            let position = match row.columns.get(4).and_then(|c| c.as_ref()) {
                Some(CqlValue::Int(p)) => *p,
                _ => -1,
            };
            // Sort group: partition keys, then clustering keys, then the rest.
            let group: i8 = match kind.as_str() {
                "partition_key" => 0,
                "clustering" => 1,
                _ => 2,
            };
            let primary_key = group < 2;
            cols.entry((ks, table)).or_default().push((
                group,
                position,
                ColumnInfo {
                    name: column,
                    data_type: row_str(&row, 5).unwrap_or_default(),
                    nullable: !primary_key,
                    primary_key,
                },
            ));
        }
        for list in cols.values_mut() {
            list.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
        }
        for table in &mut tree.tables {
            if let Some(ks) = table.schema.clone() {
                if let Some(list) = cols.remove(&(ks, table.name.clone())) {
                    table.columns = list.into_iter().map(|(_, _, c)| c).collect();
                }
            }
        }
        for view in &mut tree.views {
            if let Some(ks) = view.schema.clone() {
                if let Some(list) = cols.remove(&(ks, view.name.clone())) {
                    view.columns = list.into_iter().map(|(_, _, c)| c).collect();
                }
            }
        }

        // Secondary indexes. The indexed column hides in options['target'].
        for row in self
            .fetch(
                "SELECT keyspace_name, table_name, index_name, options \
                 FROM system_schema.indexes",
            )
            .await?
        {
            let (Some(ks), Some(table), Some(index)) =
                (row_str(&row, 0), row_str(&row, 1), row_str(&row, 2))
            else {
                continue;
            };
            let target = match row.columns.get(3).and_then(|c| c.as_ref()) {
                Some(CqlValue::Map(entries)) => entries
                    .iter()
                    .find(|(k, _)| matches!(k, CqlValue::Text(t) if t == "target"))
                    .and_then(|(_, v)| v.as_text().cloned()),
                _ => None,
            };
            if let Some(info) = tree
                .tables
                .iter_mut()
                .find(|t| t.schema.as_deref() == Some(ks.as_str()) && t.name == table)
            {
                info.indexes.push(IndexInfo {
                    name: index,
                    unique: false, // CQL has no unique secondary indexes
                    columns: target.into_iter().collect(),
                });
            }
        }

        // Materialized-view definitions: system_schema stores the WHERE clause, not a
        // SELECT, so synthesize a readable one.
        for row in self
            .fetch(
                "SELECT keyspace_name, view_name, base_table_name, where_clause \
                 FROM system_schema.views",
            )
            .await?
        {
            let (Some(ks), Some(name)) = (row_str(&row, 0), row_str(&row, 1)) else {
                continue;
            };
            if let Some(view) = tree
                .views
                .iter_mut()
                .find(|v| v.schema.as_deref() == Some(ks.as_str()) && v.name == name)
            {
                let base = row_str(&row, 2).unwrap_or_default();
                let where_clause = row_str(&row, 3).unwrap_or_default();
                view.definition = format!("SELECT * FROM {base} WHERE {where_clause}");
            }
        }

        // CQL user-defined functions (Java/JS snippets), read-only in the tree.
        let mut routines = Vec::new();
        for row in self
            .fetch(
                "SELECT keyspace_name, function_name, argument_names, argument_types, \
                        return_type, language, body \
                 FROM system_schema.functions",
            )
            .await
            .unwrap_or_default()
        {
            let (Some(ks), Some(name)) = (row_str(&row, 0), row_str(&row, 1)) else {
                continue;
            };
            if !keyspaces.contains(&ks) {
                continue;
            }
            let names = row_str_list(&row, 2);
            let types = row_str_list(&row, 3);
            let params = names
                .iter()
                .zip(types.iter())
                .map(|(n, t)| RoutineParam {
                    name: n.clone(),
                    data_type: t.clone(),
                    mode: ParamMode::In,
                    default: None,
                })
                .collect();
            routines.push(RoutineInfo {
                schema: Some(ks),
                name,
                kind: RoutineKind::Function,
                params,
                return_type: row_str(&row, 4),
                language: row_str(&row, 5).unwrap_or_default(),
                body: row_str(&row, 6).unwrap_or_default(),
            });
        }
        routines.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
        tree.routines = routines;

        Ok(tree)
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        self.run_capped(sql, max_rows, None).await
    }

    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: CancellationToken,
    ) -> Result<QueryResult> {
        self.run_capped(sql, max_rows, Some(cancel)).await
    }

    /// CQL has no transactions, so atomicity cannot be provided: statements run in order
    /// and the first failure stops the batch, reporting its position — statements before
    /// it have already been applied and stay applied.
    ///
    /// A `LOGGED BATCH` is deliberately *not* used here: servers reject batches above a
    /// small size threshold (~50 KB by default), which is exactly the shape imports —
    /// this method's main caller — produce. CQL writes are idempotent upserts, so
    /// re-running a partially applied batch converges rather than duplicating.
    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        for (i, stmt) in stmts.iter().enumerate() {
            self.session
                .query_unpaged(stmt.as_str(), &[])
                .await
                .map_err(|e| {
                    CoreError::Statement(i + 1, Box::new(CoreError::Cql(e.to_string())))
                })?;
        }
        Ok(stmts.len())
    }

    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        let mut statement = Statement::new(sql);
        statement.set_page_size(PAGE_SIZE);
        let pager = self
            .session
            .query_iter(statement, &[])
            .await
            .map_err(|e| CoreError::Cql(e.to_string()))?;
        let columns: Vec<ColumnMeta> = pager
            .column_specs()
            .iter()
            .map(|spec| ColumnMeta {
                name: spec.name().to_string(),
                type_name: type_name(spec.typ()),
            })
            .collect();
        sink.begin(&columns)?;
        let mut stream = pager
            .rows_stream::<Row>()
            .map_err(|e| CoreError::Cql(e.to_string()))?;
        let mut count = 0u64;
        while let Some(row) = stream.next().await {
            let row = row.map_err(|e| CoreError::Cql(e.to_string()))?;
            let values: Vec<Value> = row.columns.iter().map(decode).collect();
            sink.write_row(&values)?;
            count += 1;
        }
        sink.finish()?;
        Ok(count)
    }

    /// Keyspaces play the role of databases in the switcher.
    async fn list_databases(&self) -> Result<Vec<String>> {
        let mut keyspaces: Vec<String> = self
            .fetch("SELECT keyspace_name FROM system_schema.keyspaces")
            .await?
            .iter()
            .filter_map(|r| row_str(r, 0))
            .filter(|ks| !SYSTEM_KEYSPACES.contains(&ks.as_str()))
            .collect();
        keyspaces.sort();
        Ok(keyspaces)
    }
}

/// Read column `idx` of an introspection row as text (None when null or not text-shaped).
fn row_str(row: &Row, idx: usize) -> Option<String> {
    match row.columns.get(idx)?.as_ref()? {
        CqlValue::Text(s) | CqlValue::Ascii(s) => Some(s.clone()),
        _ => None,
    }
}

/// Read column `idx` as a list/frozen-list of text values (empty when null/other).
fn row_str_list(row: &Row, idx: usize) -> Vec<String> {
    match row.columns.get(idx).and_then(|c| c.as_ref()) {
        Some(CqlValue::List(items)) | Some(CqlValue::Set(items)) => items
            .iter()
            .filter_map(|v| v.as_text().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

/// Decode one cell into the app's [`Value`]. Scalars map directly; temporal types render
/// through chrono like the other backends; collections/UDTs/tuples render as CQL-literal
/// text (the grid shows them read-only, like JSON columns elsewhere).
fn decode(cell: &Option<CqlValue>) -> Value {
    let Some(cell) = cell else { return Value::Null };
    match cell {
        CqlValue::Boolean(b) => Value::Bool(*b),
        CqlValue::TinyInt(v) => Value::Int(*v as i64),
        CqlValue::SmallInt(v) => Value::Int(*v as i64),
        CqlValue::Int(v) => Value::Int(*v as i64),
        CqlValue::BigInt(v) => Value::Int(*v),
        CqlValue::Counter(c) => Value::Int(c.0),
        CqlValue::Float(v) => Value::Float(*v as f64),
        CqlValue::Double(v) => Value::Float(*v),
        CqlValue::Ascii(s) | CqlValue::Text(s) => Value::Text(s.clone()),
        CqlValue::Blob(b) => Value::Bytes(b.clone()),
        CqlValue::Uuid(u) => Value::Text(u.to_string()),
        CqlValue::Timeuuid(u) => Value::Text(u.to_string()),
        CqlValue::Inet(ip) => Value::Text(ip.to_string()),
        CqlValue::Date(d) => Value::Text(format_cql_date(d.0)),
        CqlValue::Time(t) => Value::Text(format_cql_time(t.0)),
        CqlValue::Timestamp(ts) => Value::Text(format_cql_timestamp(ts.0)),
        CqlValue::Duration(d) => Value::Text(format_cql_duration(d.months, d.days, d.nanoseconds)),
        CqlValue::Decimal(d) => {
            let (bytes, scale) = d.as_signed_be_bytes_slice_and_exponent();
            Value::Text(format_decimal(bytes, scale))
        }
        CqlValue::Varint(v) => Value::Text(format_varint(v.as_signed_bytes_be_slice())),
        // Legacy "empty" value of a typed column: distinct from null; closest is "".
        CqlValue::Empty => Value::Text(String::new()),
        other => Value::Text(display_cql(other)),
    }
}

/// Render a (possibly nested) CQL value as CQL-literal-style text, for collections, UDTs,
/// tuples, and vectors. Scalars inside recurse through [`decode`]'s formatting.
fn display_cql(value: &CqlValue) -> String {
    fn scalar(value: &CqlValue) -> String {
        match value {
            CqlValue::Ascii(s) | CqlValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
            other => match decode(&Some(other.clone())) {
                Value::Text(t) => t,
                v => v.display(),
            },
        }
    }
    match value {
        CqlValue::List(items) | CqlValue::Vector(items) => {
            let inner: Vec<String> = items.iter().map(render_nested).collect();
            format!("[{}]", inner.join(", "))
        }
        CqlValue::Set(items) => {
            let inner: Vec<String> = items.iter().map(render_nested).collect();
            format!("{{{}}}", inner.join(", "))
        }
        CqlValue::Map(entries) => {
            let inner: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", render_nested(k), render_nested(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        CqlValue::Tuple(items) => {
            let inner: Vec<String> = items
                .iter()
                .map(|v| match v {
                    Some(v) => render_nested(v),
                    None => "null".to_string(),
                })
                .collect();
            format!("({})", inner.join(", "))
        }
        CqlValue::UserDefinedType { fields, .. } => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(name, v)| match v {
                    Some(v) => format!("{name}: {}", render_nested(v)),
                    None => format!("{name}: null"),
                })
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        other => scalar(other),
    }
}

/// Inside a collection, nested collections recurse and scalars quote like CQL literals.
fn render_nested(value: &CqlValue) -> String {
    match value {
        CqlValue::List(_)
        | CqlValue::Vector(_)
        | CqlValue::Set(_)
        | CqlValue::Map(_)
        | CqlValue::Tuple(_)
        | CqlValue::UserDefinedType { .. } => display_cql(value),
        CqlValue::Ascii(s) | CqlValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
        other => match decode(&Some(other.clone())) {
            Value::Text(t) => t,
            Value::Null => "null".to_string(),
            v => v.display(),
        },
    }
}

/// CQL `date`: days since epoch, stored offset by 2^31.
fn format_cql_date(raw: u32) -> String {
    let days = raw as i64 - (1i64 << 31);
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|epoch| epoch.checked_add_signed(chrono::Duration::days(days)))
        .map(|d| d.to_string())
        .unwrap_or_else(|| format!("{days} days from epoch"))
}

/// CQL `time`: nanoseconds since midnight.
fn format_cql_time(nanos: i64) -> String {
    let secs = (nanos / 1_000_000_000) as u32;
    let nano = (nanos % 1_000_000_000) as u32;
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, nano)
        .map(|t| t.to_string())
        .unwrap_or_else(|| format!("{nanos}ns"))
}

/// CQL `timestamp`: milliseconds since the unix epoch, rendered as naive UTC (matching
/// how the other backends render their timestamps).
fn format_cql_timestamp(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| dt.naive_utc().to_string())
        .unwrap_or_else(|| format!("{millis}ms since epoch"))
}

/// CQL `duration` (months, days, nanoseconds), rendered in cqlsh's compact unit style.
fn format_cql_duration(months: i32, days: i32, nanos: i64) -> String {
    let mut out = String::new();
    let (years, months) = (months / 12, months % 12);
    let mut push = |n: i64, unit: &str| {
        if n != 0 {
            out.push_str(&format!("{n}{unit}"));
        }
    };
    push(years as i64, "y");
    push(months as i64, "mo");
    push(days as i64, "d");
    let (hours, rem) = (nanos / 3_600_000_000_000, nanos % 3_600_000_000_000);
    let (minutes, rem) = (rem / 60_000_000_000, rem % 60_000_000_000);
    let (seconds, rem) = (rem / 1_000_000_000, rem % 1_000_000_000);
    let (millis, rem) = (rem / 1_000_000, rem % 1_000_000);
    let (micros, ns) = (rem / 1_000, rem % 1_000);
    push(hours, "h");
    push(minutes, "m");
    push(seconds, "s");
    push(millis, "ms");
    push(micros, "us");
    push(ns, "ns");
    if out.is_empty() {
        out.push_str("0s");
    }
    out
}

/// Format a two's-complement big-endian integer of arbitrary width. Values wider than
/// 16 bytes (beyond i128 — astronomically rare) fall back to a hex rendering rather
/// than losing digits silently.
fn format_varint(bytes: &[u8]) -> String {
    match i128_from_be(bytes) {
        Some(v) => v.to_string(),
        None => format!("0x{}", hex(bytes)),
    }
}

/// Format a CQL `decimal`: `unscaled × 10^(-scale)` with the unscaled part as a
/// two's-complement big-endian integer.
fn format_decimal(bytes: &[u8], scale: i32) -> String {
    let Some(unscaled) = i128_from_be(bytes) else {
        return format!("0x{}e-{scale}", hex(bytes));
    };
    if scale <= 0 {
        // Non-positive scale multiplies by 10^|scale|: append zeros.
        let zeros = "0".repeat(scale.unsigned_abs() as usize);
        return format!("{unscaled}{zeros}");
    }
    let negative = unscaled < 0;
    let digits = unscaled.unsigned_abs().to_string();
    let scale = scale as usize;
    let with_point = if digits.len() > scale {
        let (int, frac) = digits.split_at(digits.len() - scale);
        format!("{int}.{frac}")
    } else {
        format!("0.{}{digits}", "0".repeat(scale - digits.len()))
    };
    if negative {
        format!("-{with_point}")
    } else {
        with_point
    }
}

fn i128_from_be(bytes: &[u8]) -> Option<i128> {
    if bytes.is_empty() {
        return Some(0);
    }
    if bytes.len() > 16 {
        return None;
    }
    let negative = bytes[0] & 0x80 != 0;
    let mut buf = [if negative { 0xff } else { 0x00 }; 16];
    buf[16 - bytes.len()..].copy_from_slice(bytes);
    Some(i128::from_be_bytes(buf))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Human-readable CQL type name for a result column (tooltips), e.g. `list<text>`.
fn type_name(typ: &ColumnType<'_>) -> String {
    match typ {
        ColumnType::Native(native) => native_type_name(native).to_string(),
        ColumnType::Collection { frozen, typ } => {
            let inner = match typ {
                CollectionType::List(t) => format!("list<{}>", type_name(t)),
                CollectionType::Set(t) => format!("set<{}>", type_name(t)),
                CollectionType::Map(k, v) => {
                    format!("map<{}, {}>", type_name(k), type_name(v))
                }
                _ => "collection".to_string(),
            };
            if *frozen {
                format!("frozen<{inner}>")
            } else {
                inner
            }
        }
        ColumnType::Vector { typ, dimensions } => {
            format!("vector<{}, {dimensions}>", type_name(typ))
        }
        ColumnType::UserDefinedType { definition, .. } => definition.name.to_string(),
        ColumnType::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(type_name).collect();
            format!("tuple<{}>", inner.join(", "))
        }
        _ => "unknown".to_string(),
    }
}

fn native_type_name(native: &NativeType) -> &'static str {
    match native {
        NativeType::Ascii => "ascii",
        NativeType::Boolean => "boolean",
        NativeType::Blob => "blob",
        NativeType::Counter => "counter",
        NativeType::Date => "date",
        NativeType::Decimal => "decimal",
        NativeType::Double => "double",
        NativeType::Duration => "duration",
        NativeType::Float => "float",
        NativeType::Int => "int",
        NativeType::BigInt => "bigint",
        NativeType::Text => "text",
        NativeType::Timestamp => "timestamp",
        NativeType::Inet => "inet",
        NativeType::SmallInt => "smallint",
        NativeType::TinyInt => "tinyint",
        NativeType::Time => "time",
        NativeType::Timeuuid => "timeuuid",
        NativeType::Uuid => "uuid",
        NativeType::Varint => "varint",
        _ => "unknown",
    }
}

/// TLS that encrypts but does not authenticate the server (Prefer/Require modes).
fn tls_config_no_verify() -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| CoreError::Pool(format!("tls setup: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerification(provider)))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// TLS with real verification against `ca_path` (or the system store when empty).
fn tls_config_verify(ca_path: &str) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    if ca_path.is_empty() {
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            // Skip unparsable store entries, same as every other rustls consumer.
            let _ = roots.add(cert);
        }
        if roots.is_empty() {
            return Err(CoreError::Pool(
                "no usable certificates in the system trust store".into(),
            ));
        }
    } else {
        use rustls_pki_types::pem::PemObject;
        let certs = rustls_pki_types::CertificateDer::pem_file_iter(ca_path)
            .map_err(|e| CoreError::Pool(format!("reading CA certificate {ca_path}: {e}")))?;
        let mut added = 0usize;
        for cert in certs {
            let cert =
                cert.map_err(|e| CoreError::Pool(format!("parsing CA certificate: {e}")))?;
            roots
                .add(cert)
                .map_err(|e| CoreError::Pool(format!("loading CA certificate: {e}")))?;
            added += 1;
        }
        if added == 0 {
            return Err(CoreError::Pool(format!(
                "no certificates found in {ca_path}"
            )));
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CoreError::Pool(format!("tls setup: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Accepts any server certificate while still verifying handshake signatures with the
/// ring provider. This is what SslMode::Prefer/Require mean across the app: encrypted,
/// unauthenticated — the verify modes are the authenticated ones.
#[derive(Debug)]
struct NoCertVerification(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_values_decode() {
        assert_eq!(decode(&None), Value::Null);
        assert_eq!(decode(&Some(CqlValue::Boolean(true))), Value::Bool(true));
        assert_eq!(decode(&Some(CqlValue::TinyInt(-3))), Value::Int(-3));
        assert_eq!(decode(&Some(CqlValue::SmallInt(300))), Value::Int(300));
        assert_eq!(decode(&Some(CqlValue::Int(70_000))), Value::Int(70_000));
        assert_eq!(
            decode(&Some(CqlValue::BigInt(i64::MAX))),
            Value::Int(i64::MAX)
        );
        assert_eq!(decode(&Some(CqlValue::Double(1.5))), Value::Float(1.5));
        assert_eq!(
            decode(&Some(CqlValue::Text("สวัสดี".into()))),
            Value::Text("สวัสดี".into())
        );
        assert_eq!(
            decode(&Some(CqlValue::Blob(vec![0, 255]))),
            Value::Bytes(vec![0, 255])
        );
        assert_eq!(decode(&Some(CqlValue::Empty)), Value::Text(String::new()));
    }

    #[test]
    fn temporal_values_render_like_other_backends() {
        // 2026-07-23 is 20657 days after the epoch.
        assert_eq!(
            decode(&Some(CqlValue::Date(scylla::value::CqlDate(
                (1u32 << 31) + 20_657
            )))),
            Value::Text("2026-07-23".into())
        );
        assert_eq!(
            decode(&Some(CqlValue::Time(scylla::value::CqlTime(
                (13 * 3600 + 30 * 60 + 5) * 1_000_000_000
            )))),
            Value::Text("13:30:05".into())
        );
        assert_eq!(
            decode(&Some(CqlValue::Timestamp(scylla::value::CqlTimestamp(0)))),
            Value::Text("1970-01-01 00:00:00".into())
        );
    }

    #[test]
    fn durations_render_compact() {
        assert_eq!(format_cql_duration(14, 3, 3_601_000_000_000), "1y2mo3d1h1s");
        assert_eq!(format_cql_duration(0, 0, 0), "0s");
        assert_eq!(format_cql_duration(0, 0, 1_500_000), "1ms500us");
    }

    #[test]
    fn decimals_and_varints_format_exactly() {
        // 12345 × 10^-2 = 123.45
        assert_eq!(format_decimal(&12345i32.to_be_bytes(), 2), "123.45");
        // -5 × 10^-3 = -0.005
        assert_eq!(format_decimal(&(-5i8).to_be_bytes(), 3), "-0.005");
        // 7 × 10^2 = 700 (negative scale appends zeros)
        assert_eq!(format_decimal(&7i8.to_be_bytes(), -2), "700");
        assert_eq!(format_varint(&(-1i8).to_be_bytes()), "-1");
        assert_eq!(
            format_varint(&i128::MAX.to_be_bytes()),
            i128::MAX.to_string()
        );
        // Wider than i128: hex fallback, never silent truncation.
        assert_eq!(format_varint(&[1u8; 17]).starts_with("0x"), true);
    }

    #[test]
    fn collections_render_as_cql_literals() {
        let list = CqlValue::List(vec![CqlValue::Int(1), CqlValue::Int(2)]);
        assert_eq!(decode(&Some(list)), Value::Text("[1, 2]".into()));
        let set = CqlValue::Set(vec![CqlValue::Text("a'b".into())]);
        assert_eq!(decode(&Some(set)), Value::Text("{'a''b'}".into()));
        let map = CqlValue::Map(vec![(CqlValue::Text("k".into()), CqlValue::Int(9))]);
        assert_eq!(decode(&Some(map)), Value::Text("{'k': 9}".into()));
        let udt = CqlValue::UserDefinedType {
            keyspace: "ks".into(),
            name: "addr".into(),
            fields: vec![
                ("street".into(), Some(CqlValue::Text("x".into()))),
                ("zip".into(), None),
            ],
        };
        assert_eq!(
            decode(&Some(udt)),
            Value::Text("{street: 'x', zip: null}".into())
        );
        let tuple = CqlValue::Tuple(vec![Some(CqlValue::Int(1)), None]);
        assert_eq!(decode(&Some(tuple)), Value::Text("(1, null)".into()));
    }
}
