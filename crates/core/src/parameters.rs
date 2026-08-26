//! Application-owned named query parameters (`{{name}}`).
//!
//! The marker is deliberately independent of backend placeholder syntax (`$1`, `?`, `@name`).
//! PlusPlus currently executes complete SQL strings, so values are rendered through the same
//! dialect-aware literal path used by grid edits. Markers inside comments and quoted text are
//! ignored.

use std::collections::HashSet;

use crate::{model::value_to_literal, DbKind, Value};

/// Parameter resolution failed before a statement reached a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterError {
    Missing(String),
    UnsupportedValue(String),
}

impl std::fmt::Display for ParameterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "Enter a value for query parameter “{name}”."),
            Self::UnsupportedValue(name) => {
                write!(f, "Query parameter “{name}” cannot be rendered as SQL.")
            }
        }
    }
}

/// Unique parameter names in first-appearance order.
pub fn query_parameter_names(sql: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    parameter_ranges(sql)
        .into_iter()
        .filter_map(|(_, _, name)| seen.insert(name.to_string()).then(|| name.to_string()))
        .collect()
}

/// Replace every executable `{{name}}` marker with an escaped SQL/CQL literal.
pub fn resolve_query_parameters(
    sql: &str,
    kind: DbKind,
    values: &[(String, Value)],
) -> Result<String, ParameterError> {
    let ranges = parameter_ranges(sql);
    if ranges.is_empty() {
        return Ok(sql.to_string());
    }

    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    for (start, end, name) in ranges {
        out.push_str(&sql[copied..start]);
        let value = values
            .iter()
            .find_map(|(candidate, value)| (candidate == name).then_some(value))
            .ok_or_else(|| ParameterError::Missing(name.to_string()))?;
        let literal = value_to_literal(value, kind)
            .ok_or_else(|| ParameterError::UnsupportedValue(name.to_string()))?;
        out.push_str(&literal);
        copied = end;
    }
    out.push_str(&sql[copied..]);
    Ok(out)
}

/// Byte ranges of valid markers, skipping SQL lexical regions where braces are plain text.
fn parameter_ranges(sql: &str) -> Vec<(usize, usize, &str)> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => i = skip_quoted(bytes, i, quote),
            b'[' => i = skip_bracketed(bytes, i),
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i),
            b'$' => i = skip_dollar_quote(bytes, i).unwrap_or(i + 1),
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                let name_start = i + 2;
                let mut cursor = name_start;
                if bytes.get(cursor).is_some_and(|byte| is_name_start(*byte)) {
                    cursor += 1;
                    while bytes
                        .get(cursor)
                        .is_some_and(|byte| is_name_continue(*byte))
                    {
                        cursor += 1;
                    }
                    if bytes.get(cursor) == Some(&b'}') && bytes.get(cursor + 1) == Some(&b'}') {
                        out.push((i, cursor + 2, &sql[name_start..cursor]));
                        i = cursor + 2;
                        continue;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

fn is_name_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_name_continue(byte: u8) -> bool {
    is_name_start(byte) || byte.is_ascii_digit()
}

fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

fn skip_bracketed(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b']' {
            if bytes.get(i + 1) == Some(&b']') {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

fn skip_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut depth = 1usize;
    let mut i = start + 2;
    while i < bytes.len() && depth > 0 {
        if bytes.get(i..i + 2) == Some(b"/*") {
            depth += 1;
            i += 2;
        } else if bytes.get(i..i + 2) == Some(b"*/") {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_dollar_quote(bytes: &[u8], start: usize) -> Option<usize> {
    let mut tag_end = start + 1;
    while bytes
        .get(tag_end)
        .is_some_and(|byte| *byte == b'_' || byte.is_ascii_alphanumeric())
    {
        tag_end += 1;
    }
    if bytes.get(tag_end) != Some(&b'$') {
        return None;
    }
    let delimiter = &bytes[start..=tag_end];
    let mut i = tag_end + 1;
    while i + delimiter.len() <= bytes.len() {
        if &bytes[i..i + delimiter.len()] == delimiter {
            return Some(i + delimiter.len());
        }
        i += 1;
    }
    Some(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_ignore_quoted_or_commented_markers() {
        let sql = "SELECT {{id}}, '{{ignored}}', {{name}}, {{id}} -- {{comment}}\n/* {{block}} */";
        assert_eq!(query_parameter_names(sql), ["id", "name"]);
    }

    #[test]
    fn resolution_uses_dialect_literals_and_leaves_quoted_markers_alone() {
        let sql = "SELECT * FROM users WHERE id = {{id}} AND name = {{name}} AND note = '{{name}}'";
        let values = vec![
            ("id".into(), Value::Int(7)),
            ("name".into(), Value::Text("O'Brien".into())),
        ];
        assert_eq!(
            resolve_query_parameters(sql, DbKind::Postgres, &values).unwrap(),
            "SELECT * FROM users WHERE id = 7 AND name = 'O''Brien' AND note = '{{name}}'"
        );
    }

    #[test]
    fn dollar_quotes_and_invalid_names_are_not_parameters() {
        let sql = "SELECT $$ {{body}} $$, {{ok}}, {{not-valid}}, {{9bad}}";
        assert_eq!(query_parameter_names(sql), ["ok"]);
    }

    #[test]
    fn missing_values_are_reported_by_name() {
        assert_eq!(
            resolve_query_parameters("SELECT {{id}}", DbKind::Sqlite, &[]),
            Err(ParameterError::Missing("id".into()))
        );
    }
}
