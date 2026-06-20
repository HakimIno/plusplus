//! Serialize a result to CSV or JSON for export to a file.
//!
//! The data layer owns this (rather than the UI) so the encoding is unit-testable without a
//! window, and so every backend exports identically — values are already normalised into the
//! common [`Value`] enum by the time they reach here.
//!
//! Export is *streaming*: a backend pulls rows off the wire and pushes them into a [`RowSink`]
//! one at a time, which writes straight to the file. A multi-million-row table is exported
//! whole without ever materializing in memory, so there is no row cap — unlike the grid, which
//! caps the fetch to protect the UI.

use std::io::{self, Write};

use crate::model::ColumnMeta;
use crate::value::Value;

/// A format a result can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// RFC 4180 comma-separated values, with a header row of column names.
    Csv,
    /// A JSON array of objects, one per row, keyed by column name.
    Json,
}

impl ExportFormat {
    /// File extension (no dot) for save dialogs and default file names.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    }

    /// Human label for menus.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
        }
    }

    /// Build a streaming sink that writes this format to `w`.
    pub fn sink<'a, W: Write + Send + 'a>(self, w: W) -> Box<dyn RowSink + Send + 'a> {
        match self {
            ExportFormat::Csv => Box::new(CsvSink::new(w)),
            ExportFormat::Json => Box::new(JsonSink::new(w)),
        }
    }
}

/// A streaming destination for an exported result. A backend calls [`RowSink::begin`] once
/// with the column metadata (on the first row), [`RowSink::write_row`] for every row in fetch
/// order, then [`RowSink::finish`] exactly once at the end — even when there were no rows, so
/// the sink can still emit a valid empty document.
pub trait RowSink {
    /// Announce the columns. Called once, before the first [`RowSink::write_row`].
    fn begin(&mut self, columns: &[ColumnMeta]) -> io::Result<()>;
    /// Write one row, in fetch order. Its length matches the columns from [`RowSink::begin`].
    fn write_row(&mut self, row: &[Value]) -> io::Result<()>;
    /// Flush and close out the document. Always called once, even for an empty result.
    fn finish(&mut self) -> io::Result<()>;
}

// ─── CSV ───────────────────────────────────────────────────────────────────

/// Writes RFC 4180 CSV: a header row of column names, then one CRLF-terminated line per row.
/// A field is quoted only when it contains a comma, double quote, CR or LF; embedded quotes
/// are doubled. SQL NULL becomes an empty (unquoted) field, distinct from an empty string.
pub struct CsvSink<W: Write> {
    w: W,
}

impl<W: Write> CsvSink<W> {
    pub fn new(w: W) -> Self {
        Self { w }
    }
}

impl<W: Write> RowSink for CsvSink<W> {
    fn begin(&mut self, columns: &[ColumnMeta]) -> io::Result<()> {
        write_csv_record(&mut self.w, columns.iter().map(|c| csv_field(&c.name)))
    }

    fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        write_csv_record(
            &mut self.w,
            row.iter().map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    csv_field(&v.as_text())
                }
            }),
        )
    }

    fn finish(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

fn write_csv_record(
    w: &mut impl Write,
    fields: impl Iterator<Item = String>,
) -> io::Result<()> {
    let mut first = true;
    for field in fields {
        if !first {
            w.write_all(b",")?;
        }
        w.write_all(field.as_bytes())?;
        first = false;
    }
    w.write_all(b"\r\n")
}

/// Quote a single CSV field if it needs it, doubling any embedded quotes.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ─── JSON ────────────────────────────────────────────────────────────────────

/// Writes a JSON array of objects, one per row, keyed by column name (one object per line so
/// the stream stays readable without buffering the whole document). Values keep their JSON
/// type where it survives the round trip (null, bool, integer, finite float, string); bytes
/// fall back to their display placeholder. If two columns share a name the later one wins,
/// since a JSON object can't hold duplicate keys — CSV is the faithful format for such results.
pub struct JsonSink<W: Write> {
    w: W,
    columns: Vec<String>,
    wrote_row: bool,
    began: bool,
}

impl<W: Write> JsonSink<W> {
    pub fn new(w: W) -> Self {
        Self {
            w,
            columns: Vec::new(),
            wrote_row: false,
            began: false,
        }
    }
}

impl<W: Write> RowSink for JsonSink<W> {
    fn begin(&mut self, columns: &[ColumnMeta]) -> io::Result<()> {
        self.columns = columns.iter().map(|c| c.name.clone()).collect();
        self.began = true;
        self.w.write_all(b"[")
    }

    fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        if self.wrote_row {
            self.w.write_all(b",")?;
        }
        self.w.write_all(b"\n  ")?;
        let mut map = serde_json::Map::with_capacity(self.columns.len());
        for (name, val) in self.columns.iter().zip(row) {
            map.insert(name.clone(), json_value(val));
        }
        let obj = serde_json::Value::Object(map);
        // The values are all plain scalars (json_value already neutralises NaN/∞), so this
        // only fails on an underlying write error — surface that as one.
        serde_json::to_writer(&mut self.w, &obj).map_err(io::Error::from)?;
        self.wrote_row = true;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        match (self.began, self.wrote_row) {
            // Rows were written: close the array on its own line.
            (_, true) => self.w.write_all(b"\n]")?,
            // begin() ran but no rows arrived: an empty array on one line.
            (true, false) => self.w.write_all(b"]")?,
            // No columns ever announced (empty result, no metadata): still valid JSON.
            (false, false) => self.w.write_all(b"[]")?,
        }
        self.w.flush()
    }
}

fn json_value(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        // NaN/Infinity have no JSON representation; emit null rather than invalid JSON.
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Bytes(_) => serde_json::Value::String(v.as_text()),
    }
}

// ─── In-memory convenience (used by tests and small one-shot exports) ─────────

/// Serialize a fully-materialized result to a CSV string. Thin wrapper over [`CsvSink`].
pub fn to_csv(columns: &[ColumnMeta], rows: &[Vec<Value>]) -> String {
    serialize_to_string(ExportFormat::Csv, columns, rows)
}

/// Serialize a fully-materialized result to a JSON string. Thin wrapper over [`JsonSink`].
pub fn to_json(columns: &[ColumnMeta], rows: &[Vec<Value>]) -> String {
    serialize_to_string(ExportFormat::Json, columns, rows)
}

fn serialize_to_string(
    format: ExportFormat,
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
) -> String {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut sink = format.sink(&mut buf);
        // Writing to a Vec is infallible, so these unwraps never fire.
        sink.begin(columns).expect("write to Vec");
        for row in rows {
            sink.write_row(row).expect("write to Vec");
        }
        sink.finish().expect("write to Vec");
    }
    String::from_utf8(buf).expect("export output is valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<ColumnMeta> {
        names
            .iter()
            .map(|n| ColumnMeta {
                name: n.to_string(),
                type_name: "text".to_string(),
            })
            .collect()
    }

    #[test]
    fn csv_has_header_and_crlf_rows() {
        let c = cols(&["id", "name"]);
        let rows = vec![
            vec![Value::Int(1), Value::Text("Alice".into())],
            vec![Value::Int(2), Value::Text("Bob".into())],
        ];
        assert_eq!(to_csv(&c, &rows), "id,name\r\n1,Alice\r\n2,Bob\r\n");
    }

    #[test]
    fn csv_quotes_only_when_needed_and_doubles_quotes() {
        let c = cols(&["a", "b", "c"]);
        let rows = vec![vec![
            Value::Text("plain".into()),
            Value::Text("has,comma".into()),
            Value::Text("say \"hi\"".into()),
        ]];
        assert_eq!(
            to_csv(&c, &rows),
            "a,b,c\r\nplain,\"has,comma\",\"say \"\"hi\"\"\"\r\n"
        );
    }

    #[test]
    fn csv_quotes_fields_with_newlines() {
        let c = cols(&["note"]);
        let rows = vec![vec![Value::Text("line1\nline2".into())]];
        assert_eq!(to_csv(&c, &rows), "note\r\n\"line1\nline2\"\r\n");
    }

    #[test]
    fn csv_null_is_empty_field() {
        let c = cols(&["a", "b"]);
        let rows = vec![vec![Value::Null, Value::Text(String::new())]];
        assert_eq!(to_csv(&c, &rows), "a,b\r\n,\r\n");
    }

    #[test]
    fn json_keeps_native_types() {
        let c = cols(&["id", "ok", "ratio", "name", "missing"]);
        let rows = vec![vec![
            Value::Int(7),
            Value::Bool(true),
            Value::Float(1.5),
            Value::Text("x".into()),
            Value::Null,
        ]];
        let v: serde_json::Value = serde_json::from_str(&to_json(&c, &rows)).unwrap();
        let obj = &v[0];
        assert_eq!(obj["id"], serde_json::json!(7));
        assert_eq!(obj["ok"], serde_json::json!(true));
        assert_eq!(obj["ratio"], serde_json::json!(1.5));
        assert_eq!(obj["name"], serde_json::json!("x"));
        assert_eq!(obj["missing"], serde_json::Value::Null);
    }

    #[test]
    fn json_is_an_array_with_one_object_per_row() {
        let c = cols(&["a"]);
        let rows = vec![vec![Value::Int(1)], vec![Value::Int(2)]];
        let v: serde_json::Value = serde_json::from_str(&to_json(&c, &rows)).unwrap();
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[1]["a"], serde_json::json!(2));
    }

    #[test]
    fn json_non_finite_float_becomes_null() {
        let c = cols(&["f"]);
        let rows = vec![vec![Value::Float(f64::INFINITY)]];
        let v: serde_json::Value = serde_json::from_str(&to_json(&c, &rows)).unwrap();
        assert_eq!(v[0]["f"], serde_json::Value::Null);
    }

    #[test]
    fn empty_result_round_trips() {
        let c = cols(&["a"]);
        assert_eq!(to_csv(&c, &[]), "a\r\n");
        // begin() ran with one column but no rows: a valid empty array.
        assert_eq!(to_json(&c, &[]), "[]");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&to_json(&c, &[])).unwrap(),
            serde_json::json!([])
        );
    }

    #[test]
    fn streaming_sink_matches_one_shot_helper() {
        // Drive the sink directly (as a backend would) and confirm it equals to_csv.
        let c = cols(&["x", "y"]);
        let rows = vec![vec![Value::Int(1), Value::Null]];
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut sink = ExportFormat::Csv.sink(&mut buf);
            sink.begin(&c).unwrap();
            for row in &rows {
                sink.write_row(row).unwrap();
            }
            sink.finish().unwrap();
        }
        assert_eq!(String::from_utf8(buf).unwrap(), to_csv(&c, &rows));
    }
}
