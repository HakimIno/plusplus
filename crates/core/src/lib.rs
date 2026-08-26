//! `core` — the backend-agnostic data layer for plusplus.
//!
//! This crate knows nothing about egui. It exposes:
//! - [`Database`]: the trait every backend implements.
//! - [`connect`]: a factory that returns an `Arc<dyn Database>` for a [`ConnectionConfig`].
//! - [`model`]: plain data types (configs, schema metadata, query results).
//! - [`config`] / [`secrets`]: persistence of connections (JSON) and passwords (keychain).
//!
//! Everything here is testable without a window (see the tests at the bottom of this file).

pub mod audit;
pub mod backends;
pub mod bookmarks;
pub mod clipboard;
pub mod coerce;
pub mod config;
mod connection;
pub mod database;
pub mod erd;
pub mod error;
pub mod export;
pub mod favorites;
pub mod history;
pub mod import;
pub mod model;
pub mod parameters;
pub mod safety;
pub mod secrets;
pub mod syntax;
pub mod tunnel;
pub mod value;

pub use bookmarks::Bookmark;
pub use clipboard::{copy_rows, CopyFormat};
pub use coerce::{CoerceError, EditorKind};
pub use connection::connect;
pub use database::{returns_rows as query_returns_rows, Database};
pub use erd::{DesignColumn, DesignForeignKey, DesignIndex, DesignTable, ErDesign};
pub use error::{CoreError, Result};
pub use export::{ExportFormat, RowSink};
pub use favorites::Favorite;
pub use import::{ImportFormat, Preview, Record, Target};
pub use model::{
    build_add_column_sql, build_add_fk_sql, build_alter_column_sql, build_clone_table_sql,
    build_count_sql, build_create_index_sql, build_create_routine_sql, build_create_table_sql,
    build_create_trigger_sql, build_create_view_sql, build_delete_sql, build_drop_column_sql,
    build_drop_fk_sql, build_drop_index_sql, build_drop_routine_sql, build_drop_table_sql,
    build_drop_trigger_sql, build_drop_view_sql, build_insert_sql, build_rename_column_sql,
    build_select_where_sql, build_truncate_table_sql, build_update_sql, parse_page_window,
    parse_trigger_header, routine_supports_replace, select_body_after_as, simple_select_target,
    view_supports_replace, with_keyset_page, with_page_window, with_where_predicate, ColumnDef,
    ColumnInfo, ColumnMeta, ConnectionColor, ConnectionConfig, ConnectionIcon, DbKind, FkAction,
    ForeignKeyDef, ForeignKeyInfo, IndexDef, IndexInfo, PageWindow, ParamMode, QueryResult,
    QueryStats, RoutineBuild, RoutineInfo, RoutineKind, RoutineParam, SafetyProfile, SchemaTree,
    SslMode, TableInfo, TriggerBuild, TriggerEvent, TriggerInfo, TriggerLevel, TriggerTiming,
    ViewInfo,
};
pub use parameters::{query_parameter_names, resolve_query_parameters, ParameterError};
pub use syntax::{check_syntax, SyntaxError};
pub use value::Value;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_sorting_handles_nulls_and_numbers() {
        let mut v = [Value::Int(3), Value::Null, Value::Int(1), Value::Float(2.0)];
        v.sort_by(|a, b| a.sort_cmp(b));
        assert_eq!(v[0], Value::Int(1));
        assert_eq!(v[1], Value::Float(2.0));
        assert_eq!(v[2], Value::Int(3));
        assert!(v[3].is_null()); // NULL sorts last
    }

    #[test]
    fn build_update_escapes_and_quotes_per_dialect() {
        use model::build_update_sql;

        // Postgres: ANSI double-quoted identifiers, doubled single-quotes in strings.
        let sql = build_update_sql(
            DbKind::Postgres,
            Some("public"),
            "users",
            &[
                ("name", &Value::Text("O'Brien".into())),
                ("age", &Value::Int(30)),
            ],
            &[("id", &Value::Int(7))],
        )
        .unwrap();
        assert_eq!(
            sql,
            r#"UPDATE "public"."users" SET "name" = 'O''Brien', "age" = 30 WHERE "id" = 7;"#
        );

        // MySQL: backtick identifiers, and backslashes are escaped in strings.
        let sql = build_update_sql(
            DbKind::MySql,
            None,
            "logs",
            &[("path", &Value::Text(r"C:\tmp".into()))],
            &[("id", &Value::Int(1))],
        )
        .unwrap();
        assert_eq!(sql, r"UPDATE `logs` SET `path` = 'C:\\tmp' WHERE `id` = 1;");

        // A NULL key matches with IS NULL, and binary SET values are rejected.
        let sql = build_update_sql(
            DbKind::Sqlite,
            None,
            "t",
            &[("v", &Value::Null)],
            &[("k", &Value::Null)],
        )
        .unwrap();
        assert_eq!(sql, r#"UPDATE "t" SET "v" = NULL WHERE "k" IS NULL;"#);
        assert!(build_update_sql(
            DbKind::Sqlite,
            None,
            "t",
            &[("v", &Value::Bytes(vec![1, 2]))],
            &[("k", &Value::Int(1))]
        )
        .is_none());
    }

    /// Coerce every record of `path` against `targets` and insert them in one transaction —
    /// exactly what the UI's `run_import` does, minus the progress messages.

    #[test]
    fn returns_rows_classifies_statements() {
        use database::returns_rows;
        assert!(returns_rows("SELECT 1"));
        assert!(returns_rows("  with x as (select 1) select * from x"));
        assert!(returns_rows("DESCRIBE users"));
        assert!(returns_rows("PRAGMA table_info(t)"));
        assert!(!returns_rows("INSERT INTO t VALUES (1)"));
        assert!(!returns_rows("update t set a = 1"));
    }
}
