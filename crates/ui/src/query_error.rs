//! Turn backend error strings into actionable query diagnostics.

use std::ops::Range;

/// Explain a database error with a source location when the SQL identifies one reliably.
pub fn explain(sql: &str, raw: &str) -> String {
    let message = clean(raw);
    let lower = message.to_ascii_lowercase();

    if let Some(name) = extract(&message, &lower, "ambiguous column name:", None).or_else(|| {
        extract(
            &message,
            &lower,
            "column reference \"",
            Some("\" is ambiguous"),
        )
    }) {
        return diagnostic(
            sql,
            locate(sql, &name),
            format!("Column \"{name}\" is ambiguous."),
            format!("Qualify it with its table or alias, for example table.{name}."),
        );
    }

    if let Some(name) = extract(&message, &lower, "no such column:", None)
        .or_else(|| extract(&message, &lower, "unknown column '", Some("'")))
        .or_else(|| extract(&message, &lower, "invalid column name '", Some("'")))
        .or_else(|| extract(&message, &lower, "column \"", Some("\" does not exist")))
        .or_else(|| extract(&message, &lower, "referenced column \"", Some("\"")))
    {
        return diagnostic(
            sql,
            locate(sql, &name),
            format!("Column \"{name}\" was not found."),
            "Check its spelling, table alias, and whether it exists in the selected table."
                .to_string(),
        );
    }

    if let Some(name) = extract(&message, &lower, "no such table:", None)
        .or_else(|| extract(&message, &lower, "relation \"", Some("\" does not exist")))
        .or_else(|| extract(&message, &lower, "table '", Some("' doesn't exist")))
        .or_else(|| extract(&message, &lower, "invalid object name '", Some("'")))
    {
        return diagnostic(
            sql,
            locate(sql, &name),
            format!("Table or view \"{name}\" was not found."),
            "Check its spelling, schema or database name, and the active connection.".to_string(),
        );
    }

    if let Some(name) = extract(&message, &lower, "not null constraint failed:", None) {
        return diagnostic(
            sql,
            locate(sql, &name),
            format!("Column \"{name}\" cannot be NULL."),
            "Provide a value for this column before running the statement.".to_string(),
        );
    }

    if let Some(name) = extract(&message, &lower, "unique constraint failed:", None) {
        return diagnostic(
            sql,
            locate(sql, &name),
            format!("The value for \"{name}\" must be unique."),
            "Use a value that is not already present in the table.".to_string(),
        );
    }

    if is_syntax_error(&lower) {
        if let Some(error) = dbcore::check_syntax(None, sql) {
            return diagnostic(
                sql,
                Some(error.range.clone()),
                format!("SQL syntax error: {}", sentence(&error.message)),
                "Check the highlighted token and the clause immediately before it.".to_string(),
            );
        }
    }

    message
}

fn clean(raw: &str) -> String {
    let mut message = raw.trim();
    for prefix in [
        "database error:",
        "error returned from database:",
        "duckdb error:",
        "sql server error:",
        "cassandra error:",
    ] {
        if message
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        {
            message = message[prefix.len()..].trim();
        }
    }
    if message.starts_with("(code:") {
        if let Some(end) = message.find(')') {
            message = message[end + 1..].trim();
        }
    }
    message.to_string()
}

fn extract(message: &str, lower: &str, marker: &str, end: Option<&str>) -> Option<String> {
    let start = lower.find(marker)? + marker.len();
    let suffix = &message[start..];
    let value = match end {
        Some(end) => {
            let lower_suffix = &lower[start..];
            &suffix[..lower_suffix.find(end)?]
        }
        None => suffix.lines().next()?,
    };
    let value = value
        .trim()
        .trim_matches(['\'', '"', '`'])
        .trim_end_matches(['.', ';']);
    (!value.is_empty()).then(|| value.to_string())
}

fn is_syntax_error(lower: &str) -> bool {
    lower.contains("syntax error")
        || lower.contains("sql syntax")
        || lower.contains("parser error")
        || lower.contains("incorrect syntax")
        || lower.contains("incomplete input")
}

fn locate(sql: &str, identifier: &str) -> Option<Range<usize>> {
    let needle = identifier.rsplit('.').next().unwrap_or(identifier);
    let sql_lower = sql.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let byte_start = sql_lower.find(&needle_lower)?;
    let start = sql[..byte_start].chars().count();
    Some(start..start + needle.chars().count())
}

fn diagnostic(
    sql: &str,
    range: Option<Range<usize>>,
    reason: String,
    suggestion: String,
) -> String {
    let Some(range) = range else {
        return format!("{reason}\n{suggestion}");
    };
    let (line, column, source) = line_context(sql, range.start);
    let marker_len = (range.end - range.start).max(1);
    format!(
        "Line {line}, column {column}\n{reason}\n{suggestion}\n\n{source}\n{}{}",
        " ".repeat(column.saturating_sub(1)),
        "^".repeat(marker_len)
    )
}

fn line_context(sql: &str, char_index: usize) -> (usize, usize, String) {
    let mut line = 1;
    let mut column = 1;
    for (index, ch) in sql.chars().enumerate() {
        if index == char_index {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    let source = sql
        .lines()
        .nth(line - 1)
        .unwrap_or_default()
        .replace('\t', "    ");
    (line, column, source)
}

fn sentence(message: &str) -> String {
    let message = message.trim();
    if message.ends_with(['.', '!', '?']) {
        message.to_string()
    } else {
        format!("{message}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_column_names_the_reason_and_source_location() {
        let sql = "SELECT id,\n       missing_name\nFROM customers";
        let error = explain(sql, "database error: no such column: missing_name");
        assert!(error.contains("Line 2, column 8"), "{error}");
        assert!(error.contains("Column \"missing_name\" was not found."));
        assert!(error.contains("missing_name\n       ^^^^^^^^^^^^"));
    }

    #[test]
    fn malformed_sql_uses_the_parser_location() {
        let sql = "SELECT *\nFROM customers\nWHER id = 1";
        let error = explain(sql, "(code: 1) near \"id\": syntax error");
        assert!(error.contains("Line 3"), "{error}");
        assert!(error.contains("SQL syntax error:"));
        assert!(error.contains('^'));
    }

    #[test]
    fn common_backend_table_errors_are_humanized() {
        let error = explain(
            "SELECT * FROM missing_table",
            "error returned from database: relation \"missing_table\" does not exist",
        );
        assert!(error.contains("Table or view \"missing_table\" was not found."));
        assert!(error.contains("Line 1, column 15"));
    }

    #[test]
    fn unknown_errors_remain_available_without_driver_noise() {
        assert_eq!(
            explain(
                "SELECT 1",
                "database error: error returned from database: server busy"
            ),
            "server busy"
        );
    }
}
