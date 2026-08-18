//! Query result metadata and materialized row values.

use crate::value::Value;

/// Metadata for a single result-set column.
#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub name: String,
    /// The backend's native type name (best-effort), shown in tooltips.
    pub type_name: String,
}

/// Stats about a single query execution.
#[derive(Debug, Clone, Default)]
pub struct QueryStats {
    /// Wall-clock execution time in milliseconds.
    pub elapsed_ms: f64,
    /// Rows affected for DML statements (INSERT/UPDATE/DELETE). `None` for SELECTs.
    pub rows_affected: Option<u64>,
}

/// A complete result set: column metadata plus rows of [`Value`]s.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub stats: QueryStats,
    /// The fetch stopped at the caller's row cap; the server had more rows to give.
    pub truncated: bool,
}

impl QueryResult {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Approximate owned memory used by this materialized result. This intentionally counts
    /// vector and string capacities so the UI can enforce a conservative cross-tab budget.
    pub fn estimated_memory_bytes(&self) -> usize {
        let columns = self.columns.capacity() * std::mem::size_of::<ColumnMeta>()
            + self
                .columns
                .iter()
                .map(|column| column.name.capacity() + column.type_name.capacity())
                .sum::<usize>();
        let rows = self.rows.capacity() * std::mem::size_of::<Vec<Value>>()
            + self
                .rows
                .iter()
                .map(|row| {
                    let inline_values = row.capacity() * std::mem::size_of::<Value>();
                    let heap_values = row
                        .iter()
                        .map(|value| {
                            value
                                .estimated_memory_bytes()
                                .saturating_sub(std::mem::size_of::<Value>())
                        })
                        .sum::<usize>();
                    inline_values + heap_values
                })
                .sum::<usize>();
        std::mem::size_of::<Self>() + columns + rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_memory_estimate_grows_with_owned_cell_buffers() {
        let small = QueryResult {
            columns: vec![ColumnMeta {
                name: "value".into(),
                type_name: "TEXT".into(),
            }],
            rows: vec![vec![Value::Text("x".into())]],
            ..QueryResult::default()
        };
        let large = QueryResult {
            columns: small.columns.clone(),
            rows: vec![vec![Value::Text("x".repeat(16_384))]],
            ..QueryResult::default()
        };
        assert!(large.estimated_memory_bytes() >= small.estimated_memory_bytes() + 16_000);
    }
}
