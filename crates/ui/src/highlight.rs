//! A lightweight SQL syntax highlighter used by the query editor. It produces an egui
//! `LayoutJob`, colouring keywords, strings, numbers, comments, and punctuation. It is
//! deliberately simple (no external parser) — enough to give the editor a polished,
//! TablePlus-like look.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

// Tokyo-Night-ish palette, readable on the dark theme.
const KEYWORD: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7); // blue
const STRING: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a); // green
const NUMBER: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68); // amber
const COMMENT: Color32 = Color32::from_rgb(0x60, 0x68, 0x79); // gray
const PUNCT: Color32 = Color32::from_rgb(0x89, 0xdd, 0xff); // cyan
const IDENT: Color32 = Color32::from_rgb(0xc6, 0xcc, 0xd6); // default text

/// Build a coloured layout job for `text`, using `font` for every run.
pub fn highlight_sql(text: &str, font: FontId) -> LayoutJob {
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
            push(&mut job, &chars[start..i].iter().collect::<String>(), COMMENT);
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
            push(&mut job, &chars[start..i].iter().collect::<String>(), COMMENT);
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
            push(&mut job, &chars[start..i].iter().collect::<String>(), STRING);
            continue;
        }

        // Identifier / keyword.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let color = if is_keyword(&word) { KEYWORD } else { IDENT };
            push(&mut job, &word, color);
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            push(&mut job, &chars[start..i].iter().collect::<String>(), NUMBER);
            continue;
        }

        // Punctuation / operators.
        if "(),;*=<>!+-/%|.".contains(c) {
            push(&mut job, &c.to_string(), PUNCT);
            i += 1;
            continue;
        }

        // Whitespace and everything else.
        let start = i;
        i += 1;
        push(&mut job, &chars[start..i].iter().collect::<String>(), IDENT);
    }

    job
}

fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.contains(&upper.as_str())
}

const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "VIEW", "INDEX", "DROP", "ALTER", "ADD", "COLUMN", "JOIN", "INNER", "LEFT", "RIGHT",
    "FULL", "OUTER", "CROSS", "ON", "USING", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET",
    "DISTINCT", "AS", "AND", "OR", "NOT", "NULL", "IS", "IN", "LIKE", "ILIKE", "BETWEEN", "EXISTS",
    "UNION", "ALL", "CASE", "WHEN", "THEN", "ELSE", "END", "ASC", "DESC", "COUNT", "SUM", "AVG",
    "MIN", "MAX", "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "UNIQUE", "DEFAULT", "WITH", "PRAGMA",
    "EXPLAIN", "BEGIN", "COMMIT", "ROLLBACK", "TRANSACTION", "INT", "INTEGER", "TEXT", "REAL",
    "BLOB", "BOOLEAN", "VARCHAR", "TIMESTAMP", "DATE", "TRUE", "FALSE", "CAST", "COALESCE",
];
