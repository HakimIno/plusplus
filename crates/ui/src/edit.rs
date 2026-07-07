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
            EditorKind::Bool => matches!(
                s.to_ascii_lowercase().as_str(),
                "true" | "false" | "0" | "1" | "t" | "f" | "yes" | "no"
            ),
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
    fn hint(self) -> &'static str {
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
            // Dates, decimals (exact digits preserved), and free text are stored (and later
            // quoted) as strings.
            EditorKind::Date
            | EditorKind::Time
            | EditorKind::DateTime
            | EditorKind::Decimal
            | EditorKind::Text => Value::Text(buf.to_string()),
        }
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

/// Read the boolean sense of a cell value, for toggling and checkbox display.
pub(crate) fn as_bool(value: &Value) -> bool {
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
/// Where an edit was started from. The grid and the Details panel can both display the
/// active cell; only the view that began the edit renders the text editor (two live
/// editors over one buffer would fight over keyboard focus).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EditOrigin {
    Grid,
    Details,
}

pub struct ActiveEdit {
    /// Index into `result.rows` (the *raw* row, not the display order).
    pub row: usize,
    pub col: usize,
    pub kind: EditorKind,
    pub buf: String,
    /// Which view opened this editor (that view renders it; the other shows a label).
    pub origin: EditOrigin,
}

/// Where the cell cursor should move after a commit (Tab / Shift+Tab advance).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorDir {
    Left,
    Right,
}

/// What [`render_editor`] decided this frame.
pub enum EditOutcome {
    /// Keep editing.
    Continue,
    /// Finish and stage the value; `advance` asks the caller to move the cell cursor and
    /// continue editing there (Tab/Shift+Tab), `None` commits in place.
    Commit { advance: Option<CursorDir> },
    /// Abandon the edit.
    Cancel,
}

/// Row indices at or above this base address *new* (to-be-inserted) rows rather than rows
/// in `result.rows`. Keeping new rows in the same `usize` address space as stored rows lets
/// the staging map, the active editor, and the grid all stay `usize`-keyed; helpers below
/// translate back to the new-row slot when needed.
pub const NEW_ROW_BASE: usize = 1 << 48;

/// Whether `row` addresses a new (insert) row rather than a stored result row.
pub fn is_new_row(row: usize) -> bool {
    row >= NEW_ROW_BASE
}

/// How a row should be painted / treated, derived from the pending edits on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowState {
    /// No pending changes.
    Clean,
    /// A stored row with staged cell edits (will become an `UPDATE`).
    Edited,
    /// A stored row marked for deletion (will become a `DELETE`).
    Deleted,
    /// A brand-new row being filled in (will become an `INSERT`).
    New,
}

/// One reversible mutation of the staged-edit state, recorded as it happens so
/// Cmd/Ctrl+Z can walk back through them. Each op carries both sides of the change
/// (`before`/`after`, the cleared cells, the removed row's contents) so it can be
/// applied in either direction.
#[derive(Clone, Debug)]
enum EditOp {
    /// The staged value at `(row, col)` changed (`None` ⇒ no staged edit).
    Cell {
        row: usize,
        col: usize,
        before: Option<Value>,
        after: Option<Value>,
    },
    /// `row` was marked for deletion, dropping its staged edits (`cleared`).
    MarkDelete {
        row: usize,
        cleared: HashMap<usize, Value>,
    },
    /// `row`'s deletion mark was removed.
    UnmarkDelete { row: usize },
    /// A new (insert) row was appended (always at the top slot).
    AddRow,
    /// The new row at `slot` (0-based) was removed; `cells` were its entered values.
    RemoveRow {
        slot: usize,
        cells: HashMap<usize, Value>,
    },
}

/// Steps beyond this are forgotten, oldest first — one fill over a huge row range can
/// hold a lot of `Cell` ops, and the history must not outgrow the edits themselves.
const MAX_UNDO_STEPS: usize = 100;

/// Undo/redo history over the staged-edit state. Each step holds the [`EditOp`]s one user
/// action produced: usually a single op, but a multi-row action (Backspace over a
/// selection, paste, fill, Esc-discard) groups all of its ops into one step.
#[derive(Default)]
struct History {
    undo: Vec<Vec<EditOp>>,
    redo: Vec<Vec<EditOp>>,
    /// Ops of the step currently being built; flushed when `depth` returns to 0.
    pending: Vec<EditOp>,
    /// [`Edits::begin_undo_group`] nesting depth. At 0 every op flushes as its own step.
    depth: usize,
}

impl History {
    fn record(&mut self, op: EditOp) {
        self.pending.push(op);
        if self.depth == 0 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        self.undo.push(std::mem::take(&mut self.pending));
        // A fresh edit invalidates anything that was undone.
        self.redo.clear();
        if self.undo.len() > MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
    }

    fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending.clear();
        self.depth = 0;
    }
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
    /// Staged changes: row index → column index → new value. Row indices below
    /// [`NEW_ROW_BASE`] are stored rows (a diff against the original); indices at/above it
    /// are new rows (the full set of entered cells).
    pub cells: HashMap<usize, HashMap<usize, Value>>,
    /// Stored rows (raw indices) marked for deletion.
    pub deleted: std::collections::HashSet<usize>,
    /// Number of new rows; their ids are `NEW_ROW_BASE .. NEW_ROW_BASE + new_rows`.
    pub new_rows: usize,
    /// The cell open in a text editor right now.
    pub active: Option<ActiveEdit>,
    /// Undo/redo history over the staging state above (cleared on save/reload).
    history: History,
}

impl Edits {
    /// Whether the current result's rows can be edited at all.
    pub fn editable(&self) -> bool {
        self.source.as_ref().is_some_and(EditSource::editable)
    }

    pub fn has_pending(&self) -> bool {
        self.new_rows > 0
            || !self.deleted.is_empty()
            || self
                .cells
                .iter()
                .any(|(row, m)| !is_new_row(*row) && !m.is_empty())
    }

    pub fn row_dirty(&self, row: usize) -> bool {
        self.cells.get(&row).is_some_and(|m| !m.is_empty())
    }

    /// How `row` should be painted/treated given the pending edits on it.
    pub fn row_state(&self, row: usize) -> RowState {
        if is_new_row(row) {
            RowState::New
        } else if self.deleted.contains(&row) {
            RowState::Deleted
        } else if self.row_dirty(row) {
            RowState::Edited
        } else {
            RowState::Clean
        }
    }

    /// Toggle a stored row's deletion mark. Clears any staged cell edits on it (a deleted
    /// row's edits are moot) and closes the editor if it sat on this row.
    pub fn toggle_delete(&mut self, row: usize) {
        if is_new_row(row) {
            return;
        }
        if self.deleted.remove(&row) {
            self.history.record(EditOp::UnmarkDelete { row });
        } else {
            self.deleted.insert(row);
            let cleared = self.cells.remove(&row).unwrap_or_default();
            if self.active.as_ref().is_some_and(|a| a.row == row) {
                self.active = None;
            }
            self.history.record(EditOp::MarkDelete { row, cleared });
        }
    }

    /// Append a new (empty) insert row and return its id.
    pub fn add_new_row(&mut self) -> usize {
        let id = self.add_new_row_raw();
        self.history.record(EditOp::AddRow);
        id
    }

    /// Remove the new row with the given id, renumbering the new rows above it so their ids
    /// stay contiguous (and fixing the active editor if it pointed into them).
    pub fn remove_new_row(&mut self, id: usize) {
        if !is_new_row(id) {
            return;
        }
        let slot = id - NEW_ROW_BASE;
        if slot >= self.new_rows {
            return;
        }
        let cells = self.remove_new_row_raw(id);
        self.history.record(EditOp::RemoveRow { slot, cells });
    }

    /// Append a new (empty) insert row and return its id, without recording history.
    fn add_new_row_raw(&mut self) -> usize {
        let id = NEW_ROW_BASE + self.new_rows;
        self.new_rows += 1;
        self.cells.entry(id).or_default();
        id
    }

    /// Remove new row `id` (renumbering the rows above it), without recording history.
    /// Returns the removed row's staged cells so history can restore them on undo. Assumes
    /// the caller has already checked `id` addresses a live new row.
    fn remove_new_row_raw(&mut self, id: usize) -> HashMap<usize, Value> {
        let j = id - NEW_ROW_BASE;
        let removed = self.cells.remove(&id).unwrap_or_default();
        for k in (j + 1)..self.new_rows {
            if let Some(m) = self.cells.remove(&(NEW_ROW_BASE + k)) {
                self.cells.insert(NEW_ROW_BASE + k - 1, m);
            }
        }
        if let Some(a) = self.active.as_mut() {
            if is_new_row(a.row) {
                let aj = a.row - NEW_ROW_BASE;
                if aj == j {
                    self.active = None;
                } else if aj > j {
                    a.row -= 1;
                }
            }
        }
        self.new_rows -= 1;
        removed
    }

    /// Re-insert a new row at `slot`, sliding the rows at/above it up by one and restoring
    /// its `cells`. The inverse of [`Self::remove_new_row_raw`]; never records history.
    fn insert_new_row_at(&mut self, slot: usize, cells: HashMap<usize, Value>) {
        let slot = slot.min(self.new_rows);
        for k in (slot..self.new_rows).rev() {
            if let Some(m) = self.cells.remove(&(NEW_ROW_BASE + k)) {
                self.cells.insert(NEW_ROW_BASE + k + 1, m);
            }
        }
        if let Some(a) = self.active.as_mut() {
            if is_new_row(a.row) && (a.row - NEW_ROW_BASE) >= slot {
                a.row += 1;
            }
        }
        self.cells.insert(NEW_ROW_BASE + slot, cells);
        self.new_rows += 1;
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
    /// Public so type-aware widgets (the Details panel's date picker and checkboxes) can
    /// stage a value directly, without going through a text editor.
    pub fn stage(&mut self, row: usize, col: usize, new: Value, original: &Value) {
        let before = self.staged(row, col).cloned();
        let after = if &new == original { None } else { Some(new) };
        if after == before {
            return;
        }
        self.set_staged(row, col, after.clone());
        self.history.record(EditOp::Cell {
            row,
            col,
            before,
            after,
        });
    }

    /// Write (or clear, when `value` is `None`) a cell's staged value directly, without
    /// recording history. The primitive that [`Self::stage`] and undo/redo both build on.
    fn set_staged(&mut self, row: usize, col: usize, value: Option<Value>) {
        match value {
            Some(v) => {
                self.cells.entry(row).or_default().insert(col, v);
            }
            None => {
                if let Some(entry) = self.cells.get_mut(&row) {
                    entry.remove(&col);
                    if entry.is_empty() {
                        self.cells.remove(&row);
                    }
                }
            }
        }
    }

    /// Stage `text` into `(row, col)` as a value typed by the column's kind (empty → NULL),
    /// without opening an editor. Used by paste-to-insert to fill a new row's cells from
    /// clipboard text. New rows have no stored value, so NULL is the baseline (a non-NULL
    /// value stages; NULL clears).
    pub fn stage_text(&mut self, row: usize, col: usize, text: &str) {
        let value = self.col_kind(col).parse(text);
        self.stage(row, col, value, &Value::Null);
    }

    /// Flip a boolean cell and stage the result immediately (no text editor needed).
    pub fn toggle_bool(&mut self, row: usize, col: usize, original: &Value) {
        let current = self.staged(row, col).map(as_bool).unwrap_or(as_bool(original));
        self.stage(row, col, Value::Bool(!current), original);
    }

    /// Open an editor on `(row, col)`, seeding the buffer from the cell's current value.
    /// `origin` is the view that should render the editor (grid or Details panel).
    pub fn begin(&mut self, row: usize, col: usize, current: &Value, origin: EditOrigin) {
        let buf = match current {
            Value::Null => String::new(),
            other => other.display(),
        };
        self.active = Some(ActiveEdit {
            row,
            col,
            kind: self.col_kind(col),
            buf,
            origin,
        });
    }

    /// Whether `(row, col)` is being edited *and* `origin` is the view that opened the
    /// editor — i.e. the view that should render it.
    pub fn is_active_from(&self, row: usize, col: usize, origin: EditOrigin) -> bool {
        self.is_active(row, col)
            && self
                .active
                .as_ref()
                .is_some_and(|a| a.origin == origin)
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
    /// The staged rows are gone for good, so the undo history is dropped too — undoing into a
    /// state that referenced saved/reloaded rows would be meaningless.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.deleted.clear();
        self.new_rows = 0;
        self.active = None;
        self.history.clear();
    }

    /// Revert *all* pending edits (the Esc "discard" action) as one undoable step: staged
    /// cell edits drop, deletion marks lift, and new rows are removed. Unlike [`Self::clear`]
    /// this is recorded, so an accidental discard can be taken back with Cmd/Ctrl+Z.
    pub fn discard_all(&mut self) {
        if !self.has_pending() {
            return;
        }
        self.active = None;
        self.begin_undo_group();
        // Clear staged cell edits on stored rows (new rows are handled by removal below).
        let dirty: Vec<(usize, usize)> = self
            .cells
            .iter()
            .filter(|(row, _)| !is_new_row(**row))
            .flat_map(|(&row, m)| m.keys().map(move |&col| (row, col)))
            .collect();
        for (row, col) in dirty {
            if let Some(before) = self.staged(row, col).cloned() {
                self.set_staged(row, col, None);
                self.history.record(EditOp::Cell {
                    row,
                    col,
                    before: Some(before),
                    after: None,
                });
            }
        }
        // Lift every deletion mark.
        let deleted: Vec<usize> = self.deleted.iter().copied().collect();
        for row in deleted {
            self.deleted.remove(&row);
            self.history.record(EditOp::UnmarkDelete { row });
        }
        // Remove new rows from the top down, so each removal leaves the lower ids intact.
        while self.new_rows > 0 {
            self.remove_new_row(NEW_ROW_BASE + self.new_rows - 1);
        }
        self.end_undo_group();
    }

    /// Whether there is a step to undo / redo (drives menu + shortcut enablement).
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Open an undo group: every mutation until the matching [`Self::end_undo_group`] folds
    /// into a single undo step. Used to make a multi-row action (paste, fill, delete over a
    /// selection) undo in one keystroke rather than row by row. Groups may nest.
    pub fn begin_undo_group(&mut self) {
        self.history.depth += 1;
    }

    pub fn end_undo_group(&mut self) {
        self.history.depth = self.history.depth.saturating_sub(1);
        if self.history.depth == 0 {
            self.history.flush();
        }
    }

    /// Undo the most recent step (its ops reversed, newest first). Closes any open editor
    /// first. Returns whether anything was undone, so the app can refresh its view.
    pub fn undo(&mut self) -> bool {
        // A partly-built group (shouldn't happen between frames) is flushed so it can undo.
        self.history.flush();
        let Some(step) = self.history.undo.pop() else {
            return false;
        };
        self.active = None;
        for op in step.iter().rev() {
            self.apply_op(op, false);
        }
        self.history.redo.push(step);
        true
    }

    /// Redo the step undone most recently (its ops replayed in original order).
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.history.redo.pop() else {
            return false;
        };
        self.active = None;
        for op in &step {
            self.apply_op(op, true);
        }
        self.history.undo.push(step);
        true
    }

    /// Apply one recorded op. `forward` replays it (redo); `!forward` inverts it (undo).
    /// Uses only the raw, non-recording mutators so undo/redo never feed back into history.
    fn apply_op(&mut self, op: &EditOp, forward: bool) {
        match op {
            EditOp::Cell {
                row,
                col,
                before,
                after,
            } => {
                let target = if forward { after } else { before };
                self.set_staged(*row, *col, target.clone());
            }
            EditOp::MarkDelete { row, cleared } => {
                if forward {
                    self.cells.remove(row);
                    self.deleted.insert(*row);
                } else {
                    self.deleted.remove(row);
                    if !cleared.is_empty() {
                        self.cells.insert(*row, cleared.clone());
                    }
                }
            }
            EditOp::UnmarkDelete { row } => {
                if forward {
                    self.deleted.remove(row);
                } else {
                    self.deleted.insert(*row);
                }
            }
            EditOp::AddRow => {
                if forward {
                    self.add_new_row_raw();
                } else if self.new_rows > 0 {
                    self.remove_new_row_raw(NEW_ROW_BASE + self.new_rows - 1);
                }
            }
            EditOp::RemoveRow { slot, cells } => {
                if forward {
                    if *slot < self.new_rows {
                        self.remove_new_row_raw(NEW_ROW_BASE + *slot);
                    }
                } else {
                    self.insert_new_row_at(*slot, cells.clone());
                }
            }
        }
    }
}

/// Horizontal inset for value text in the Details panel (display paint + editor must match).
pub const DETAILS_VALUE_PAD_X: f32 = 8.0;

/// Render the active text editor (numbers, dates, free text) and report what to do next.
/// Invalid input (per the column kind) is shown in the danger colour and can't be committed
/// by pressing Enter; clicking away from invalid input discards the edit. `fill`, when set,
/// sizes the field to exactly that rect (used to fill a grid cell or a Details value box).
/// Details-panel editors are frameless — that panel paints the surrounding box itself so
/// focus doesn't add a second border. Grid cells keep a normal input frame.
pub fn render_editor(
    ui: &mut egui::Ui,
    active: &mut ActiveEdit,
    fill: Option<egui::Vec2>,
) -> EditOutcome {
    let valid = active.kind.is_valid(&active.buf);
    // Tab / Shift+Tab: commit and advance to the neighbouring cell, spreadsheet-style.
    // Consumed *before* the TextEdit is built so egui's focus traversal never sees it.
    // Invalid input swallows the Tab and stays put — same rule as Enter-on-invalid below.
    let advance = ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
            Some(CursorDir::Left)
        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            Some(CursorDir::Right)
        } else {
            None
        }
    });
    if advance.is_some() && valid {
        return EditOutcome::Commit { advance };
    }
    let embedded = fill.is_some();
    let is_details = active.origin == EditOrigin::Details;
    let mut field = egui::TextEdit::singleline(&mut active.buf)
        .hint_text(active.kind.hint())
        .id_salt((active.row, active.col, active.origin))
        // Keep Tab out of egui's focus traversal (which latches it at frame start, before
        // the consume_key above could run): the editor's event filter absorbs it, and the
        // consume_key prevents a literal '\t' from reaching the field.
        .lock_focus(true)
        .vertical_align(egui::Align::Center);
    if !valid {
        field = field.text_color(palette::DANGER());
    }
    if embedded && is_details {
        // Margin on the builder is ignored when a custom frame is set — use inner_margin
        // on a frameless frame so text lines up with display mode (left + DETAILS_VALUE_PAD_X).
        field = field
            .horizontal_align(egui::Align::LEFT)
            .vertical_align(egui::Align::Center)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(
                DETAILS_VALUE_PAD_X.round() as i8,
                0,
            )));
    } else {
        // A thin accent border marks the cell under edit — it reads as "active/primary"
        // against the grid. Invalid input swaps to the danger colour (not just red text) so
        // the editor reads as "blocked" at a glance.
        let border = if valid {
            palette::ACCENT()
        } else {
            palette::DANGER()
        };
        let cr = egui::CornerRadius::ZERO;
        // In the grid the editor is given the whole cell size, but a singleline field is only
        // as tall as its text — `add_sized` would then centre a short pill inside the cell,
        // leaving a gap above and below. Grow the frame's vertical padding to the cell height
        // so the border hugs the cell edges exactly.
        let mut inner = egui::Margin::symmetric(4, 0);
        if let Some(size) = fill {
            let text_h = ui.text_style_height(&egui::TextStyle::Body);
            let vpad = ((size.y - text_h) / 2.0).clamp(0.0, 24.0).round() as i8;
            inner.top = vpad;
            inner.bottom = vpad;
        }
        field = field.frame(
            egui::Frame::new()
                .fill(palette::CODE_BG())
                .stroke(egui::Stroke::new(1.0, border))
                .corner_radius(cr)
                .inner_margin(inner),
        );
        if !embedded {
            field = field.margin(egui::Margin::symmetric(6, 3));
        }
    }
    let resp = match fill {
        // Details keeps its centred fixed-size placement. The grid cell instead lets the field
        // stretch to the full cell width (infinite desired width → clamps to the cell) with its
        // height already grown to the cell via the frame padding above, so the border sits flush
        // with all four cell edges instead of floating as a smaller centred pill.
        Some(size) if is_details => ui.add_sized(size, field),
        Some(_) => ui.add(field.desired_width(f32::INFINITY)),
        None => ui.add(field.desired_width(f32::INFINITY)),
    };
    // An open editor owns keyboard focus: re-request it any frame it doesn't have it.
    // A one-shot request can be swallowed by a discarded egui pass, and egui silently
    // drops focus when the cell scrolls out of the virtualized grid (the widget isn't
    // rendered, so no lost_focus is ever reported) — either would leave a visible editor
    // that ignores typing. A *deliberate* focus move (clicking elsewhere) is observed as
    // lost_focus below and closes the editor, so this never fights another widget.
    if !resp.has_focus() && !resp.lost_focus() {
        resp.request_focus();
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return EditOutcome::Cancel;
    }
    if resp.lost_focus() {
        if valid {
            return EditOutcome::Commit { advance: None };
        }
        // Enter on invalid input keeps the editor open so it can be fixed (the focus
        // re-request above grabs it back next frame); losing focus by clicking elsewhere
        // discards it.
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            return EditOutcome::Continue;
        }
        return EditOutcome::Cancel;
    }
    EditOutcome::Continue
}

/// Map a *display* row index to the raw row id it addresses: an index into `order` for
/// stored rows, or a [`NEW_ROW_BASE`] slot for the new (insert) rows past its end.
pub fn disp_to_raw(order: &[usize], new_rows: usize, disp: usize) -> Option<usize> {
    if disp < order.len() {
        Some(order[disp])
    } else if disp < order.len() + new_rows {
        Some(NEW_ROW_BASE + (disp - order.len()))
    } else {
        None
    }
}

/// The value a cell edit is typed against: NULL for new (insert) rows, which have no stored
/// value; the stored cell otherwise.
pub fn original_value(result: &dbcore::QueryResult, raw: usize, col: usize) -> Option<Value> {
    if is_new_row(raw) {
        Some(Value::Null)
    } else {
        result.rows.get(raw).and_then(|row| row.get(col)).cloned()
    }
}

/// Commit the open editor into the staged set, typing the value against the stored cell;
/// invalid input matches the click-away rule and is discarded. No-op when nothing is open.
pub fn settle_active(edits: &mut Edits, result: &dbcore::QueryResult) {
    let Some((ar, ac)) = edits.active.as_ref().map(|a| (a.row, a.col)) else {
        return;
    };
    match original_value(result, ar, ac) {
        Some(orig) => {
            if !edits.commit_active(&orig) {
                edits.cancel_active();
            }
        }
        None => edits.cancel_active(),
    }
}

/// Open a grid-origin editor on `(raw, col)`, settling any *other* open editor first (its
/// cell may have scrolled out of the virtualized grid without ever reporting lost_focus —
/// dropping it silently would lose the typed value). Seeds from the staged value if present,
/// else the original.
pub fn begin_cell_edit(edits: &mut Edits, result: &dbcore::QueryResult, raw: usize, col: usize) {
    if edits.active.as_ref().is_some_and(|a| (a.row, a.col) != (raw, col)) {
        settle_active(edits, result);
    }
    let seed = edits
        .staged(raw, col)
        .cloned()
        .or_else(|| original_value(result, raw, col));
    if let Some(seed) = seed {
        edits.begin(raw, col, &seed, EditOrigin::Grid);
    }
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
        // DECIMAL validates as a number but is carried as text to keep precision.
        assert_eq!(EditorKind::classify("decimal(10,2)"), Decimal);
        assert_eq!(EditorKind::classify("NUMERIC"), Decimal);
        assert_eq!(EditorKind::classify("varchar"), Text);
        // INTERVAL and POINT contain "INT" but must not classify as integers.
        assert_eq!(EditorKind::classify("interval"), Text);
        assert_eq!(EditorKind::classify("point"), Text);
    }

    #[test]
    fn validation_gates_numbers_and_dates() {
        assert!(EditorKind::Int.is_valid("42"));
        assert!(!EditorKind::Int.is_valid("4.2"));
        assert!(EditorKind::Float.is_valid("4.2"));
        assert!(!EditorKind::Float.is_valid("abc"));
        assert!(EditorKind::Decimal.is_valid("1234567890.123456789"));
        assert!(!EditorKind::Decimal.is_valid("abc"));
        assert!(!EditorKind::Decimal.is_valid("inf"));
        assert!(EditorKind::Date.is_valid("2024-06-09"));
        assert!(!EditorKind::Date.is_valid("09/06/2024"));
        assert!(EditorKind::DateTime.is_valid("2024-06-09 13:45:00"));
        // Empty is always valid — it means NULL.
        assert!(EditorKind::Int.is_valid(""));
    }

    /// The exact strings the backends render must round-trip through the editor unchanged:
    /// fractional seconds (Postgres TIMESTAMP), RFC 3339 (TIMESTAMPTZ), psql-style offsets.
    #[test]
    fn validation_accepts_backend_rendered_datetimes() {
        assert!(EditorKind::DateTime.is_valid("2025-11-26 11:08:39.593333333"));
        assert!(EditorKind::DateTime.is_valid("2025-11-26T11:08:39.593333333+00:00"));
        assert!(EditorKind::DateTime.is_valid("2025-11-26T11:08:39Z"));
        assert!(EditorKind::DateTime.is_valid("2025-12-03 16:24:55.166666666"));
        assert!(EditorKind::DateTime.is_valid("2025-11-26 11:08:39.59+07"));
        assert!(!EditorKind::DateTime.is_valid("2025-13-26 11:08:39"));
        assert!(EditorKind::Time.is_valid("11:08:39.593333333"));
        assert!(EditorKind::Time.is_valid("11:08:39+07"));
        assert!(EditorKind::Time.is_valid("11:08:39.5-05:00"));
        assert!(!EditorKind::Time.is_valid("25:00:00"));
    }

    #[test]
    fn invalid_input_cannot_be_staged() {
        let mut e = Edits::default();
        e.set_columns(&[dbcore::ColumnMeta {
            name: "age".into(),
            type_name: "INT".into(),
        }]);
        e.begin(0, 0, &Value::Int(30), EditOrigin::Grid);
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

    #[test]
    fn delete_mark_toggles_and_clears_edits() {
        let mut e = Edits::default();
        // A staged edit makes the row "Edited".
        e.stage(2, 0, Value::Int(9), &Value::Int(8));
        assert_eq!(e.row_state(2), RowState::Edited);
        // Marking it for deletion wins and drops the edit.
        e.toggle_delete(2);
        assert_eq!(e.row_state(2), RowState::Deleted);
        assert_eq!(e.staged(2, 0), None);
        assert!(e.has_pending());
        // Toggling again un-marks it.
        e.toggle_delete(2);
        assert_eq!(e.row_state(2), RowState::Clean);
        assert!(!e.has_pending());
    }

    #[test]
    fn new_rows_address_above_base_and_renumber_on_remove() {
        let mut e = Edits::default();
        let a = e.add_new_row();
        let b = e.add_new_row();
        assert_eq!(a, NEW_ROW_BASE);
        assert_eq!(b, NEW_ROW_BASE + 1);
        assert!(is_new_row(a) && is_new_row(b));
        assert_eq!(e.row_state(a), RowState::New);
        assert!(e.has_pending());

        // Fill the second new row, then drop the first: the second slides down to `a`.
        e.stage(b, 0, Value::Text("keep".into()), &Value::Null);
        e.remove_new_row(a);
        assert_eq!(e.new_rows, 1);
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("keep".into())));

        // Removing the last new row clears all pending state.
        e.remove_new_row(NEW_ROW_BASE);
        assert_eq!(e.new_rows, 0);
        assert!(!e.has_pending());
    }

    #[test]
    fn undo_redo_cell_edit() {
        let mut e = Edits::default();
        assert!(!e.can_undo() && !e.can_redo());
        assert!(!e.undo(), "nothing to undo");

        e.stage(0, 0, Value::Int(5), &Value::Int(1));
        assert_eq!(e.staged(0, 0), Some(&Value::Int(5)));
        assert!(e.can_undo());

        assert!(e.undo());
        assert_eq!(e.staged(0, 0), None, "undo reverts to the stored value");
        assert!(!e.has_pending());
        assert!(e.can_redo());

        assert!(e.redo());
        assert_eq!(e.staged(0, 0), Some(&Value::Int(5)), "redo re-applies the edit");
        assert!(!e.can_redo());
    }

    #[test]
    fn undo_restores_previous_staged_value_not_just_original() {
        let mut e = Edits::default();
        // Two edits to the same cell: 1 → 5 → 9. Each undo peels back one step.
        e.stage(0, 0, Value::Int(5), &Value::Int(1));
        e.stage(0, 0, Value::Int(9), &Value::Int(1));
        assert!(e.undo());
        assert_eq!(e.staged(0, 0), Some(&Value::Int(5)), "back to the first edit");
        assert!(e.undo());
        assert_eq!(e.staged(0, 0), None, "back to the stored value");
    }

    #[test]
    fn a_fresh_edit_clears_the_redo_stack() {
        let mut e = Edits::default();
        e.stage(0, 0, Value::Int(5), &Value::Int(1));
        assert!(e.undo());
        assert!(e.can_redo());
        // A new edit invalidates the redo branch (standard editor behaviour).
        e.stage(1, 0, Value::Int(7), &Value::Int(0));
        assert!(!e.can_redo());
    }

    #[test]
    fn undo_group_folds_many_ops_into_one_step() {
        let mut e = Edits::default();
        e.begin_undo_group();
        e.stage(0, 0, Value::Int(1), &Value::Null);
        e.stage(1, 0, Value::Int(2), &Value::Null);
        e.stage(2, 0, Value::Int(3), &Value::Null);
        e.end_undo_group();

        // A single undo reverts all three edits made inside the group.
        assert!(e.undo());
        assert_eq!(e.staged(0, 0), None);
        assert_eq!(e.staged(1, 0), None);
        assert_eq!(e.staged(2, 0), None);
        assert!(!e.can_undo());
        // And a single redo restores them all.
        assert!(e.redo());
        assert_eq!(e.staged(0, 0), Some(&Value::Int(1)));
        assert_eq!(e.staged(2, 0), Some(&Value::Int(3)));
    }

    #[test]
    fn undo_delete_restores_cleared_cell_edits() {
        let mut e = Edits::default();
        e.stage(2, 0, Value::Int(9), &Value::Int(8)); // step 1: edit
        e.toggle_delete(2); // step 2: mark deleted, dropping the edit
        assert_eq!(e.row_state(2), RowState::Deleted);
        assert_eq!(e.staged(2, 0), None);

        assert!(e.undo()); // undo the delete → the edit comes back
        assert_eq!(e.row_state(2), RowState::Edited);
        assert_eq!(e.staged(2, 0), Some(&Value::Int(9)));

        assert!(e.undo()); // undo the edit
        assert_eq!(e.staged(2, 0), None);
        assert!(!e.has_pending());

        // Redo replays delete-clears-edit exactly.
        assert!(e.redo());
        assert!(e.redo());
        assert_eq!(e.row_state(2), RowState::Deleted);
        assert_eq!(e.staged(2, 0), None);
    }

    #[test]
    fn undo_redo_add_new_row_and_its_edit() {
        let mut e = Edits::default();
        let a = e.add_new_row();
        e.stage(a, 0, Value::Text("x".into()), &Value::Null);

        assert!(e.undo()); // undo the cell edit
        assert_eq!(e.staged(a, 0), None);
        assert_eq!(e.new_rows, 1, "row still there");
        assert!(e.undo()); // undo the row add
        assert_eq!(e.new_rows, 0);
        assert!(!e.has_pending());

        assert!(e.redo()); // re-add the row
        assert_eq!(e.new_rows, 1);
        assert!(e.redo()); // re-apply the cell edit
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("x".into())));
    }

    #[test]
    fn undo_remove_new_row_reinserts_it_in_place_with_cells() {
        let mut e = Edits::default();
        let r0 = e.add_new_row();
        let r1 = e.add_new_row();
        let r2 = e.add_new_row();
        e.stage(r0, 0, Value::Text("a".into()), &Value::Null);
        e.stage(r1, 0, Value::Text("b".into()), &Value::Null);
        e.stage(r2, 0, Value::Text("c".into()), &Value::Null);

        // Remove the middle row; the one above slides down into its slot.
        e.remove_new_row(r1);
        assert_eq!(e.new_rows, 2);
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("a".into())));
        assert_eq!(e.staged(NEW_ROW_BASE + 1, 0), Some(&Value::Text("c".into())));

        // Undo brings "b" back in the middle, sliding "c" back up.
        assert!(e.undo());
        assert_eq!(e.new_rows, 3);
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("a".into())));
        assert_eq!(e.staged(NEW_ROW_BASE + 1, 0), Some(&Value::Text("b".into())));
        assert_eq!(e.staged(NEW_ROW_BASE + 2, 0), Some(&Value::Text("c".into())));
    }

    #[test]
    fn discard_all_is_one_undoable_step() {
        let mut e = Edits::default();
        e.stage(0, 0, Value::Int(9), &Value::Int(8)); // stored-row edit
        e.toggle_delete(1); // deletion mark
        let n = e.add_new_row(); // new row…
        e.stage(n, 0, Value::Text("new".into()), &Value::Null); // …with a value
        assert!(e.has_pending());

        e.discard_all();
        assert!(!e.has_pending(), "discard clears every pending change");

        // A single undo restores the whole prior edit state.
        assert!(e.undo());
        assert_eq!(e.staged(0, 0), Some(&Value::Int(9)));
        assert_eq!(e.row_state(1), RowState::Deleted);
        assert_eq!(e.new_rows, 1);
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("new".into())));
        // The discard was one step on top of the four individual edits, which remain
        // undoable beneath it.
        assert!(e.can_undo());
    }

    #[test]
    fn clear_wipes_history() {
        let mut e = Edits::default();
        e.stage(0, 0, Value::Int(5), &Value::Int(1));
        // A save/reload clears staged edits *and* the history — there's nothing left to undo.
        e.clear();
        assert!(!e.can_undo());
        assert!(!e.can_redo());
        assert!(!e.undo());
    }
}
