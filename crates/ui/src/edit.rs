//! In-flight, TablePlus-style cell editing state.
//!
//! Edits are *staged*, not written through immediately: changed cells are remembered in
//! [`Edits::cells`] (and their rows highlight green in the grid) until the user saves with
//! Cmd/Ctrl+S, at which point the app turns them into `UPDATE` statements. Editing is only
//! possible when the current result came from a single table opened from the sidebar, so we
//! know the table and its primary key — that source travels in [`EditSource`].
//!
//! Each column is classified once per result into an [`EditorKind`] so the editor can be
//! type-aware: booleans toggle on double-click, numbers and dates are validated before they
//! can be staged, and everything else is free text.

use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use crate::style::palette;
use dbcore::{ColumnMeta, Value};

/// How a column should be edited, derived from its backend type name. Computed once when a
/// result loads (cheap, and avoids re-parsing the type string every frame).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorKind {
    #[default]
    Text,
    Int,
    Float,
    Bool,
    Date,
    Time,
    DateTime,
}

impl EditorKind {
    /// Classify a backend type name (e.g. `"BIGINT"`, `"timestamp"`, `"bit"`). Order matters:
    /// `DATETIME`/`TIMESTAMP` must be matched before the bare `DATE`/`TIME` substrings, and
    /// `DECIMAL`/`NUMERIC` deliberately stay [`EditorKind::Text`] to preserve exact precision.
    pub fn classify(type_name: &str) -> EditorKind {
        let t = type_name.to_ascii_uppercase();
        if t.contains("BOOL") || t == "BIT" {
            EditorKind::Bool
        } else if t.contains("DATETIME") || t.contains("TIMESTAMP") {
            EditorKind::DateTime
        } else if t.contains("DATE") {
            EditorKind::Date
        } else if t.contains("TIME") {
            EditorKind::Time
        } else if t.contains("DECIMAL") || t.contains("NUMERIC") || t.contains("MONEY") {
            EditorKind::Text
        } else if t.contains("INT") || t.contains("SERIAL") {
            EditorKind::Int
        } else if t.contains("FLOAT") || t.contains("DOUBLE") || t.contains("REAL") {
            EditorKind::Float
        } else {
            EditorKind::Text
        }
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
            EditorKind::Bool => matches!(
                s.to_ascii_lowercase().as_str(),
                "true" | "false" | "0" | "1" | "t" | "f" | "yes" | "no"
            ),
            EditorKind::Date => NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
            EditorKind::Time => {
                NaiveTime::parse_from_str(s, "%H:%M:%S").is_ok()
                    || NaiveTime::parse_from_str(s, "%H:%M").is_ok()
            }
            EditorKind::DateTime => {
                NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").is_ok()
                    || NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
                    || NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").is_ok()
            }
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
            (EditorKind::Bool, Value::Bool(_)) => true,
            // Dates are carried as text; validate their string form.
            (EditorKind::Date | EditorKind::Time | EditorKind::DateTime, Value::Text(s)) => {
                self.is_valid(s)
            }
            _ => false,
        }
    }

    /// Placeholder text shown in an empty editor, hinting the expected format.
    fn hint(self) -> &'static str {
        match self {
            EditorKind::Text => "",
            EditorKind::Int => "123",
            EditorKind::Float => "1.5",
            EditorKind::Bool => "true / false",
            EditorKind::Date => "YYYY-MM-DD",
            EditorKind::Time => "HH:MM:SS",
            EditorKind::DateTime => "YYYY-MM-DD HH:MM:SS",
        }
    }

    /// Parse edited text into a typed [`Value`]. An empty buffer is `NULL`. Validation is
    /// expected to have run already; unparseable numbers fall back to text.
    fn parse(self, buf: &str) -> Value {
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
            EditorKind::Bool => match trimmed.to_ascii_lowercase().as_str() {
                "true" | "1" | "t" | "yes" => Value::Bool(true),
                "false" | "0" | "f" | "no" => Value::Bool(false),
                _ => Value::Text(buf.to_string()),
            },
            // Dates and free text are stored (and later quoted) as strings.
            EditorKind::Date | EditorKind::Time | EditorKind::DateTime | EditorKind::Text => {
                Value::Text(buf.to_string())
            }
        }
    }
}

/// Read the boolean sense of a cell value, for toggling.
fn as_bool(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Text(s) => matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "t" | "yes"),
        _ => false,
    }
}

/// The table a result was read from, plus the primary-key columns needed to target rows in
/// an `UPDATE`. Built when a table is opened from the schema sidebar.
#[derive(Clone)]
pub struct EditSource {
    pub schema: Option<String>,
    pub table: String,
    /// Names of the primary-key columns. Empty ⇒ no PK ⇒ rows can't be edited.
    pub pk_cols: Vec<String>,
}

impl EditSource {
    pub fn editable(&self) -> bool {
        !self.pk_cols.is_empty()
    }
}

/// The cell currently being typed into (only ever one at a time, across grid and details).
pub struct ActiveEdit {
    /// Index into `result.rows` (the *raw* row, not the display order).
    pub row: usize,
    pub col: usize,
    pub kind: EditorKind,
    pub buf: String,
    /// Set when the editor first opens so the widget can grab keyboard focus once.
    pub focus: bool,
}

/// What [`render_editor`] decided this frame.
pub enum EditOutcome {
    /// Keep editing.
    Continue,
    /// Finish and stage the value.
    Commit,
    /// Abandon the edit.
    Cancel,
}

/// All editing state for the current result.
#[derive(Default)]
pub struct Edits {
    /// Source of the *current* result (`None` ⇒ not editable, e.g. an ad-hoc query).
    pub source: Option<EditSource>,
    /// Source of the query currently in flight; promoted to `source` when it returns.
    pub pending_source: Option<EditSource>,
    /// Per-column editor kind, indexed like `result.columns`.
    col_kinds: Vec<EditorKind>,
    /// Staged changes: raw row index → column index → new value.
    pub cells: HashMap<usize, HashMap<usize, Value>>,
    /// The cell open in a text editor right now.
    pub active: Option<ActiveEdit>,
}

impl Edits {
    /// Whether the current result's rows can be edited at all.
    pub fn editable(&self) -> bool {
        self.source.as_ref().is_some_and(EditSource::editable)
    }

    pub fn has_pending(&self) -> bool {
        !self.cells.is_empty()
    }

    pub fn row_dirty(&self, row: usize) -> bool {
        self.cells.contains_key(&row)
    }

    /// Recompute the per-column editor kinds for a freshly loaded result.
    pub fn set_columns(&mut self, columns: &[ColumnMeta]) {
        self.col_kinds = columns
            .iter()
            .map(|c| EditorKind::classify(&c.type_name))
            .collect();
    }

    pub fn col_kind(&self, col: usize) -> EditorKind {
        self.col_kinds.get(col).copied().unwrap_or_default()
    }

    /// The staged value for a cell, if it has an uncommitted edit.
    pub fn staged(&self, row: usize, col: usize) -> Option<&Value> {
        self.cells.get(&row).and_then(|m| m.get(&col))
    }

    pub fn is_active(&self, row: usize, col: usize) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.row == row && a.col == col)
    }

    /// Stage `new` for `(row, col)`, or clear the staged edit if it equals `original`.
    fn stage(&mut self, row: usize, col: usize, new: Value, original: &Value) {
        let entry = self.cells.entry(row).or_default();
        if &new == original {
            entry.remove(&col);
            if entry.is_empty() {
                self.cells.remove(&row);
            }
        } else {
            entry.insert(col, new);
        }
    }

    /// Flip a boolean cell and stage the result immediately (no text editor needed).
    pub fn toggle_bool(&mut self, row: usize, col: usize, original: &Value) {
        let current = self.staged(row, col).map(as_bool).unwrap_or(as_bool(original));
        self.stage(row, col, Value::Bool(!current), original);
    }

    /// Open an editor on `(row, col)`, seeding the buffer from the cell's current value.
    pub fn begin(&mut self, row: usize, col: usize, current: &Value) {
        let buf = match current {
            Value::Null => String::new(),
            other => other.display(),
        };
        self.active = Some(ActiveEdit {
            row,
            col,
            kind: self.col_kind(col),
            buf,
            focus: true,
        });
    }

    /// Commit the active editor into the staged set, typing the input by its column kind. If
    /// the result equals `original` the cell is left (or reverted to) unchanged.
    ///
    /// Returns `false` — leaving the editor open — when the input is invalid for the column,
    /// so an invalid (red) value can never be staged or saved.
    pub fn commit_active(&mut self, original: &Value) -> bool {
        let Some(active) = self.active.as_ref() else {
            return true;
        };
        if !active.kind.is_valid(&active.buf) {
            return false;
        }
        let active = self.active.take().expect("active checked above");
        let new = active.kind.parse(&active.buf);
        self.stage(active.row, active.col, new, original);
        true
    }

    pub fn cancel_active(&mut self) {
        self.active = None;
    }

    /// Drop all staged edits and any open editor (e.g. after a successful save or a reload).
    pub fn clear(&mut self) {
        self.cells.clear();
        self.active = None;
    }
}

/// Render the active text editor (numbers, dates, free text) and report what to do next.
/// Invalid input (per the column kind) is shown in the danger colour and can't be committed
/// by pressing Enter; clicking away from invalid input discards the edit. `fill`, when set,
/// sizes the field to exactly that rect (used to fill a grid cell).
pub fn render_editor(
    ui: &mut egui::Ui,
    active: &mut ActiveEdit,
    fill: Option<egui::Vec2>,
) -> EditOutcome {
    // Slightly rounded corners so the field reads as an input box, not a thin strip.
    {
        let cr = egui::CornerRadius::same(3);
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.corner_radius = cr;
        w.hovered.corner_radius = cr;
        w.active.corner_radius = cr;
    }

    let valid = active.kind.is_valid(&active.buf);
    let mut field = egui::TextEdit::singleline(&mut active.buf).hint_text(active.kind.hint());
    if !valid {
        field = field.text_color(palette::DANGER());
    }
    let resp = match fill {
        Some(size) => ui.add_sized(size, field.margin(egui::vec2(4.0, 2.0))),
        None => ui.add(field.desired_width(f32::INFINITY).margin(egui::vec2(4.0, 3.0))),
    };
    if active.focus {
        resp.request_focus();
        active.focus = false;
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return EditOutcome::Cancel;
    }
    if resp.lost_focus() {
        if valid {
            return EditOutcome::Commit;
        }
        // Enter on invalid input keeps the editor open so it can be fixed; losing focus by
        // clicking elsewhere discards it.
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            active.focus = true;
            return EditOutcome::Continue;
        }
        return EditOutcome::Cancel;
    }
    EditOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_backend_types() {
        use EditorKind::*;
        assert_eq!(EditorKind::classify("BIGINT"), Int);
        assert_eq!(EditorKind::classify("integer"), Int);
        assert_eq!(EditorKind::classify("DOUBLE PRECISION"), Float);
        assert_eq!(EditorKind::classify("bit"), Bool);
        assert_eq!(EditorKind::classify("boolean"), Bool);
        // TIMESTAMP/DATETIME win over the bare DATE/TIME substrings they contain.
        assert_eq!(EditorKind::classify("timestamp"), DateTime);
        assert_eq!(EditorKind::classify("DATETIME2"), DateTime);
        assert_eq!(EditorKind::classify("date"), Date);
        assert_eq!(EditorKind::classify("time"), Time);
        // DECIMAL stays text to keep precision; varchar/json are text.
        assert_eq!(EditorKind::classify("decimal(10,2)"), Text);
        assert_eq!(EditorKind::classify("varchar"), Text);
    }

    #[test]
    fn validation_gates_numbers_and_dates() {
        assert!(EditorKind::Int.is_valid("42"));
        assert!(!EditorKind::Int.is_valid("4.2"));
        assert!(EditorKind::Float.is_valid("4.2"));
        assert!(!EditorKind::Float.is_valid("abc"));
        assert!(EditorKind::Date.is_valid("2024-06-09"));
        assert!(!EditorKind::Date.is_valid("09/06/2024"));
        assert!(EditorKind::DateTime.is_valid("2024-06-09 13:45:00"));
        // Empty is always valid — it means NULL.
        assert!(EditorKind::Int.is_valid(""));
    }

    #[test]
    fn invalid_input_cannot_be_staged() {
        let mut e = Edits::default();
        e.set_columns(&[dbcore::ColumnMeta {
            name: "age".into(),
            type_name: "INT".into(),
        }]);
        e.begin(0, 0, &Value::Int(30));
        // Type something invalid for an INT column.
        e.active.as_mut().unwrap().buf = "abc".into();
        // commit refuses: returns false, leaves the editor open, stages nothing.
        assert!(!e.commit_active(&Value::Int(30)));
        assert!(e.active.is_some());
        assert!(!e.has_pending());

        // Fix it, and now it commits.
        e.active.as_mut().unwrap().buf = "31".into();
        assert!(e.commit_active(&Value::Int(30)));
        assert_eq!(e.staged(0, 0), Some(&Value::Int(31)));

        // The final write-guard rejects a value of the wrong shape for the column.
        assert!(!EditorKind::Int.accepts(&Value::Text("31".into())));
        assert!(EditorKind::Int.accepts(&Value::Int(31)));
    }

    #[test]
    fn toggle_bool_flips_and_clears() {
        let mut e = Edits::default();
        let original = Value::Bool(false);
        e.toggle_bool(0, 0, &original);
        assert_eq!(e.staged(0, 0), Some(&Value::Bool(true)));
        // Toggling back to the original value clears the staged edit.
        e.toggle_bool(0, 0, &original);
        assert_eq!(e.staged(0, 0), None);
        assert!(!e.has_pending());
    }
}
