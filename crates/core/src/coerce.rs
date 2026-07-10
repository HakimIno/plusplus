//! Classifying a column by its backend type name, and coercing text into a typed [`Value`].
//!
//! This lives in the data layer (rather than the UI) for the same reason [`crate::export`]
//! does: it is the rule for how a string becomes a database value, it must behave identically
//! for every backend, and it should be unit-testable without a window.
//!
//! Two callers, two different needs:
//! - the **grid editor** validates a cell as the user types ([`EditorKind::is_valid`]) and then
//!   coerces the buffer with [`EditorKind::parse`], which never fails — a malformed number
//!   falls back to text, because validation already ran.
//! - **file import** has no chance to validate keystroke-by-keystroke, so it uses
//!   [`EditorKind::parse_strict`], which reports the failure instead of quietly pushing
//!   `"abc"` into an `INT` column and letting the database produce an opaque error.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use crate::value::Value;

/// A value in a file could not be coerced into the type its target column expects.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected}, got \"{got}\"")]
pub struct CoerceError {
    /// Human name of the expected kind, e.g. `"an integer"`.
    pub expected: &'static str,
    /// The offending text, as it appeared in the file.
    pub got: String,
}

/// How a column should be edited, derived from its backend type name. Computed once when a
/// result loads (cheap, and avoids re-parsing the type string every frame).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorKind {
    #[default]
    Text,
    Int,
    Float,
    /// Arbitrary-precision numerics (DECIMAL/NUMERIC/MONEY): validated as a number but
    /// carried as text so the exact digits the user typed reach the database.
    Decimal,
    Bool,
    Date,
    Time,
    DateTime,
}

impl EditorKind {
    /// Classify a backend type name (e.g. `"BIGINT"`, `"timestamp"`, `"bit"`). Order matters:
    /// `DATETIME`/`TIMESTAMP` must be matched before the bare `DATE`/`TIME` substrings, and
    /// `INTERVAL`/`POINT` before the `INT` substring they contain.
    pub fn classify(type_name: &str) -> EditorKind {
        let t = type_name.to_ascii_uppercase();
        if t.contains("BOOL") || t == "BIT" {
            EditorKind::Bool
        } else if t.contains("DATETIME") || t.contains("TIMESTAMP") {
            EditorKind::DateTime
        } else if t.contains("DATE") {
            EditorKind::Date
        } else if t.contains("INTERVAL") {
            EditorKind::Text
        } else if t.contains("TIME") {
            EditorKind::Time
        } else if t.contains("DECIMAL") || t.contains("NUMERIC") || t.contains("MONEY") {
            EditorKind::Decimal
        } else if t.contains("POINT") {
            // POINT/MULTIPOINT contain "INT" but are spatial types; edit them as text.
            EditorKind::Text
        } else if t.contains("INT") || t.contains("SERIAL") {
            EditorKind::Int
        } else if t.contains("FLOAT") || t.contains("DOUBLE") || t.contains("REAL") {
            EditorKind::Float
        } else {
            EditorKind::Text
        }
    }

    /// Human name of what this kind accepts, for error messages.
    pub fn expected(self) -> &'static str {
        match self {
            EditorKind::Text => "text",
            EditorKind::Int => "an integer",
            EditorKind::Float => "a number",
            EditorKind::Decimal => "a decimal number",
            EditorKind::Bool => "a boolean",
            EditorKind::Date => "a date (YYYY-MM-DD)",
            EditorKind::Time => "a time (HH:MM:SS)",
            EditorKind::DateTime => "a timestamp (YYYY-MM-DD HH:MM:SS)",
        }
    }

    /// Whether values of this kind read best in a fixed-width font (numbers and temporals,
    /// where digit alignment matters).
    pub fn monospace_value(self) -> bool {
        !matches!(self, EditorKind::Text | EditorKind::Bool)
    }

    /// Whether `s` is a valid value for this kind. An empty string is always valid — it means
    /// "set NULL". Used to block staging malformed numbers/dates.
    pub fn is_valid(self, s: &str) -> bool {
        let s = s.trim();
        if s.is_empty() {
            return true;
        }
        match self {
            EditorKind::Text => true,
            EditorKind::Int => s.parse::<i64>().is_ok(),
            EditorKind::Float => s.parse::<f64>().is_ok(),
            // Finite numbers only ("inf"/"NaN" parse as f64 but aren't SQL numerics).
            EditorKind::Decimal => s.parse::<f64>().is_ok_and(f64::is_finite),
            EditorKind::Bool => parse_bool(s).is_some(),
            EditorKind::Date => NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
            EditorKind::Time => valid_time(s),
            EditorKind::DateTime => valid_datetime(s),
        }
    }

    /// Whether a staged [`Value`] is acceptable for this column kind. A final guard before
    /// writing to the database, in case a value reached the staged set some other way.
    pub fn accepts(self, value: &Value) -> bool {
        match (self, value) {
            (_, Value::Null) => true,
            (EditorKind::Text, _) => true,
            (EditorKind::Int, Value::Int(_)) => true,
            (EditorKind::Float, Value::Float(_) | Value::Int(_)) => true,
            (EditorKind::Decimal, Value::Float(_) | Value::Int(_)) => true,
            (EditorKind::Bool, Value::Bool(_)) => true,
            // Dates and decimals are carried as text; validate their string form.
            (
                EditorKind::Date | EditorKind::Time | EditorKind::DateTime | EditorKind::Decimal,
                Value::Text(s),
            ) => self.is_valid(s),
            _ => false,
        }
    }

    /// Placeholder text shown in an empty editor, hinting the expected format.
    pub fn hint(self) -> &'static str {
        match self {
            EditorKind::Text => "",
            EditorKind::Int => "123",
            EditorKind::Float => "1.5",
            EditorKind::Decimal => "123.45",
            EditorKind::Bool => "true / false",
            EditorKind::Date => "YYYY-MM-DD",
            EditorKind::Time => "HH:MM:SS",
            EditorKind::DateTime => "YYYY-MM-DD HH:MM:SS",
        }
    }

    /// Parse edited text into a typed [`Value`]. An empty buffer is `NULL`. Validation is
    /// expected to have run already; unparseable numbers fall back to text.
    pub fn parse(self, buf: &str) -> Value {
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Value::Null;
        }
        match self {
            EditorKind::Int => trimmed
                .parse::<i64>()
                .map(Value::Int)
                .unwrap_or_else(|_| Value::Text(buf.to_string())),
            EditorKind::Float => trimmed
                .parse::<f64>()
                .map(Value::Float)
                .unwrap_or_else(|_| Value::Text(buf.to_string())),
            EditorKind::Bool => {
                parse_bool(trimmed).map_or_else(|| Value::Text(buf.to_string()), Value::Bool)
            }
            // Dates, decimals (exact digits preserved), and free text are stored (and later
            // quoted) as strings.
            EditorKind::Date
            | EditorKind::Time
            | EditorKind::DateTime
            | EditorKind::Decimal
            | EditorKind::Text => Value::Text(buf.to_string()),
        }
    }

    /// Coerce a field read from a file into a typed [`Value`], reporting a failure rather than
    /// falling back to text the way [`EditorKind::parse`] does.
    ///
    /// The empty string is **not** special-cased into `NULL` here — the caller decides, because
    /// the formats disagree: an empty CSV field means NULL (that is what [`crate::export`]
    /// writes for one), while in JSON `null` means NULL and `""` is a genuine empty string.
    /// For every kind but [`EditorKind::Text`] an empty field is an error, so a caller that
    /// forgets to map it gets told rather than silently inserting `''` into a numeric column.
    pub fn parse_strict(self, s: &str) -> Result<Value, CoerceError> {
        let err = || CoerceError {
            expected: self.expected(),
            got: s.to_string(),
        };
        // Text keeps the field byte-for-byte, including surrounding spaces and the empty string.
        if self == EditorKind::Text {
            return Ok(Value::Text(s.to_string()));
        }
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(err());
        }
        match self {
            EditorKind::Text => unreachable!("handled above"),
            EditorKind::Int => trimmed.parse::<i64>().map(Value::Int).map_err(|_| err()),
            EditorKind::Float => trimmed.parse::<f64>().map(Value::Float).map_err(|_| err()),
            // Carried as text so the exact digits survive; validated as a finite number.
            EditorKind::Decimal => match trimmed.parse::<f64>() {
                Ok(f) if f.is_finite() => Ok(Value::Text(trimmed.to_string())),
                _ => Err(err()),
            },
            EditorKind::Bool => parse_bool(trimmed).map(Value::Bool).ok_or_else(err),
            EditorKind::Date | EditorKind::Time | EditorKind::DateTime => {
                if self.is_valid(trimmed) {
                    Ok(Value::Text(trimmed.to_string()))
                } else {
                    Err(err())
                }
            }
        }
    }
}

/// The boolean spellings backends and spreadsheets actually emit.
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "t" | "yes" => Some(true),
        "false" | "0" | "f" | "no" => Some(false),
        _ => None,
    }
}

/// Strip a trailing UTC-offset (`Z`, `+07`, `+07:00`, `-0500`) off a time string, so TIMETZ
/// values like `11:08:39+07` validate. The `+`/`-` of an offset can only appear after the
/// `HH:MM` part, which keeps date separators (`-`) untouched.
fn strip_time_offset(s: &str) -> &str {
    if let Some(base) = s.strip_suffix(['Z', 'z']) {
        return base;
    }
    match s.rfind(['+', '-']) {
        Some(pos)
            if pos >= 5
                && !s[pos + 1..].is_empty()
                && s[pos + 1..].chars().all(|c| c.is_ascii_digit() || c == ':') =>
        {
            &s[..pos]
        }
        _ => s,
    }
}

/// Accept the time shapes backends actually render — with or without fractional seconds,
/// and with an optional trailing UTC offset (TIMETZ).
fn valid_time(s: &str) -> bool {
    let base = strip_time_offset(s);
    NaiveTime::parse_from_str(base, "%H:%M:%S%.f").is_ok()
        || NaiveTime::parse_from_str(base, "%H:%M").is_ok()
}

/// Accept the datetime shapes backends actually render: space- or `T`-separated, optional
/// fractional seconds of any precision, and an optional UTC offset (TIMESTAMPTZ comes back
/// as RFC 3339, psql-style output as `… 11:08:39.59+07`).
fn valid_datetime(s: &str) -> bool {
    const NAIVE: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ];
    const ZONED: &[&str] = &["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%dT%H:%M:%S%.f%#z"];
    NAIVE
        .iter()
        .any(|f| NaiveDateTime::parse_from_str(s, f).is_ok())
        || ZONED
            .iter()
            .any(|f| chrono::DateTime::parse_from_str(s, f).is_ok())
        || chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_orders_overlapping_type_substrings() {
        assert_eq!(EditorKind::classify("BIGINT"), EditorKind::Int);
        assert_eq!(EditorKind::classify("timestamptz"), EditorKind::DateTime);
        assert_eq!(EditorKind::classify("DATE"), EditorKind::Date);
        // INTERVAL and POINT both contain "INT" but are not integers.
        assert_eq!(EditorKind::classify("INTERVAL"), EditorKind::Text);
        assert_eq!(EditorKind::classify("POINT"), EditorKind::Text);
        assert_eq!(EditorKind::classify("NUMERIC(10,2)"), EditorKind::Decimal);
        assert_eq!(EditorKind::classify("bit"), EditorKind::Bool);
    }

    #[test]
    fn parse_strict_rejects_what_parse_silently_downgrades() {
        // The lenient editor path turns a bad integer into text; import must refuse it.
        assert_eq!(
            EditorKind::Int.parse("abc"),
            Value::Text("abc".to_string()),
            "lenient parse falls back to text"
        );
        let err = EditorKind::Int.parse_strict("abc").unwrap_err();
        assert_eq!(err.expected, "an integer");
        assert_eq!(err.got, "abc");

        assert_eq!(EditorKind::Int.parse_strict("42").unwrap(), Value::Int(42));
        assert_eq!(
            EditorKind::Int.parse_strict(" 42 ").unwrap(),
            Value::Int(42),
            "surrounding whitespace is tolerated on numbers"
        );
    }

    #[test]
    fn parse_strict_keeps_decimal_digits_exact() {
        // Round-tripping through f64 would lose these digits, so Decimal stays text.
        let v = EditorKind::Decimal.parse_strict("123.4500").unwrap();
        assert_eq!(v, Value::Text("123.4500".to_string()));
        // ...but it still has to be a finite number.
        assert!(EditorKind::Decimal.parse_strict("1e400").is_err());
        assert!(EditorKind::Decimal.parse_strict("NaN").is_err());
        assert!(EditorKind::Decimal.parse_strict("12.3.4").is_err());
    }

    #[test]
    fn parse_strict_accepts_the_boolean_spellings_a_spreadsheet_emits() {
        for s in ["true", "TRUE", "1", "t", "yes"] {
            assert_eq!(EditorKind::Bool.parse_strict(s).unwrap(), Value::Bool(true));
        }
        for s in ["false", "FALSE", "0", "f", "no"] {
            assert_eq!(EditorKind::Bool.parse_strict(s).unwrap(), Value::Bool(false));
        }
        assert!(EditorKind::Bool.parse_strict("maybe").is_err());
    }

    #[test]
    fn parse_strict_leaves_the_empty_string_to_the_caller() {
        // Text keeps it verbatim (JSON's "" is a real empty string)...
        assert_eq!(
            EditorKind::Text.parse_strict("").unwrap(),
            Value::Text(String::new())
        );
        // ...and every other kind errors rather than inserting '' into a numeric column.
        // Callers map empty -> NULL before reaching here (see import::coerce_record).
        assert!(EditorKind::Int.parse_strict("").is_err());
        assert!(EditorKind::Bool.parse_strict("   ").is_err());
        assert!(EditorKind::Date.parse_strict("").is_err());
    }

    #[test]
    fn parse_strict_preserves_text_verbatim() {
        // No trimming: a text column may legitimately store padded strings.
        assert_eq!(
            EditorKind::Text.parse_strict("  hi  ").unwrap(),
            Value::Text("  hi  ".to_string())
        );
    }

    #[test]
    fn parse_strict_validates_temporal_shapes() {
        assert_eq!(
            EditorKind::Date.parse_strict("2026-07-10").unwrap(),
            Value::Text("2026-07-10".to_string())
        );
        assert!(EditorKind::Date.parse_strict("10/07/2026").is_err());

        // TIMETZ / TIMESTAMPTZ shapes the backends actually render.
        assert!(EditorKind::Time.parse_strict("11:08:39+07").is_ok());
        assert!(EditorKind::DateTime
            .parse_strict("2026-07-10 11:08:39.59+07")
            .is_ok());
        assert!(EditorKind::DateTime.parse_strict("not a time").is_err());
    }

    #[test]
    fn accepts_guards_staged_values() {
        assert!(EditorKind::Int.accepts(&Value::Int(1)));
        assert!(EditorKind::Int.accepts(&Value::Null));
        assert!(!EditorKind::Int.accepts(&Value::Text("1".into())));
        assert!(EditorKind::Date.accepts(&Value::Text("2026-07-10".into())));
        assert!(!EditorKind::Date.accepts(&Value::Text("nope".into())));
    }
}
