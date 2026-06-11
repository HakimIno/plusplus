//! Heuristics for spotting destructive SQL before it runs. The UI uses these to gate
//! queries on connections marked as production behind a confirmation dialog.

use crate::database::{skip_leading_noise, split_statements};

/// The destructive statement classes worth confirming before they touch production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerKind {
    Update,
    Delete,
    Drop,
    Truncate,
    Alter,
}

impl DangerKind {
    pub fn label(self) -> &'static str {
        match self {
            DangerKind::Update => "UPDATE",
            DangerKind::Delete => "DELETE",
            DangerKind::Drop => "DROP",
            DangerKind::Truncate => "TRUNCATE",
            DangerKind::Alter => "ALTER",
        }
    }
}

/// One destructive statement found in a batch.
#[derive(Debug, Clone)]
pub struct DangerousStatement {
    pub kind: DangerKind,
    /// An `UPDATE`/`DELETE` with no `WHERE` anywhere in it — it touches every row.
    /// Lexical check only: a `WHERE` inside a subquery also counts, so this can
    /// under-warn on exotic statements, but it never warns on a properly scoped one.
    pub missing_where: bool,
    /// The statement text (trimmed), for display in the confirmation dialog.
    pub sql: String,
}

/// Scan a SQL batch and return every destructive statement in it, in order.
/// Empty means the batch is safe to run without confirmation.
pub fn dangerous_statements(sql: &str) -> Vec<DangerousStatement> {
    split_statements(sql)
        .into_iter()
        .filter_map(|stmt| {
            let head = skip_leading_noise(stmt);
            let first = head
                .split(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let kind = match first.as_str() {
                "update" => DangerKind::Update,
                "delete" => DangerKind::Delete,
                "drop" => DangerKind::Drop,
                "truncate" => DangerKind::Truncate,
                "alter" => DangerKind::Alter,
                _ => return None,
            };
            let missing_where = matches!(kind, DangerKind::Update | DangerKind::Delete)
                && !contains_keyword(stmt, "where");
            Some(DangerousStatement {
                kind,
                missing_where,
                sql: stmt.to_string(),
            })
        })
        .collect()
}

/// Does `stmt` contain `keyword` as a standalone word, outside string literals, quoted
/// identifiers, and comments? Case-insensitive; `keyword` must be ASCII lowercase.
fn contains_keyword(stmt: &str, keyword: &str) -> bool {
    let bytes = stmt.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        match bytes[i] {
            // String literal or quoted identifier; a doubled quote escapes the delimiter.
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < n {
                    if bytes[i] == quote {
                        if i + 1 < n && bytes[i + 1] == quote {
                            i += 2;
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
            // Block comment (T-SQL allows nesting).
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
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if stmt[start..i].eq_ignore_ascii_case(keyword) {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<DangerKind> {
        dangerous_statements(sql).iter().map(|d| d.kind).collect()
    }

    #[test]
    fn selects_are_safe() {
        assert!(dangerous_statements("SELECT * FROM users").is_empty());
        assert!(dangerous_statements("WITH c AS (SELECT 1) SELECT * FROM c").is_empty());
        assert!(dangerous_statements("INSERT INTO t VALUES (1)").is_empty());
    }

    #[test]
    fn classifies_each_destructive_kind() {
        assert_eq!(kinds("UPDATE t SET a = 1 WHERE id = 1"), [DangerKind::Update]);
        assert_eq!(kinds("delete from t where id = 1"), [DangerKind::Delete]);
        assert_eq!(kinds("DROP TABLE t"), [DangerKind::Drop]);
        assert_eq!(kinds("TRUNCATE TABLE t"), [DangerKind::Truncate]);
        assert_eq!(kinds("ALTER TABLE t ADD c INT"), [DangerKind::Alter]);
    }

    #[test]
    fn flags_update_delete_without_where() {
        let found = dangerous_statements("DELETE FROM t");
        assert!(found[0].missing_where);
        let found = dangerous_statements("UPDATE t SET a = 1");
        assert!(found[0].missing_where);
        let found = dangerous_statements("UPDATE t SET a = 1 WHERE id = 3");
        assert!(!found[0].missing_where);
        // DROP has no WHERE concept — never flagged for it.
        let found = dangerous_statements("DROP TABLE t");
        assert!(!found[0].missing_where);
    }

    #[test]
    fn where_inside_literal_or_comment_does_not_count() {
        let found = dangerous_statements("DELETE FROM t -- where id = 1");
        assert!(found[0].missing_where);
        let found = dangerous_statements("UPDATE t SET a = 'where'");
        assert!(found[0].missing_where);
        let found = dangerous_statements("UPDATE [where] SET a = 1");
        assert!(found[0].missing_where);
        // `WHEREabouts` is a different word, not a WHERE.
        let found = dangerous_statements("DELETE FROM whereabouts_x");
        assert!(found[0].missing_where);
    }

    #[test]
    fn scans_every_statement_in_a_batch() {
        let found = dangerous_statements(
            "SELECT 1; UPDATE t SET a = 1; DELETE FROM u WHERE id = 2; DROP TABLE v",
        );
        assert_eq!(
            found.iter().map(|d| d.kind).collect::<Vec<_>>(),
            [DangerKind::Update, DangerKind::Delete, DangerKind::Drop]
        );
        assert!(found[0].missing_where);
        assert!(!found[1].missing_where);
    }

    #[test]
    fn leading_comments_do_not_hide_danger() {
        assert_eq!(kinds("-- cleanup\nDROP TABLE t"), [DangerKind::Drop]);
        assert_eq!(kinds("/* x */ TRUNCATE t"), [DangerKind::Truncate]);
        // ...and a destructive keyword inside a comment is not a statement.
        assert!(dangerous_statements("-- DROP TABLE t\nSELECT 1").is_empty());
    }
}
