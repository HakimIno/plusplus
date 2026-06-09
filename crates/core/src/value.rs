//! Backend-agnostic representation of a single cell value.
//!
//! Every backend decodes its native types into this common `Value` enum so the UI
//! and analysis layers never need to know which database produced a row.

use std::cmp::Ordering;

/// A single value in a result set, normalized across backends.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL NULL.
    Null,
    Bool(bool),
    /// Any integer type (INT2/4/8, etc.).
    Int(i64),
    /// Any floating type (FLOAT4/8). NUMERIC is rendered as `Text` to preserve precision.
    Float(f64),
    /// Text and any type we render as a string (uuid, json, dates, numeric, ...).
    Text(String),
    /// Raw bytes (BYTEA / BLOB). Rendered as a placeholder, not decoded as text.
    Bytes(Vec<u8>),
}

impl Value {
    /// Whether this value is SQL NULL.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A human-readable string for display in a grid cell.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Text(s) => s.clone(),
            Value::Bytes(b) => format!("[{} bytes]", b.len()),
        }
    }

    /// A value suitable for CSV/JSON export or clipboard (no `[N bytes]` placeholder noise
    /// beyond what `display` provides; NULL becomes an empty string for CSV purposes is left
    /// to the caller).
    pub fn as_text(&self) -> String {
        self.display()
    }
}

impl Value {
    /// Total ordering used for column sorting. NULLs sort last; numbers compare numerically;
    /// mixed types fall back to comparing their display strings so sorting never panics.
    pub fn sort_cmp(&self, other: &Value) -> Ordering {
        use Value::*;
        match (self, other) {
            (Null, Null) => Ordering::Equal,
            (Null, _) => Ordering::Greater,
            (_, Null) => Ordering::Less,
            (Int(a), Int(b)) => a.cmp(b),
            (Float(a), Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Int(a), Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal),
            (Float(a), Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal),
            (Bool(a), Bool(b)) => a.cmp(b),
            (Text(a), Text(b)) => a.cmp(b),
            (a, b) => a.display().cmp(&b.display()),
        }
    }
}
