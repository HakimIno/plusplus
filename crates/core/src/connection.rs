//! Database connection factory and SSH-backed connection wrapper.

use crate::backends::{
    cassandra::CassandraDb, mssql::MsSqlDb, mysql::MySqlDb, postgres::PostgresDb, sqlite::SqliteDb,
};

use std::sync::Arc;

use crate::database::Database;
use crate::error::{CoreError, Result};
use crate::model::{ConnectionConfig, DbKind, QueryResult, SchemaTree};
use crate::{export, tunnel};

/// Connect to the database described by `cfg`, returning a shareable handle.
///
/// `password` and `ssh_secret` are the secrets fetched from the OS keychain by the
/// caller (or `None` for passwordless / file-based connections). With `ssh_enabled`,
/// an SSH tunnel to the bastion is opened first and the backend connects through it;
/// the tunnel lives exactly as long as the returned handle. Adding a new backend means
/// adding a match arm to [`connect_direct`] and an implementation in [`crate::backends`].
pub async fn connect(
    cfg: &ConnectionConfig,
    password: Option<String>,
    ssh_secret: Option<String>,
) -> Result<Arc<dyn Database>> {
    if cfg.ssh_enabled && cfg.kind.is_server() {
        let tun = tunnel::SshTunnel::open(cfg, ssh_secret.as_deref()).await?;
        // The driver dials the tunnel's loopback end instead of the real host. Note for
        // TLS: verify-full then checks the certificate against the *original* hostname
        // only if the driver pins it — with a tunnel, prefer verify-ca.
        let mut local = cfg.clone();
        local.host = "127.0.0.1".to_string();
        local.port = tun.local_port;
        let inner = connect_direct(&local, password, true).await?;
        return Ok(Arc::new(Tunneled {
            inner,
            _tunnel: tun,
        }));
    }
    connect_direct(cfg, password, false).await
}

/// Connect straight to `cfg.host:cfg.port` (or the SQLite file) with no tunnel.
/// `via_tunnel` tells cluster-discovering backends (Cassandra/ScyllaDB) that the host is
/// a tunnel endpoint, so peer addresses the cluster broadcasts must not be dialed directly.
async fn connect_direct(
    cfg: &ConnectionConfig,
    password: Option<String>,
    via_tunnel: bool,
) -> Result<Arc<dyn Database>> {
    match cfg.kind {
        DbKind::Postgres => Ok(Arc::new(PostgresDb::connect(cfg, password).await?)),
        DbKind::MySql | DbKind::MariaDb => Ok(Arc::new(MySqlDb::connect(cfg, password).await?)),
        DbKind::SqlServer => Ok(Arc::new(MsSqlDb::connect(cfg, password).await?)),
        DbKind::Cassandra | DbKind::ScyllaDb => Ok(Arc::new(
            CassandraDb::connect(cfg, password, via_tunnel).await?,
        )),
        DbKind::Sqlite => {
            if cfg.sqlite_path.trim().is_empty() {
                return Err(CoreError::InvalidConfig("SQLite path is empty".into()));
            }
            Ok(Arc::new(SqliteDb::connect(cfg).await?))
        }
    }
}

/// A backend riding an SSH tunnel: delegates everything and keeps the tunnel alive for
/// as long as the connection itself.
struct Tunneled {
    inner: Arc<dyn Database>,
    _tunnel: tunnel::SshTunnel,
}

#[async_trait::async_trait]
impl Database for Tunneled {
    fn kind(&self) -> DbKind {
        self.inner.kind()
    }
    async fn introspect(&self) -> Result<SchemaTree> {
        self.inner.introspect().await
    }
    async fn introspect_overview(&self) -> Result<SchemaTree> {
        self.inner.introspect_overview().await
    }
    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        self.inner.execute_capped(sql, max_rows).await
    }
    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<QueryResult> {
        self.inner
            .execute_capped_cancellable(sql, max_rows, cancel)
            .await
    }
    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        self.inner.execute_transaction(stmts).await
    }
    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn export::RowSink + Send),
    ) -> Result<u64> {
        self.inner.export_query(sql, sink).await
    }
    async fn export_query_cancellable(
        &self,
        sql: &str,
        cancel: tokio_util::sync::CancellationToken,
        sink: &mut (dyn export::RowSink + Send),
    ) -> Result<u64> {
        self.inner.export_query_cancellable(sql, cancel, sink).await
    }
    async fn list_databases(&self) -> Result<Vec<String>> {
        self.inner.list_databases().await
    }
}
