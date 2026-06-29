//! Render a selection of result rows to text for the clipboard — "Copy as CSV / JSON / SQL
//! INSERT". CSV and JSON reuse the streaming [`crate::export`] sinks (so the encoding matches
//! file export exactly); SQL INSERT reuses [`crate::model::build_multi_insert_sql`] (so value
//! escaping and identifier quoting match the rest of the generated DML).

use crate::export::ExportFormat;
use crate::model::{build_multi_insert_sql, ColumnMeta, DbKind};
use crate::value::Value;

/// A text format a row selection can be copied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFormat {
    /// Tab-separated values, **no header** — the spreadsheet-native clipboard format and what
    /// paste-to-insert reads back. This is the plain Cmd/Ctrl+C copy.
    Tsv,
    /// RFC 4180 CSV, with a header row of column names.
    Csv,
    /// A JSON array of objects, one per row, keyed by column name.
    Json,
    /// A single multi-row `INSERT INTO … VALUES …;`, escaped for the connection's dialect.
    Insert,
}

impl CopyFormat {
    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            CopyFormat::Tsv => "TSV",
            CopyFormat::Csv => "CSV",
            CopyFormat::Json => "JSON",
            CopyFormat::Insert => "SQL INSERT",
        }
    }
}

/// Render `rows` (each a full row of values matching `columns`) as clipboard text in `format`.
///
/// For [`CopyFormat::Insert`], `kind` selects the dialect and `schema`/`table` name the target
/// table. Returns `None` only for `Insert` when a value has no SQL literal form (binary data) —
/// CSV and JSON always succeed. An empty selection yields an empty document (CSV/JSON) or `None`
/// (`Insert` has nothing to insert).
pub fn copy_rows(
    format: CopyFormat,
    columns: &[ColumnMeta],
    rows: &[&[Value]],
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
) -> Option<String> {
    match format {
        CopyFormat::Tsv => Some(render_tsv(rows)),
        CopyFormat::Csv => Some(render_via_sink(ExportFormat::Csv, columns, rows)),
        CopyFormat::Json => Some(render_via_sink(ExportFormat::Json, columns, rows)),
        CopyFormat::Insert => build_multi_insert_sql(kind, schema, table, columns, rows),
    }
}

/// Tab-separated rows, no header and no trailing newline. NULL is an empty field; tabs and
/// newlines embedded in a value are flattened to spaces, since TSV has no standard escaping —
/// this keeps the clipboard grid rectangular so paste-to-insert can split it back cleanly.
fn render_tsv(rows: &[&[Value]]) -> String {
    let mut out = String::new();
    for (r, row) in rows.iter().enumerate() {
        if r > 0 {
            out.push('\n');
        }
        for (c, v) in row.iter().enumerate() {
            if c > 0 {
                out.push('\t');
            }
            if !v.is_null() {
                out.push_str(&v.as_text().replace(['\t', '\n', '\r'], " "));
            }
        }
    }
    out
}

/// Drive an [`ExportFormat`] sink over an in-memory buffer to reuse its exact CSV/JSON encoding.
/// Writes to a `Vec<u8>` never fail, so the `io::Result`s are infallible here.
fn render_via_sink(format: ExportFormat, columns: &[ColumnMeta], rows: &[&[Value]]) -> String {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut sink = format.sink(&mut buf);
        let _ = sink.begin(columns);
        for row in rows {
            let _ = sink.write_row(row);
        }
        let _ = sink.finish();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<ColumnMeta> {
        names
            .iter()
            .map(|n| ColumnMeta {
                name: (*n).to_string(),
                type_name: "TEXT".into(),
            })
            .collect()
    }

    fn sample() -> (Vec<ColumnMeta>, Vec<Vec<Value>>) {
        let columns = cols(&["id", "name"]);
        let rows = vec![
            vec![Value::Int(1), Value::Text("O'Brien".into())],
            vec![Value::Int(2), Value::Null],
        ];
        (columns, rows)
    }

    fn refs(rows: &[Vec<Value>]) -> Vec<&[Value]> {
        rows.iter().map(|r| r.as_slice()).collect()
    }

    #[test]
    fn tsv_has_no_header_and_no_trailing_newline() {
        let (c, r) = sample();
        let out = copy_rows(CopyFormat::Tsv, &c, &refs(&r), DbKind::Postgres, None, "t").unwrap();
        // Data only (no "id\tname" header), NULL is an empty field, no trailing newline so a
        // paste doesn't gain a blank row.
        assert_eq!(out, "1\tO'Brien\n2\t");
    }

    #[test]
    fn csv_has_header_and_escapes() {
        let (c, r) = sample();
        let out = copy_rows(CopyFormat::Csv, &c, &refs(&r), DbKind::Postgres, None, "t").unwrap();
        // Header row, then one line per row; NULL is an empty field, a comma-free name unquoted.
        assert_eq!(out, "id,name\r\n1,O'Brien\r\n2,\r\n");
    }

    #[test]
    fn json_is_array_of_objects() {
        let (c, r) = sample();
        let out = copy_rows(CopyFormat::Json, &c, &refs(&r), DbKind::Postgres, None, "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v[0]["id"], 1);
        assert_eq!(v[0]["name"], "O'Brien");
        assert!(v[1]["name"].is_null());
    }

    #[test]
    fn insert_is_one_multi_row_statement_escaped_per_dialect() {
        let (c, r) = sample();
        let pg = copy_rows(
            CopyFormat::Insert,
            &c,
            &refs(&r),
            DbKind::Postgres,
            Some("public"),
            "users",
        )
        .unwrap();
        assert_eq!(
            pg,
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES\n  (1, 'O''Brien'),\n  (2, NULL);"
        );
        // MySQL uses backtick identifiers.
        let my = copy_rows(CopyFormat::Insert, &c, &refs(&r), DbKind::MySql, None, "users").unwrap();
        assert!(my.starts_with("INSERT INTO `users` (`id`, `name`) VALUES"));
    }

    #[test]
    fn insert_refuses_binary_values() {
        let columns = cols(&["blob"]);
        let rows = vec![vec![Value::Bytes(vec![1, 2, 3])]];
        assert!(
            copy_rows(CopyFormat::Insert, &columns, &refs(&rows), DbKind::Sqlite, None, "t")
                .is_none()
        );
        // …but CSV/JSON still work (binary falls back to a placeholder there).
        assert!(
            copy_rows(CopyFormat::Csv, &columns, &refs(&rows), DbKind::Sqlite, None, "t").is_some()
        );
    }
}
