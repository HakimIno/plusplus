//! SQL beautifier behind the editor's Beautify (Cmd/Ctrl+I) action.
//!
//! Wraps the token-based `sqlformat` crate, mapping the active connection's [`DbKind`]
//! to the matching tokenizer dialect so each backend's special syntax — `[bracketed
//! identifiers]` and `@variables` on SQL Server, array subscripts and `$n` placeholders
//! on PostgreSQL, backticks on MySQL/MariaDB — survives formatting untouched. Being
//! token-based (not a parser) it never fails: SQL it doesn't understand is reflowed
//! conservatively, and comments are preserved.

use dbcore::model::DbKind;
use sqlformat::{Dialect, FormatOptions, Indent, QueryParams};

/// User-tunable beautifier preferences (persisted in settings.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeautifyPrefs {
    /// Convert reserved keywords to ALL CAPS.
    pub uppercase: bool,
    /// Indent width, in spaces.
    pub indent: u8,
}

impl Default for BeautifyPrefs {
    fn default() -> Self {
        Self {
            uppercase: true,
            indent: 2,
        }
    }
}

/// Tokenizer dialect for a backend. `None` (no live connection) formats as generic SQL.
/// MySQL/MariaDB/SQLite have no dialect-specific tokens beyond what Generic handles
/// (backticks and standard quoting are always recognised).
fn dialect(kind: Option<DbKind>) -> Dialect {
    match kind {
        Some(DbKind::Postgres) => Dialect::PostgreSql,
        Some(DbKind::SqlServer) => Dialect::SQLServer,
        // CQL keywords are a rough subset of SQL's; Generic formats it acceptably.
        Some(
            DbKind::MySql
            | DbKind::MariaDb
            | DbKind::Sqlite
            | DbKind::Cassandra
            | DbKind::ScyllaDb,
        )
        | None => Dialect::Generic,
    }
}

/// Reformat `sql` for readability in the dialect of `kind`. Whitespace-only and
/// keyword-case changes only — identifiers, literals, placeholders, and comments are
/// preserved exactly, so the query's meaning never changes.
pub fn beautify(sql: &str, kind: Option<DbKind>, prefs: BeautifyPrefs) -> String {
    let options = FormatOptions {
        indent: Indent::Spaces(prefs.indent),
        uppercase: Some(prefs.uppercase),
        dialect: dialect(kind),
        ..FormatOptions::default()
    };
    sqlformat::format(sql, &QueryParams::None, &options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> BeautifyPrefs {
        BeautifyPrefs::default()
    }

    #[test]
    fn breaks_clauses_and_uppercases_keywords() {
        let out = beautify(
            "select id, name from users where id = 1 order by name",
            Some(DbKind::Sqlite),
            prefs(),
        );
        assert_eq!(
            out,
            "SELECT\n  id,\n  name\nFROM\n  users\nWHERE\n  id = 1\nORDER BY\n  name"
        );
    }

    #[test]
    fn respects_lowercase_and_indent_prefs() {
        let out = beautify(
            "SELECT a FROM t",
            None,
            BeautifyPrefs {
                uppercase: false,
                indent: 4,
            },
        );
        assert_eq!(out, "select\n    a\nfrom\n    t");
    }

    #[test]
    fn preserves_mysql_backticks_and_string_case() {
        let out = beautify(
            "select `from` from `Order Items` where note = 'select me'",
            Some(DbKind::MySql),
            prefs(),
        );
        assert!(out.contains("`from`"), "backticked ident changed: {out}");
        assert!(out.contains("`Order Items`"), "ident case changed: {out}");
        assert!(out.contains("'select me'"), "string literal changed: {out}");
    }

    #[test]
    fn preserves_sqlserver_brackets_and_variables() {
        let out = beautify(
            "select top 10 [Order Details].[Unit Price], @total from [Order Details]",
            Some(DbKind::SqlServer),
            prefs(),
        );
        assert!(out.contains("[Order Details]"), "brackets broken: {out}");
        assert!(out.contains("[Unit Price]"), "brackets broken: {out}");
        assert!(out.contains("@total"), "variable broken: {out}");
    }

    #[test]
    fn preserves_postgres_placeholders_and_casts() {
        let out = beautify(
            "select id::text, tags[1] from items where id = $1",
            Some(DbKind::Postgres),
            prefs(),
        );
        assert!(out.contains("id::text"), "cast broken: {out}");
        assert!(out.contains("tags[1]"), "array subscript broken: {out}");
        assert!(out.contains("$1"), "placeholder broken: {out}");
    }

    #[test]
    fn preserves_comments() {
        let out = beautify(
            "-- top of query\nselect 1 /* inline */ from t",
            None,
            prefs(),
        );
        assert!(out.contains("-- top of query"), "line comment lost: {out}");
        assert!(out.contains("/* inline */"), "block comment lost: {out}");
    }
}
