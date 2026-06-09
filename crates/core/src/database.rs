//! The backend abstraction. Every supported database implements [`Database`];
//! the rest of the app only ever holds an `Arc<dyn Database>`.

use async_trait::async_trait;

use crate::error::Result;
use crate::model::{DbKind, QueryResult, SchemaTree};

/// A live connection to a database backend.
///
/// Implementors wrap a connection pool and are cheap to clone behind an `Arc`. All methods
/// are async and run on the tokio runtime; the UI never calls them on its own thread.
#[async_trait]
pub trait Database: Send + Sync {
    /// Which backend this is.
    fn kind(&self) -> DbKind;

    /// Introspect the connected database into a [`SchemaTree`] (tables, columns, indexes).
    async fn introspect(&self) -> Result<SchemaTree>;

    /// Execute an arbitrary SQL statement and return the result set. For DML statements
    /// the returned rows are empty and `stats.rows_affected` is populated instead.
    async fn execute(&self, sql: &str) -> Result<QueryResult>;
}

/// Heuristic: does this statement return rows? Used to pick `fetch_all` vs `execute`.
pub(crate) fn returns_rows(sql: &str) -> bool {
    let head = sql.trim_start().trim_start_matches('(').trim_start();
    let first = head
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "select"
            | "with"
            | "show"
            | "describe"
            | "desc"
            | "pragma"
            | "explain"
            | "values"
            | "table"
    )
}
