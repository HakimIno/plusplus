//! Backend implementations of the [`crate::Database`] trait.

pub mod cassandra;
pub mod duckdb;
pub mod mssql;
pub mod mysql;
pub mod postgres;
pub mod sqlite;

/// Avoid the repeated small reallocations at the start of a result fetch without reserving
/// the full 100k-row safety cap for the common case where a query returns only a few rows.
pub(super) fn initial_row_capacity(max_rows: usize) -> usize {
    max_rows.min(1_024)
}
