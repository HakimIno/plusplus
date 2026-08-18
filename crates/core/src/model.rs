//! Plain data structures shared across the app: connection configs, schema metadata,
//! and query results. None of these depend on a specific backend.

use serde::{Deserialize, Serialize};

/// Which database backend a connection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbKind {
    Postgres,
    MySql,
    MariaDb,
    SqlServer,
    Sqlite,
    /// Embedded analytical database, backed by a local file or an in-memory catalog.
    DuckDb,
    /// Apache Cassandra, spoken over the CQL native protocol.
    Cassandra,
    /// ScyllaDB — wire-compatible with Cassandra; both share the CQL backend.
    ScyllaDb,
}

impl DbKind {
    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL",
            DbKind::MariaDb => "MariaDB",
            DbKind::SqlServer => "SQL Server",
            DbKind::Sqlite => "SQLite",
            DbKind::DuckDb => "DuckDB",
            DbKind::Cassandra => "Cassandra",
            DbKind::ScyllaDb => "ScyllaDB",
        }
    }

    /// Whether this backend speaks CQL (Cassandra / ScyllaDB) rather than SQL. CQL looks
    /// like SQL but has no joins, no transactions, and single-row `INSERT` only, so a few
    /// SQL-generic paths branch on this.
    pub fn is_cql(self) -> bool {
        matches!(self, DbKind::Cassandra | DbKind::ScyllaDb)
    }

    /// Whether this backend authenticates with a server (host/port/user/password)
    /// versus a local file path.
    pub fn is_server(self) -> bool {
        !matches!(self, DbKind::Sqlite | DbKind::DuckDb)
    }

    /// Whether this backend can present a client certificate (mutual TLS).
    /// tiberius hardcodes no-client-auth, so SQL Server can't; SQLite has no TLS at all.
    pub fn supports_client_cert(self) -> bool {
        matches!(self, DbKind::Postgres | DbKind::MySql | DbKind::MariaDb)
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::MySql | DbKind::MariaDb => 3306,
            DbKind::SqlServer => 1433,
            DbKind::Sqlite => 0,
            DbKind::DuckDb => 0,
            DbKind::Cassandra | DbKind::ScyllaDb => 9042,
        }
    }

    /// Build a "preview the first `limit` rows" query for `qualified_table` in this dialect.
    /// SQL Server has no `LIMIT`; it caps rows with `TOP` instead.
    pub fn preview_query(self, qualified_table: &str, limit: u32) -> String {
        match self {
            DbKind::SqlServer => format!("SELECT TOP {limit} * FROM {qualified_table};"),
            _ => format!("SELECT * FROM {qualified_table} LIMIT {limit};"),
        }
    }

    /// Quote a table/column identifier for this dialect. MySQL/MariaDB use backticks; the
    /// rest use ANSI double quotes. Embedded quote characters are doubled to neutralise them.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            DbKind::MySql | DbKind::MariaDb => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// How many row tuples one `INSERT ... VALUES` may carry. SQL Server rejects more than
    /// 1000 outright; CQL has no multi-row `VALUES` at all, so each statement holds exactly
    /// one row there; the others have no fixed row limit, so the cap is a conservative
    /// batch size that keeps a statement well inside MySQL's `max_allowed_packet`.
    pub fn max_insert_rows(self) -> usize {
        match self {
            DbKind::SqlServer => 1000,
            DbKind::Cassandra | DbKind::ScyllaDb => 1,
            _ => 500,
        }
    }
}

mod catalog;
mod connection;
mod ddl;
mod result;
mod sql;

pub use catalog::*;
pub use connection::*;
pub use ddl::*;
pub use result::*;
pub use sql::*;

#[cfg(test)]
mod tests;
