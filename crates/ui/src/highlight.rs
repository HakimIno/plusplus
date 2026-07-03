//! A lightweight SQL syntax highlighter used by the query editor. It produces an egui
//! `LayoutJob`, colouring keywords, strings, numbers, comments, and punctuation. It is
//! deliberately simple (no external parser) — enough to give the editor a polished,
//! TablePlus-like look.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

struct SqlColors {
    keyword: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    punct: Color32,
    ident: Color32,
}

fn sql_colors() -> SqlColors {
    let t = crate::theme::current();
    SqlColors {
        keyword: t.accent,
        string: t.success,
        number: t.warning,
        comment: t.text_faint,
        punct: mix(t.accent, t.text_weak, if t.is_dark { 0.62 } else { 0.45 }),
        ident: t.text,
    }
}

fn mix(a: Color32, b: Color32, amount_b: f32) -> Color32 {
    let amount_a = 1.0 - amount_b;
    let ch = |x: u8, y: u8| (x as f32 * amount_a + y as f32 * amount_b).round() as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// Build a coloured layout job for `text`, using `font` for every run.
pub fn highlight_sql(text: &str, font: FontId) -> LayoutJob {
    let colors = sql_colors();
    let mut job = LayoutJob::default();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    let push = |job: &mut LayoutJob, s: &str, color: Color32| {
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };

    while i < n {
        let c = chars[i];

        // Line comment: -- … end of line
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            let start = i;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            push(
                &mut job,
                &chars[start..i].iter().collect::<String>(),
                colors.comment,
            );
            continue;
        }

        // Block comment: /* … */
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i < n && !(chars[i] == '*' && i + 1 < n && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            push(
                &mut job,
                &chars[start..i].iter().collect::<String>(),
                colors.comment,
            );
            continue;
        }

        // String literal: '…' with '' as an escaped quote.
        if c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\'' {
                    if i + 1 < n && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            push(
                &mut job,
                &chars[start..i].iter().collect::<String>(),
                colors.string,
            );
            continue;
        }

        // Identifier / keyword.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let color = if is_keyword(&word) {
                colors.keyword
            } else {
                colors.ident
            };
            push(&mut job, &word, color);
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            push(
                &mut job,
                &chars[start..i].iter().collect::<String>(),
                colors.number,
            );
            continue;
        }

        // Punctuation / operators.
        if "(),;*=<>!+-/%|.".contains(c) {
            push(&mut job, &c.to_string(), colors.punct);
            i += 1;
            continue;
        }

        // Whitespace and everything else.
        let start = i;
        i += 1;
        push(
            &mut job,
            &chars[start..i].iter().collect::<String>(),
            colors.ident,
        );
    }

    job
}

fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.contains(&upper.as_str())
}

pub(crate) const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "TABLE",
    "VIEW",
    "INDEX",
    "DROP",
    "ALTER",
    "ADD",
    "COLUMN",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ON",
    "USING",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "DISTINCT",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "EXISTS",
    "UNION",
    "ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "UNIQUE",
    "DEFAULT",
    "WITH",
    "PRAGMA",
    "EXPLAIN",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "INT",
    "INTEGER",
    "TEXT",
    "REAL",
    "BLOB",
    "BOOLEAN",
    "VARCHAR",
    "TIMESTAMP",
    "DATE",
    "TRUE",
    "FALSE",
    "CAST",
    "COALESCE",
];
