//! In-flight, TablePlus-style cell editing state.
//!
//! Edits are *staged*, not written through immediately: changed cells are remembered in
//! [`Edits::cells`] (and their rows highlight green in the grid) until the user saves with
//! Cmd/Ctrl+S, at which point the app turns them into `UPDATE` statements. Editing is only
//! possible when the current result came from a single table opened from the sidebar, so we
//! know the table and its primary key — that source travels in [`EditSource`].

use std::collections::HashMap;

use dbcore::Value;

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
    pub buf: String,
    /// Set when the editor first opens so the widget can grab keyboard focus once.
    pub focus: bool,
}

/// All editing state for the current result.
#[derive(Default)]
pub struct Edits {
    /// Source of the *current* result (`None` ⇒ not editable, e.g. an ad-hoc query).
    pub source: Option<EditSource>,
    /// Source of the query currently in flight; promoted to `source` when it returns.
    pub pending_source: Option<EditSource>,
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

    /// The staged value for a cell, if it has an uncommitted edit.
    pub fn staged(&self, row: usize, col: usize) -> Option<&Value> {
        self.cells.get(&row).and_then(|m| m.get(&col))
    }

    pub fn is_active(&self, row: usize, col: usize) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.row == row && a.col == col)
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
            buf,
            focus: true,
        });
    }

    /// Commit the active editor into the staged set, typing the input like `original`. If the
    /// result equals `original` the cell is left (or reverted to) unchanged.
    pub fn commit_active(&mut self, original: &Value) {
        let Some(active) = self.active.take() else {
            return;
        };
        let new = parse_like(&active.buf, original);
        let entry = self.cells.entry(active.row).or_default();
        if &new == original {
            entry.remove(&active.col);
            if entry.is_empty() {
                self.cells.remove(&active.row);
            }
        } else {
            entry.insert(active.col, new);
        }
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

/// Parse edited text into a [`Value`] of the same flavour as `original`, so a numeric column
/// stays numeric in the generated SQL. Unparseable input falls back to text.
fn parse_like(buf: &str, original: &Value) -> Value {
    let trimmed = buf.trim();
    match original {
        Value::Int(_) => buf
            .trim()
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(buf.to_string())),
        Value::Float(_) => trimmed
        
            .parse::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Text(buf.to_string())),
        Value::Bool(_) => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" | "t" | "yes" => Value::Bool(true),
            "false" | "0" | "f" | "no" => Value::Bool(false),
            _ => Value::Text(buf.to_string()),
        },
        // A previously-NULL cell has no type to follow: infer number, else keep as text. An
        // empty buffer means "still NULL".
        Value::Null => {
            if buf.is_empty() {
                Value::Null
            } else if let Ok(i) = trimmed.parse::<i64>() {
                Value::Int(i)
            } else if let Ok(f) = trimmed.parse::<f64>() {
                Value::Float(f)
            } else {
                Value::Text(buf.to_string())
            }
        }
        Value::Text(_) => Value::Text(buf.to_string()),
        // Binary cells are never editable; preserve them untouched.
        Value::Bytes(_) => original.clone(),
    }
}
