//! Read a CSV or JSON file into records, ready to be inserted into an existing table.
//!
//! This is the inverse of [`crate::export`], and deliberately its mirror image: the data layer
//! owns the parsing so it is unit-testable without a window and behaves identically for every
//! backend. An export→import round-trip is the module's central test.
//!
//! Both formats normalise to the same record shape — `Vec<Option<String>>`, one entry per
//! source field, where `None` is a JSON `null`. That leaves exactly **one** coercion path
//! ([`crate::coerce::EditorKind::parse_strict`]) for the two formats to share, rather than one
//! set of type rules per format.
//!
//! # NULL, and the empty string
//!
//! The formats disagree, so [`coerce_row`] takes the [`ImportFormat`] and applies the rule:
//! - **CSV** has no way to spell NULL. [`crate::export::CsvSink`] writes an empty field for one,
//!   and the `csv` parser cannot distinguish a bare empty field from a quoted `""`. So an empty
//!   CSV field is always NULL — including for a text column, which round-trips what export wrote.
//! - **JSON** spells NULL as `null`, so `""` there is a genuine empty string and is preserved for
//!   a text column (and treated as NULL for any other kind, which has no empty form).
//!
//! # Safety
//!
//! Nothing read from a file ever becomes a SQL *identifier*. The header row is only ever used to
//! pick an index into the target table's introspected columns; the column names in the generated
//! `INSERT` come from [`crate::model::ColumnInfo`] and are quoted by the dialect. Field values go
//! through the same `value_to_literal` escaping as in-grid edits.

use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

use crate::coerce::{CoerceError, EditorKind};
use crate::error::{CoreError, Result};
use crate::model::DbKind;
use crate::value::Value;

/// Refuse to build a single all-or-nothing transaction larger than this. Every generated
/// statement is held in memory until the commit, so an unbounded file would OOM the app; the
/// user is told to split the file rather than being silently truncated.
pub const MAX_IMPORT_ROWS: usize = 200_000;

/// A format a table can be imported from. Mirrors [`crate::export::ExportFormat`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    /// RFC 4180 comma-separated values.
    Csv,
    /// A JSON array of objects, one per row, keyed by column name.
    Json,
}

impl ImportFormat {
    /// File extension (no dot), for file-dialog filters.
    pub fn extension(self) -> &'static str {
        match self {
            ImportFormat::Csv => "csv",
            ImportFormat::Json => "json",
        }
    }

    /// Human label for menus and dialogs.
    pub fn label(self) -> &'static str {
        match self {
            ImportFormat::Csv => "CSV",
            ImportFormat::Json => "JSON",
        }
    }

    /// Guess the format from a file's extension, case-insensitively.
    pub fn from_path(path: &Path) -> Option<ImportFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "csv" => Some(ImportFormat::Csv),
            "json" => Some(ImportFormat::Json),
            _ => None,
        }
    }
}

/// One source field of one record. `None` is a JSON `null`; CSV always yields `Some`.
pub type Record = Vec<Option<String>>;

/// The head of a file: its column names plus the first few records, to drive the mapping dialog.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    /// Source column names — the header row, or `column_1..N` when the file has none.
    pub headers: Vec<String>,
    /// Up to the requested number of data records.
    pub rows: Vec<Record>,
    /// Whether the file holds more records than [`Preview::rows`] shows.
    pub more: bool,
}

/// Read the first `n` records of `path` (plus its headers) without consuming the whole file.
pub fn preview(path: &Path, fmt: ImportFormat, has_header: bool, n: usize) -> Result<Preview> {
    let mut reader = read_records(path, fmt, has_header)?;
    let headers = reader.headers().to_vec();
    let mut rows = Vec::with_capacity(n);
    for record in reader.by_ref().take(n) {
        rows.push(record?);
    }
    // One more pull tells us whether to show "…and more" without reading the rest.
    let more = reader.next().transpose()?.is_some();
    Ok(Preview {
        headers,
        rows,
        more,
    })
}

/// Open `path` and stream its records. `None` entries are JSON `null`s.
///
/// CSV is streamed off the disk. JSON is parsed whole (a JSON array cannot be read
/// incrementally without a streaming parser, and [`MAX_IMPORT_ROWS`] bounds the cost).
///
/// `has_header` applies to CSV only — JSON objects are always keyed by name, so it is ignored
/// there.
pub fn read_records(path: &Path, fmt: ImportFormat, has_header: bool) -> Result<RecordReader> {
    match fmt {
        ImportFormat::Csv => open_csv(path, has_header),
        ImportFormat::Json => open_json(path),
    }
}

/// A streaming source of [`Record`]s, over either backing format.
pub struct RecordReader {
    headers: Vec<String>,
    inner: Inner,
}

enum Inner {
    Csv {
        /// A header-less CSV's first record, which had to be read to learn the field count.
        pending: Option<Record>,
        it: Box<csv::StringRecordsIntoIter<Box<dyn Read>>>,
    },
    Json(std::vec::IntoIter<Record>),
}

impl RecordReader {
    /// The source column names, in file order (JSON: the first object's keys).
    pub fn headers(&self) -> &[String] {
        &self.headers
    }
}

impl Iterator for RecordReader {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            Inner::Csv { pending, it } => {
                if let Some(first) = pending.take() {
                    return Some(Ok(first));
                }
                it.next().map(|r| {
                    r.map(|rec| rec.iter().map(|f| Some(f.to_string())).collect())
                        .map_err(|e| CoreError::Import(csv_error_message(&e)))
                })
            }
            Inner::Json(it) => it.next().map(Ok),
        }
    }
}

/// `csv`'s own Display for `UnequalLengths` is serviceable but buries the row number; surface
/// it the way the import dialog wants to show it.
fn csv_error_message(e: &csv::Error) -> String {
    match e.kind() {
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => {
            let row = pos.as_ref().map_or(0, |p| p.record() + 1);
            format!("row {row}: expected {expected_len} fields, found {len}")
        }
        _ => e.to_string(),
    }
}

fn open_csv(path: &Path, has_header: bool) -> Result<RecordReader> {
    // `has_headers(false)`: we take the header row ourselves so the no-header case can
    // synthesize names. `flexible(false)`: a ragged row becomes a per-record error, not a panic
    // and not a silently short row.
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(open_stripping_bom(path)?);

    // The first record is either the header row, or data whose width defines the columns. Either
    // way it must be read here; when it is data it is held as `pending` and re-emitted first.
    let mut first = csv::StringRecord::new();
    let has_any = rdr
        .read_record(&mut first)
        .map_err(|e| CoreError::Import(csv_error_message(&e)))?;

    let (headers, pending) = match (has_any, has_header) {
        (false, _) => (Vec::new(), None),
        (true, true) => (first.iter().map(str::to_string).collect(), None),
        (true, false) => {
            let headers = (1..=first.len()).map(|i| format!("column_{i}")).collect();
            let record: Record = first.iter().map(|f| Some(f.to_string())).collect();
            (headers, Some(record))
        }
    };

    Ok(RecordReader {
        headers,
        inner: Inner::Csv {
            pending,
            it: Box::new(rdr.into_records()),
        },
    })
}

/// Open `path`, skipping a UTF-8 byte-order mark if present. Excel writes one; the `csv` crate
/// would otherwise fold it into the first header name.
fn open_stripping_bom(path: &Path) -> Result<Box<dyn Read>> {
    let mut file = File::open(path)?;
    let mut head = [0u8; 3];
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    if filled == 3 && head == [0xEF, 0xBB, 0xBF] {
        Ok(Box::new(BufReader::new(file)))
    } else {
        // Not a BOM (or a file shorter than one): put the bytes back in front of the stream.
        Ok(Box::new(
            Cursor::new(head[..filled].to_vec()).chain(BufReader::new(file)),
        ))
    }
}

fn open_json(path: &Path) -> Result<RecordReader> {
    let doc: serde_json::Value = serde_json::from_reader(BufReader::new(File::open(path)?))?;
    let serde_json::Value::Array(items) = doc else {
        return Err(CoreError::Import(
            "expected a JSON array of objects at the top level".into(),
        ));
    };

    // Header order follows the first object's keys. `serde_json`'s Map is a BTreeMap unless the
    // `preserve_order` feature is on, so in practice that is alphabetical — deterministic, and
    // irrelevant to correctness because columns are matched onto the table by name.
    let headers: Vec<String> = match items.first() {
        Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        Some(_) => {
            return Err(CoreError::Import(
                "expected a JSON array of objects, found a non-object element".into(),
            ))
        }
        None => Vec::new(),
    };

    let mut rows = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let serde_json::Value::Object(map) = item else {
            return Err(CoreError::Import(format!(
                "row {}: expected a JSON object",
                i + 1
            )));
        };
        let mut record = Vec::with_capacity(headers.len());
        for key in &headers {
            // A key missing from a later object is NULL; keys absent from the first object are
            // not part of the header and cannot be mapped, so they are ignored.
            record.push(json_scalar(map.get(key), key, i + 1)?);
        }
        rows.push(record);
    }

    Ok(RecordReader {
        headers,
        inner: Inner::Json(rows.into_iter()),
    })
}

/// Render one JSON scalar as the record's text form. Nested containers have no column-shaped
/// meaning, so they are an error rather than a stringified blob the user did not ask for.
fn json_scalar(v: Option<&serde_json::Value>, column: &str, row: usize) -> Result<Option<String>> {
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
            return Err(CoreError::Import(format!(
                "column `{column}` row {row}: nested JSON is not supported"
            )))
        }
    })
}

// ─── Mapping a record onto the target table ──────────────────────────────────

/// One target column an import writes to, and where its value comes from in the source record.
#[derive(Debug, Clone)]
pub struct Target {
    /// The target column's name, as introspected from the database.
    pub name: String,
    /// How to coerce the field, from the target column's declared type.
    pub kind: EditorKind,
    /// Index into the source record.
    pub source: usize,
}

/// A field of the file could not be coerced into its target column's type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("row {row}, column `{column}`: {source}")]
pub struct FieldError {
    /// 1-based record number, not counting the header row.
    pub row: usize,
    /// The *target* column's name.
    pub column: String,
    pub source: CoerceError,
}

/// Coerce one source record into the values for `targets`, in target order.
///
/// `row` is only used to label errors. See the module docs for the NULL/empty-string rule that
/// `fmt` selects.
pub fn coerce_row(
    record: &Record,
    targets: &[Target],
    fmt: ImportFormat,
    row: usize,
) -> std::result::Result<Vec<Value>, FieldError> {
    targets
        .iter()
        .map(|t| {
            // A source index past the end of a short record reads as NULL rather than panicking.
            let field = record.get(t.source).and_then(Option::as_deref);
            coerce_field(field, t.kind, fmt).map_err(|source| FieldError {
                row,
                column: t.name.clone(),
                source,
            })
        })
        .collect()
}

fn coerce_field(
    field: Option<&str>,
    kind: EditorKind,
    fmt: ImportFormat,
) -> std::result::Result<Value, CoerceError> {
    match field {
        // JSON null, or a source column that this record does not reach.
        None => Ok(Value::Null),
        Some(s) => match fmt {
            // CSV cannot spell NULL; an empty field is one. See the module docs.
            ImportFormat::Csv if s.is_empty() => Ok(Value::Null),
            // In JSON, "" is a real empty string — meaningful only for a text column.
            ImportFormat::Json if s.is_empty() && kind != EditorKind::Text => Ok(Value::Null),
            _ => kind.parse_strict(s),
        },
    }
}

/// Soft cap on one generated `INSERT`. MySQL's `max_allowed_packet` defaults as low as 4 MiB
/// and SQL Server has its own packet limits, so batches are split well before either.
const MAX_STATEMENT_BYTES: usize = 512 * 1024;

/// Turn coerced rows into the `INSERT` statements of a single all-or-nothing transaction.
///
/// Batches are split on two limits: the dialect's row cap ([`DbKind::max_insert_rows`] — SQL
/// Server hard-rejects a `VALUES` clause over 1000 rows) and [`MAX_STATEMENT_BYTES`], estimated
/// from the rendered width of each value. Every statement is held in memory until the commit,
/// which is what [`MAX_IMPORT_ROWS`] bounds.
///
/// `col_names` must be the target table's own introspected column names — never anything read
/// from the file. See the module's safety note.
pub fn build_insert_batches(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col_names: &[&str],
    rows: &[Vec<Value>],
) -> Result<Vec<String>> {
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(CoreError::Import(format!(
            "{} rows exceeds the {MAX_IMPORT_ROWS}-row limit for a single transaction — \
             split the file and import it in parts",
            rows.len()
        )));
    }
    let max_rows = kind.max_insert_rows();
    let mut out = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        let mut end = start;
        let mut bytes = 0usize;
        while end < rows.len() && end - start < max_rows {
            // Estimate, not exact: quoting and escaping add a little, which the budget absorbs.
            let row_bytes: usize = rows[end].iter().map(|v| v.as_text().len() + 3).sum();
            if end > start && bytes + row_bytes > MAX_STATEMENT_BYTES {
                break;
            }
            bytes += row_bytes;
            end += 1;
        }
        let slice: Vec<&[Value]> = rows[start..end].iter().map(Vec::as_slice).collect();
        let sql = crate::model::build_multi_insert_for(kind, schema, table, col_names, &slice)
            .ok_or_else(|| {
                CoreError::Import(
                    "a value has no SQL literal form — binary columns cannot be imported".into(),
                )
            })?;
        out.push(sql);
        start = end;
    }
    Ok(out)
}

/// Whether a declared column type holds binary data, which has no portable SQL literal form
/// (see `value_to_literal`) and so cannot be imported. Such a column must be left unmapped.
pub fn is_binary_type(data_type: &str) -> bool {
    let t = data_type.to_ascii_uppercase();
    ["BLOB", "BYTEA", "VARBINARY", "BINARY", "IMAGE", "RAW"]
        .iter()
        .any(|b| t.contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export;
    use crate::model::ColumnMeta;
    use std::io::Write;

    /// Write `content` to a uniquely-named temp file with the given extension.
    fn temp_file(name: &str, content: &[u8]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "plusplus-import-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = File::create(&p).unwrap();
        f.write_all(content).unwrap();
        p
    }

    fn records(path: &Path, fmt: ImportFormat, has_header: bool) -> Vec<Record> {
        read_records(path, fmt, has_header)
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn cols(names: &[&str]) -> Vec<ColumnMeta> {
        names
            .iter()
            .map(|n| ColumnMeta {
                name: (*n).to_string(),
                type_name: "TEXT".to_string(),
            })
            .collect()
    }

    #[test]
    fn csv_round_trips_what_export_wrote() {
        // The values export quotes: embedded comma, quote, newline, CRLF — plus a NULL.
        let columns = cols(&["a", "b", "c"]);
        let rows = vec![
            vec![
                Value::Text("plain".into()),
                Value::Text("has,comma".into()),
                Value::Text("has\"quote".into()),
            ],
            vec![
                Value::Text("line\nbreak".into()),
                Value::Text("crlf\r\nhere".into()),
                Value::Null,
            ],
        ];
        let csv_text = export::to_csv(&columns, &rows);
        let path = temp_file("roundtrip.csv", csv_text.as_bytes());

        let reader = read_records(&path, ImportFormat::Csv, true).unwrap();
        assert_eq!(reader.headers(), ["a", "b", "c"]);
        let got = records(&path, ImportFormat::Csv, true);

        assert_eq!(got.len(), 2);
        assert_eq!(got[0][1].as_deref(), Some("has,comma"));
        assert_eq!(got[0][2].as_deref(), Some("has\"quote"));
        assert_eq!(got[1][0].as_deref(), Some("line\nbreak"));
        assert_eq!(got[1][1].as_deref(), Some("crlf\r\nhere"));
        // Export writes NULL as an empty field; import reads it back as one.
        assert_eq!(got[1][2].as_deref(), Some(""));
        let v = coerce_field(got[1][2].as_deref(), EditorKind::Text, ImportFormat::Csv).unwrap();
        assert_eq!(v, Value::Null);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_strips_a_utf8_bom_from_the_first_header() {
        let mut content = vec![0xEF, 0xBB, 0xBF];
        content.extend_from_slice(b"id,name\r\n1,ada\r\n");
        let path = temp_file("bom.csv", &content);

        let reader = read_records(&path, ImportFormat::Csv, true).unwrap();
        assert_eq!(
            reader.headers(),
            ["id", "name"],
            "a BOM must not become part of the first column name"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_without_a_header_synthesizes_names_and_keeps_the_first_row() {
        let path = temp_file("noheader.csv", b"1,ada\n2,grace\n");
        let reader = read_records(&path, ImportFormat::Csv, false).unwrap();
        assert_eq!(reader.headers(), ["column_1", "column_2"]);

        let got = records(&path, ImportFormat::Csv, false);
        assert_eq!(got.len(), 2, "the first row is data, not a header");
        assert_eq!(got[0][0].as_deref(), Some("1"));
        assert_eq!(got[1][1].as_deref(), Some("grace"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_ragged_row_is_an_error_naming_the_row() {
        let path = temp_file("ragged.csv", b"a,b\n1,2\n3\n");
        let mut reader = read_records(&path, ImportFormat::Csv, true).unwrap();
        assert!(reader.next().unwrap().is_ok());
        let err = reader.next().unwrap().unwrap_err().to_string();
        assert!(err.contains("expected 2 fields"), "got: {err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn csv_empty_file_yields_no_headers_and_no_rows() {
        let path = temp_file("empty.csv", b"");
        let reader = read_records(&path, ImportFormat::Csv, true).unwrap();
        assert!(reader.headers().is_empty());
        assert_eq!(records(&path, ImportFormat::Csv, true).len(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_reads_objects_and_maps_null_to_none() {
        let path = temp_file(
            "rows.json",
            br#"[{"id":1,"name":"ada","ok":true},{"id":2,"name":null,"ok":false}]"#,
        );
        let reader = read_records(&path, ImportFormat::Json, true).unwrap();
        // serde_json's Map is sorted without `preserve_order`.
        assert_eq!(reader.headers(), ["id", "name", "ok"]);

        let got = records(&path, ImportFormat::Json, true);
        assert_eq!(got[0][0].as_deref(), Some("1"));
        assert_eq!(got[0][2].as_deref(), Some("true"));
        assert_eq!(got[1][1], None, "JSON null becomes a None field");
        std::fs::remove_file(&path).ok();
    }

    /// `RecordReader` is a stream, not a `Debug` value, so `unwrap_err` is unavailable.
    fn expect_read_error(path: &Path, fmt: ImportFormat) -> String {
        match read_records(path, fmt, true) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected read_records to reject this file"),
        }
    }

    #[test]
    fn json_nested_value_is_rejected_naming_column_and_row() {
        let path = temp_file("nested.json", br#"[{"a":1},{"a":{"deep":true}}]"#);
        let err = expect_read_error(&path, ImportFormat::Json);
        assert!(err.contains("column `a` row 2"), "got: {err}");
        assert!(err.contains("nested JSON"), "got: {err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_top_level_must_be_an_array_of_objects() {
        let path = temp_file("obj.json", br#"{"a":1}"#);
        let err = expect_read_error(&path, ImportFormat::Json);
        assert!(err.contains("array of objects"), "got: {err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_missing_key_in_a_later_object_is_null() {
        let path = temp_file("sparse.json", br#"[{"a":1,"b":2},{"a":3}]"#);
        let got = records(&path, ImportFormat::Json, true);
        assert_eq!(got[1][0].as_deref(), Some("3"));
        assert_eq!(got[1][1], None);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn preview_reports_more_without_reading_the_whole_file() {
        let path = temp_file("many.csv", b"a\n1\n2\n3\n4\n");
        let p = preview(&path, ImportFormat::Csv, true, 2).unwrap();
        assert_eq!(p.headers, ["a"]);
        assert_eq!(p.rows.len(), 2);
        assert!(p.more);

        let p = preview(&path, ImportFormat::Csv, true, 10).unwrap();
        assert_eq!(p.rows.len(), 4);
        assert!(!p.more);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_empty_field_is_null_in_csv_even_for_a_text_column() {
        // CSV has no other way to spell NULL, and this is what export writes for one.
        let v = coerce_field(Some(""), EditorKind::Text, ImportFormat::Csv).unwrap();
        assert_eq!(v, Value::Null);
        let v = coerce_field(Some(""), EditorKind::Int, ImportFormat::Csv).unwrap();
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn json_empty_string_is_preserved_only_for_text_columns() {
        // JSON spells NULL as `null`, so "" is a genuine empty string.
        let text = coerce_field(Some(""), EditorKind::Text, ImportFormat::Json).unwrap();
        assert_eq!(text, Value::Text(String::new()));

        // A numeric column has no empty form, so "" is NULL rather than a coercion error.
        let int = coerce_field(Some(""), EditorKind::Int, ImportFormat::Json).unwrap();
        assert_eq!(int, Value::Null);

        // ...and an explicit JSON null is NULL for any kind.
        let n = coerce_field(None, EditorKind::Int, ImportFormat::Json).unwrap();
        assert_eq!(n, Value::Null);
    }

    #[test]
    fn coerce_row_types_by_target_and_names_the_failing_column() {
        let targets = vec![
            Target {
                name: "id".into(),
                kind: EditorKind::Int,
                source: 0,
            },
            Target {
                name: "active".into(),
                kind: EditorKind::Bool,
                source: 2,
            },
        ];
        let record: Record = vec![
            Some("7".into()),
            Some("ignored".into()),
            Some("yes".into()),
        ];
        let vals = coerce_row(&record, &targets, ImportFormat::Csv, 1).unwrap();
        assert_eq!(vals, vec![Value::Int(7), Value::Bool(true)]);

        let bad: Record = vec![Some("abc".into()), None, Some("yes".into())];
        let err = coerce_row(&bad, &targets, ImportFormat::Csv, 4).unwrap_err();
        assert_eq!(err.row, 4);
        assert_eq!(err.column, "id");
        assert!(err.to_string().contains("an integer"), "{err}");
    }

    #[test]
    fn coerce_row_reads_a_short_record_as_null() {
        let targets = vec![Target {
            name: "b".into(),
            kind: EditorKind::Text,
            source: 5,
        }];
        let vals = coerce_row(&vec![Some("a".into())], &targets, ImportFormat::Csv, 1).unwrap();
        assert_eq!(vals, vec![Value::Null]);
    }

    #[test]
    fn batches_insert_only_the_mapped_columns_and_quote_per_dialect() {
        let rows = vec![vec![Value::Int(1), Value::Text("O'Brien".into())]];
        let sql = build_insert_batches(
            DbKind::Postgres,
            Some("public"),
            "users",
            &["id", "name"],
            &rows,
        )
        .unwrap();
        assert_eq!(sql.len(), 1);
        assert_eq!(
            sql[0],
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES\n  (1, 'O''Brien');"
        );

        // MySQL: backtick identifiers.
        let sql =
            build_insert_batches(DbKind::MySql, None, "users", &["id", "name"], &rows).unwrap();
        assert!(sql[0].starts_with("INSERT INTO `users` (`id`, `name`)"), "{}", sql[0]);
    }

    #[test]
    fn batches_respect_the_sql_server_thousand_row_values_cap() {
        let rows: Vec<Vec<Value>> = (0..2500).map(|i| vec![Value::Int(i)]).collect();

        let mssql = build_insert_batches(DbKind::SqlServer, None, "t", &["id"], &rows).unwrap();
        assert_eq!(mssql.len(), 3, "2500 rows / 1000-row cap");
        assert_eq!(mssql[0].matches("),\n  (").count(), 999);

        // Other dialects use the conservative 500-row batch.
        let pg = build_insert_batches(DbKind::Postgres, None, "t", &["id"], &rows).unwrap();
        assert_eq!(pg.len(), 5);
    }

    #[test]
    fn batches_split_on_the_byte_budget_before_the_row_cap() {
        // Each row is ~64 KiB, so far fewer than 500 fit in the 512 KiB budget.
        let big = "x".repeat(64 * 1024);
        let rows: Vec<Vec<Value>> = (0..20).map(|_| vec![Value::Text(big.clone())]).collect();
        let sql = build_insert_batches(DbKind::Postgres, None, "t", &["v"], &rows).unwrap();
        assert!(sql.len() > 1, "a 1.2 MiB payload must not be one statement");
        assert!(sql.iter().all(|s| s.len() < MAX_STATEMENT_BYTES * 2));
    }

    #[test]
    fn batches_refuse_a_file_over_the_transaction_row_cap() {
        let rows: Vec<Vec<Value>> = (0..MAX_IMPORT_ROWS + 1).map(|_| vec![Value::Int(1)]).collect();
        let err = build_insert_batches(DbKind::Sqlite, None, "t", &["id"], &rows).unwrap_err();
        assert!(err.to_string().contains("split the file"), "{err}");
    }

    #[test]
    fn batches_reject_binary_values_which_have_no_literal_form() {
        let rows = vec![vec![Value::Bytes(vec![0, 1, 2])]];
        let err = build_insert_batches(DbKind::Sqlite, None, "t", &["blob"], &rows).unwrap_err();
        assert!(err.to_string().contains("binary columns"), "{err}");
    }

    #[test]
    fn no_rows_produces_no_statements() {
        let sql = build_insert_batches(DbKind::Sqlite, None, "t", &["id"], &[]).unwrap();
        assert!(sql.is_empty());
    }

    #[test]
    fn binary_types_are_recognised_across_dialects() {
        for t in ["BLOB", "bytea", "VARBINARY(MAX)", "image", "LONGBLOB"] {
            assert!(is_binary_type(t), "{t} should be binary");
        }
        for t in ["TEXT", "INTEGER", "timestamptz", "numeric(10,2)"] {
            assert!(!is_binary_type(t), "{t} should not be binary");
        }
    }

    #[test]
    fn format_is_guessed_from_the_extension() {
        assert_eq!(
            ImportFormat::from_path(Path::new("/tmp/a.CSV")),
            Some(ImportFormat::Csv)
        );
        assert_eq!(
            ImportFormat::from_path(Path::new("/tmp/a.json")),
            Some(ImportFormat::Json)
        );
        assert_eq!(ImportFormat::from_path(Path::new("/tmp/a.txt")), None);
    }
}
