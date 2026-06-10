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

/// First keywords that mark a statement as row-returning, for the common SQL dialects.
pub(crate) const ROW_KEYWORDS: &[&str] = &[
    "select", "with", "show", "describe", "desc", "pragma", "explain", "values", "table",
];

/// Heuristic: does this SQL batch return rows? Used to pick `fetch_all` vs `execute`.
///
/// A batch may chain several statements with `;` — SQL Server in particular loves
/// `SET NOCOUNT ON; SELECT ...` and `DECLARE @x ...; SELECT @x`. Looking at only the first
/// keyword would misclassify those as non-row-returning DML, so we scan every statement and
/// return `true` if *any* of them is row-returning. For a single statement (the common case)
/// this is identical to matching the leading keyword.
pub(crate) fn returns_rows(sql: &str) -> bool {
    statements_return_rows(sql, ROW_KEYWORDS)
}

/// Like [`returns_rows`] but with a caller-supplied set of row-returning leading keywords,
/// so dialects can extend it (e.g. SQL Server treats `EXEC`/`EXECUTE` as row-returning).
pub(crate) fn statements_return_rows(sql: &str, keywords: &[&str]) -> bool {
    split_statements(sql)
        .iter()
        .any(|stmt| statement_returns_rows(stmt, keywords))
}

/// Does a single statement start with one of `keywords`? Leading whitespace, comments, and
/// `(` (as in a parenthesised `SELECT`) are skipped before reading the first keyword.
fn statement_returns_rows(stmt: &str, keywords: &[&str]) -> bool {
    let head = skip_leading_noise(stmt);
    let first = head
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    keywords.contains(&first.as_str())
}

/// Strip leading whitespace, line/block comments, and `(` from a statement so the next token
/// is its first real keyword.
fn skip_leading_noise(stmt: &str) -> &str {
    let mut s = stmt.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix('(') {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("--") {
            s = rest
                .split_once('\n')
                .map(|(_, r)| r)
                .unwrap_or("")
                .trim_start();
        } else if s.starts_with("/*") {
            s = skip_block_comment(s).trim_start();
        } else {
            return s;
        }
    }
}

/// Consume a leading `/* ... */` block comment (T-SQL allows nesting) and return the rest.
/// `s` must start with `/*`. If the comment is unterminated, returns `""`.
fn skip_block_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 2; // past the opening "/*"
    let mut depth = 1u32;
    while i + 1 < bytes.len() && depth > 0 {
        match (bytes[i], bytes[i + 1]) {
            (b'/', b'*') => {
                depth += 1;
                i += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                i += 2;
            }
            _ => i += 1,
        }
    }
    if depth == 0 {
        &s[i..]
    } else {
        ""
    }
}

/// Split a SQL batch into its individual statements on top-level `;`, ignoring semicolons
/// inside string literals, quoted identifiers, and comments. Returned slices are trimmed and
/// empty statements are dropped. This is a lexical split only — it does not parse or validate
/// SQL — but it's robust enough to classify the statements in a batch.
pub(crate) fn split_statements(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    // All delimiters we care about are ASCII, so byte scanning never lands inside a
    // multi-byte UTF-8 sequence: continuation bytes fall through the `_` arm untouched.
    while i < n {
        match bytes[i] {
            // String literal or quoted identifier; a doubled quote escapes the delimiter.
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < n {
                    if bytes[i] == quote {
                        if i + 1 < n && bytes[i + 1] == quote {
                            i += 2; // escaped quote, stay inside the literal
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // SQL Server bracket identifier; `]]` escapes a literal `]`.
            b'[' => {
                i += 1;
                while i < n {
                    if bytes[i] == b']' {
                        if i + 1 < n && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // Line comment, runs to end of line.
            b'-' if i + 1 < n && bytes[i + 1] == b'-' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment, which T-SQL allows to nest.
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                let mut depth = 1u32;
                while i < n && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b';' => {
                statements.push(sql[start..i].trim());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < n {
        statements.push(sql[start..].trim());
    }
    statements.retain(|s| !s.is_empty());
    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_statement_matches_leading_keyword() {
        assert!(returns_rows("SELECT * FROM t"));
        assert!(returns_rows("  with cte as (select 1) select * from cte"));
        assert!(returns_rows("(SELECT 1)"));
        assert!(!returns_rows("INSERT INTO t VALUES (1)"));
        assert!(!returns_rows("UPDATE t SET a = 1"));
        assert!(!returns_rows("CREATE TABLE t (id int)"));
    }

    #[test]
    fn batch_returns_rows_if_any_statement_does() {
        // The SSMS-style prelude that previously got misrouted to the DML path.
        assert!(returns_rows("SET NOCOUNT ON; SELECT * FROM t"));
        assert!(returns_rows("DECLARE @x INT = 1; SELECT @x"));
        // Trailing SELECT after DML still counts.
        assert!(returns_rows(
            "INSERT INTO t VALUES (1); SELECT SCOPE_IDENTITY()"
        ));
        // Pure DML/DDL batch: still routed to execute() so we report rows affected.
        assert!(!returns_rows(
            "INSERT INTO t VALUES (1); UPDATE t SET a = 2"
        ));
        assert!(!returns_rows("SET NOCOUNT ON; DELETE FROM t"));
    }

    #[test]
    fn semicolons_inside_literals_do_not_split() {
        // The `;` lives in a string, so this is a single non-row-returning statement.
        assert!(!returns_rows("INSERT INTO t VALUES ('a;b')"));
        // A keyword that only appears inside a literal must not count.
        assert!(!returns_rows("INSERT INTO t VALUES ('select me')"));
        // ...but a real SELECT that merely contains a `;` literal still counts.
        assert!(returns_rows("SELECT ';' AS sep"));
        // Bracket identifier containing a semicolon.
        assert_eq!(split_statements("SELECT * FROM [a;b]").len(), 1);
    }

    #[test]
    fn keywords_inside_comments_are_ignored() {
        assert!(!returns_rows("-- SELECT 1\nINSERT INTO t VALUES (1)"));
        assert!(!returns_rows("/* SELECT 1 */ INSERT INTO t VALUES (1)"));
        assert!(!returns_rows(
            "/* a /* nested */ SELECT */ INSERT INTO t VALUES (1)"
        ));
        // A comment before a real SELECT must not hide it.
        assert!(returns_rows("-- comment\nSELECT 1"));
    }

    #[test]
    fn split_drops_empty_statements_and_trims() {
        assert_eq!(split_statements("SELECT 1;"), vec!["SELECT 1"]);
        assert_eq!(split_statements("  ; SELECT 1 ; ;"), vec!["SELECT 1"]);
        assert!(split_statements("   ").is_empty());
    }

    #[test]
    fn extra_keywords_extend_the_set() {
        // SQL Server routes EXEC to the row path; the base set does not.
        let mssql = &["select", "exec", "execute"];
        assert!(statements_return_rows("EXEC sp_helpindex 'dbo.t'", mssql));
        assert!(!returns_rows("EXEC sp_helpindex 'dbo.t'"));
    }
}
