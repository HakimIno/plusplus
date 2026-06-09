//! `analysis` — Polars-based analysis of result sets.
//!
//! Phase 2 will turn a [`core::QueryResult`] into a Polars `DataFrame` and provide
//! descriptive statistics, group-by/aggregation, and chart-ready series. For now this is
//! an intentional placeholder so the workspace builds and the dependency graph is in place.

/// Marker that the analysis layer is wired into the workspace. Replaced by real APIs
/// (`to_dataframe`, `describe`, `group_by`) in Phase 2.
pub fn placeholder() {}
