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

use crate::style::palette;
use dbcore::{ColumnMeta, Value};

/// Column type classification and text→[`Value`] coercion now live in the data layer, so file
/// import shares exactly the rules the grid editor uses. Re-exported here because the editor
/// reaches for `edit::EditorKind` throughout.
pub use dbcore::EditorKind;

/// Read the boolean sense of a cell value, for toggling and checkbox display.
pub(crate) fn as_bool(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Text(s) => matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "t" | "yes"),
        _ => false,
    }
}

pub use dbcore::edits::{is_new_row, EditSource, NEW_ROW_BASE};

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
    /// The buffer as it was seeded. Committing an untouched editor is a no-op, so opening a
    /// cell and leaving it can never rewrite its value (e.g. `''` into `NULL`).
    seed: String,
    /// The seed value was NULL — shown as a "NULL" hint in text editors, since `''` and
    /// NULL otherwise both seed an empty buffer.
    seed_null: bool,
    /// First frame of this editor: [`render_editor`] parks the caret at the end of the
    /// buffer (type-to-edit replaces the value, and a stale caret from an earlier edit of
    /// the same cell must not land mid-text).
    fresh: bool,
    /// Other rows this edit also applies to (raw row, value to type against) — the rest
    /// of a multi-row selection the editor was opened in. See [`selection_fan_out`].
    fan_out: Vec<(usize, Value)>,
    /// Text is edited in a multi-line popover over the cell instead of the one-line field:
    /// the value has line breaks, is too long for the cell, or Shift+Enter added a break.
    expanded: bool,
    /// The column holds strings (see [`is_string_type`]): an emptied buffer means `''`.
    string_col: bool,
    /// A JSON/JSONB column: shown indented and coloured, and must stay valid JSON.
    json: bool,
    /// The column's constraints, checked on every keystroke (see [`Self::check`]).
    rule: ColumnRule,
}

impl ActiveEdit {
    /// Type the buffer for its column: string columns keep it byte-for-byte (empty stays
    /// `''`, NULL is set explicitly from the context menu); other kinds read empty as NULL.
    fn parse(&self) -> Value {
        if !self.rule.enum_values.is_empty() && self.buf.is_empty() {
            Value::Null
        } else if self.kind == EditorKind::Text && self.string_col {
            Value::Text(self.buf.clone())
        } else {
            self.kind.parse(&self.buf)
        }
    }

    /// The typed value, or why it can't be written: not valid for the type, NULL in a NOT
    /// NULL column, or longer than the column allows.
    pub fn check(&self) -> Result<Value, String> {
        if !self.kind.is_valid(&self.buf) {
            return Err(format!("Not {}", self.kind.expected()));
        }
        if self.json && !self.buf.trim().is_empty() {
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&self.buf) {
                return Err(format!("Not valid JSON: {e}"));
            }
        }
        let value = self.parse();
        match self.rule.violation(&value, is_new_row(self.row)) {
            Some(problem) => Err(problem),
            None => Ok(value),
        }
    }

    /// The column is an `ENUM`: edited by picking a label, never by typing.
    pub fn is_enum(&self) -> bool {
        !self.rule.enum_values.is_empty()
    }

    /// How many *other* rows a commit also writes (0 for a plain single-cell edit).
    pub fn fan_out_len(&self) -> usize {
        self.fan_out.len()
    }

    #[cfg(test)]
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }
}

/// Where the cell cursor should move after a commit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorDir {
    Left,
    Right,
    Up,
    Down,
}

/// What [`render_editor`] decided this frame.
pub enum EditOutcome {
    /// Keep editing.
    Continue,
    /// Finish and stage the value; `advance` asks the caller to move the cell cursor and
    /// continue editing there, `None` commits in place.
    Commit { advance: Option<CursorDir> },
    /// Abandon the edit.
    Cancel,
}

/// What the editor enforces for one column, from the table's introspected schema. The
/// default (no metadata yet) enforces nothing beyond the column's type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnRule {
    /// `NOT NULL`: a stored row can't be set to NULL.
    pub not_null: bool,
    /// An `INSERT` must supply it: NOT NULL with no default and not database-generated.
    pub required: bool,
    /// Longest string the column accepts, in characters (only where the database enforces
    /// declared lengths).
    pub max_chars: Option<u32>,
    /// The only values an `ENUM` column accepts, in declaration order. Non-empty switches
    /// the grid editor from a text field to a picker.
    pub enum_values: Vec<String>,
}

impl ColumnRule {
    /// Why `value` can't be written to this column, if it can't. On a new row NULL just
    /// leaves the column out of the `INSERT` — a missing required value is caught at save.
    pub fn violation(&self, value: &Value, new_row: bool) -> Option<String> {
        if value.is_null() && self.not_null && !new_row {
            return Some("Can't be NULL".into());
        }
        if let Value::Text(text) = value {
            if !self.enum_values.is_empty() && !self.enum_values.contains(text) {
                return Some("Not one of the allowed values".into());
            }
        }
        if let (Some(max), Value::Text(text)) = (self.max_chars, value) {
            let n = text.chars().count();
            if n > max as usize {
                return Some(format!("Too long: {n} / {max} characters"));
            }
        }
        None
    }
}

/// An explicit value set from the cell context menu (no text editor involved).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetTo {
    Null,
    /// The empty string — only offered on string-typed columns.
    Empty,
}

/// Whether a column type stores character strings, where `''` is a real value distinct from
/// NULL. Other types that edit as free text (UUID, JSON, INTERVAL, enums…) reject `''`, so
/// an emptied editor on them still means NULL.
pub fn is_string_type(type_name: &str) -> bool {
    let t = type_name.to_ascii_uppercase();
    ["CHAR", "TEXT", "CLOB", "STRING", "ASCII"]
        .iter()
        .any(|k| t.contains(k))
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
    /// Per-column [`is_string_type`], indexed like `result.columns`.
    col_strings: Vec<bool>,
    /// Per-column "is a JSON/JSONB type", indexed like `result.columns`.
    col_json: Vec<bool>,
    /// Per-column constraints, indexed like `result.columns`. Empty until the table's
    /// metadata is known (see [`Self::set_rules`]).
    rules: Vec<ColumnRule>,
    /// `rules` were built for the current result; cleared by [`Self::set_columns`].
    pub rules_synced: bool,
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

    /// How many changes are staged: edited cells on stored rows, rows marked for deletion and
    /// new rows. What the status bar counts and the commit preview lists.
    pub fn pending_count(&self) -> usize {
        let edited: usize = self
            .cells
            .iter()
            .filter(|(row, _)| !is_new_row(**row) && !self.deleted.contains(*row))
            .map(|(_, cells)| cells.len())
            .sum();
        edited + self.deleted.len() + self.new_rows
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
        self.col_strings = columns
            .iter()
            .map(|c| is_string_type(&c.type_name))
            .collect();
        self.col_json = columns
            .iter()
            .map(|c| c.type_name.to_ascii_uppercase().contains("JSON"))
            .collect();
        self.rules.clear();
        self.rules_synced = false;
    }

    /// Install the per-column constraints (indexed like `result.columns`), refreshing the
    /// open editor's copy. A no-op when unchanged, so it's cheap to call every frame.
    pub fn set_rules(&mut self, rules: Vec<ColumnRule>) {
        if rules == self.rules {
            return;
        }
        self.rules = rules;
        if let Some(active) = self.active.as_mut() {
            active.rule = self.rules.get(active.col).cloned().unwrap_or_default();
        }
    }

    pub fn rule(&self, col: usize) -> Option<&ColumnRule> {
        self.rules.get(col)
    }

    /// The first new row missing a required value, as `(new-row slot, column)`: the save
    /// would fail on it, so the app reports it instead of sending the INSERT.
    pub fn missing_required(&self) -> Option<(usize, usize)> {
        (0..self.new_rows).find_map(|slot| {
            let row = NEW_ROW_BASE + slot;
            self.rules.iter().enumerate().find_map(|(col, rule)| {
                let filled = self.staged(row, col).is_some_and(|v| !v.is_null());
                (rule.required && !filled).then_some((slot, col))
            })
        })
    }

    pub fn col_kind(&self, col: usize) -> EditorKind {
        self.col_kinds.get(col).copied().unwrap_or_default()
    }

    /// Whether `col` holds strings, so `''` is a value of its own (see [`is_string_type`]).
    pub fn col_is_string(&self, col: usize) -> bool {
        self.col_strings.get(col).copied().unwrap_or(false)
    }

    /// Stage one explicit value into `col` across several rows as a single undo step.
    /// `targets` pairs each raw row with the value it is typed against (see
    /// [`original_value`]); rows marked for deletion are skipped. An open editor on an
    /// affected cell is closed so it can't commit over the new value. Rows the column's
    /// constraints reject (NULL into NOT NULL) are skipped; returns how many.
    pub fn set_cells(&mut self, targets: &[(usize, Value)], col: usize, to: SetTo) -> usize {
        let value = match to {
            SetTo::Null => Value::Null,
            SetTo::Empty => Value::Text(String::new()),
        };
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.col == col && targets.iter().any(|(r, _)| *r == a.row))
        {
            self.active = None;
        }
        let rule = self.rules.get(col).cloned().unwrap_or_default();
        let mut rejected = 0;
        self.begin_undo_group();
        for (row, original) in targets {
            if self.deleted.contains(row) {
                continue;
            }
            if rule.violation(&value, is_new_row(*row)).is_some() {
                rejected += 1;
                continue;
            }
            self.stage(*row, col, value.clone(), original);
        }
        self.end_undo_group();
        rejected
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

    /// Stage pasted `text` over `(row, col)`, typed by the column kind (an empty field is
    /// NULL, matching how Copy writes NULL to TSV). Returns `false` — staging nothing — when
    /// the text isn't valid for the column's type or constraints.
    pub fn paste_text(&mut self, row: usize, col: usize, text: &str, original: &Value) -> bool {
        let kind = self.col_kind(col);
        if !kind.is_valid(text) {
            return false;
        }
        let value = kind.parse(text);
        let violates = self
            .rules
            .get(col)
            .is_some_and(|rule| rule.violation(&value, is_new_row(row)).is_some());
        if violates {
            return false;
        }
        self.stage(row, col, value, original);
        true
    }

    /// Make the open editor also apply to `targets` on commit (see [`selection_fan_out`]).
    pub fn set_fan_out(&mut self, targets: Vec<(usize, Value)>) {
        if let Some(active) = self.active.as_mut() {
            active.fan_out = targets;
        }
    }

    /// Flip a boolean cell and stage the result immediately (no text editor needed).
    pub fn toggle_bool(&mut self, row: usize, col: usize, original: &Value) {
        let current = self
            .staged(row, col)
            .map(as_bool)
            .unwrap_or(as_bool(original));
        self.stage(row, col, Value::Bool(!current), original);
    }

    /// Open an editor on `(row, col)`, seeding the buffer from the cell's current value.
    /// `origin` is the view that should render the editor (grid or Details panel).
    pub fn begin(&mut self, row: usize, col: usize, current: &Value, origin: EditOrigin) {
        let json = self.col_json.get(col).copied().unwrap_or(false);
        let mut buf = match current {
            Value::Null => String::new(),
            other => other.display(),
        };
        // JSON opens indented so it can be read and edited; an untouched editor still stages
        // nothing (the seed is the indented text).
        if json {
            if let Some(pretty) = crate::value_viewer::pretty_json(&buf) {
                buf = pretty;
            }
        }
        let mut rule = self.rules.get(col).cloned().unwrap_or_default();
        let kind = self.col_kind(col);
        // A boolean is picked from a list like an enum, never typed.
        if kind == EditorKind::Bool && rule.enum_values.is_empty() {
            rule.enum_values = vec!["true".into(), "false".into()];
        }
        self.active = Some(ActiveEdit {
            row,
            col,
            kind,
            seed: buf.clone(),
            buf,
            origin,
            seed_null: current.is_null(),
            fresh: true,
            fan_out: Vec::new(),
            expanded: false,
            string_col: self.col_is_string(col),
            json,
            rule,
        });
    }

    /// Whether `(row, col)` is being edited *and* `origin` is the view that opened the
    /// editor — i.e. the view that should render it.
    pub fn is_active_from(&self, row: usize, col: usize, origin: EditOrigin) -> bool {
        self.is_active(row, col) && self.active.as_ref().is_some_and(|a| a.origin == origin)
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
        // Untouched: close without staging. Re-parsing the seed isn't lossless (`''` and
        // NULL seed the same empty buffer), so it must not be written back.
        if active.buf == active.seed {
            self.active = None;
            return true;
        }
        let Ok(new) = active.check() else {
            return false;
        };
        let active = self.active.take().expect("active checked above");
        // One undo step covers the edited cell and every fanned-out row (skipping any the
        // column's constraints reject — e.g. NULL is fine on a new row, not a stored one).
        self.begin_undo_group();
        for (row, orig) in &active.fan_out {
            if !self.deleted.contains(row)
                && !matches!(orig, Value::Bytes(_))
                && active.rule.violation(&new, is_new_row(*row)).is_none()
            {
                self.stage(*row, active.col, new.clone(), orig);
            }
        }
        self.stage(active.row, active.col, new, original);
        self.end_undo_group();
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

    /// Test-only visibility into the two history stacks.
    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    #[cfg(test)]
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

/// Minimum width of the expanded (multi-line) editor popover.
const EXPANDED_EDITOR_W: f32 = 360.0;
/// Height past which the expanded editor scrolls instead of growing.
const EXPANDED_EDITOR_MAX_H: f32 = 280.0;

/// Replace the editor's selection (or insert at its caret) with `text`, leaving the caret
/// after the insertion. With no stored caret, inserts at the end.
fn insert_at_caret(ctx: &egui::Context, id: egui::Id, buf: &mut String, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let n = buf.chars().count();
    let range = state
        .cursor
        .char_range()
        .map_or(n..n, |r| r.as_sorted_char_range());
    let (start, end) = (range.start.min(n), range.end.min(n));
    let byte = |c: usize| buf.char_indices().nth(c).map_or(buf.len(), |(b, _)| b);
    let (bs, be) = (byte(start), byte(end));
    buf.replace_range(bs..be, text);
    let caret = egui::text::CCursor::new(start + text.chars().count());
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(caret)));
    state.store(ctx, id);
}

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
    let editor_id = egui::Id::new(("cell_editor", active.row, active.col, active.origin));
    if active.is_enum() {
        return render_enum_picker(ui, active, editor_id, fill);
    }
    let was_expanded = active.expanded;
    // Text that can't be edited comfortably on one line opens expanded: it already has line
    // breaks, or (checked once, as the editor opens) it's wider than the cell.
    if active.kind == EditorKind::Text && !active.expanded {
        active.expanded = active.buf.contains('\n')
            || (active.fresh
                && fill.is_some_and(|size| {
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let width = ui
                        .painter()
                        .layout_no_wrap(active.buf.clone(), font, egui::Color32::PLACEHOLDER)
                        .size()
                        .x;
                    width + 12.0 > size.x
                }));
    }
    // Tab / Shift+Tab: commit and advance to the neighbouring cell, spreadsheet-style.
    // In a grid editor, Up/Down do the same vertically while keeping the current column;
    // Left/Right remain available for moving the caret within the single-line value.
    // Consumed *before* the TextEdit is built so egui's focus traversal never sees it.
    // Invalid input swallows the Tab and stays put — same rule as Enter-on-invalid below.
    let advance = ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
            Some(CursorDir::Left)
        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            Some(CursorDir::Right)
        } else if active.origin == EditOrigin::Grid
            && !active.expanded
            && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
        {
            Some(CursorDir::Up)
        } else if active.origin == EditOrigin::Grid
            && !active.expanded
            && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
        {
            Some(CursorDir::Down)
        } else {
            None
        }
    });
    if advance.is_some() && active.check().is_ok() {
        return EditOutcome::Commit { advance };
    }
    // Shift+Enter inserts a line break into text and switches to the expanded editor
    // (Enter alone still commits, in both modes).
    if active.kind == EditorKind::Text
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter))
    {
        insert_at_caret(ui.ctx(), editor_id, &mut active.buf, "\n");
        active.expanded = true;
    }
    let hint = if active.kind == EditorKind::Text && active.seed_null {
        "NULL"
    } else {
        active.kind.hint()
    };
    let problem = active.check().err();
    let valid = problem.is_none();
    let resp = if active.expanded {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            return EditOutcome::Cancel;
        }
        // Enter commits valid input; on invalid input it stays put, as in the one-line field.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) && valid {
            return EditOutcome::Commit { advance: None };
        }
        render_expanded(ui, active, editor_id, hint, problem.as_deref())
    } else {
        if let Some(problem) = problem.as_deref() {
            render_problem(ui, editor_id, problem);
        }
        render_single_line(ui, active, editor_id, hint, valid, fill)
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
    if std::mem::take(&mut active.fresh) {
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), resp.id) {
            let end = egui::text::CCursor::new(active.buf.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), resp.id);
        }
    }
    // A new popover's first frame is an invisible sizing pass, where the field can't hold
    // focus — the drop it reports is not the user leaving. Take focus back and carry on.
    if active.expanded && !was_expanded {
        resp.request_focus();
        return EditOutcome::Continue;
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

/// The editor for an `ENUM` column: the cell shows the current label and a list of the
/// allowed ones opens under it, so nothing outside the enum can be typed. Up/Down move the
/// highlight, Enter or a click picks, Esc (or a click elsewhere) abandons. NULL is offered
/// wherever the column allows it.
fn render_enum_picker(
    ui: &mut egui::Ui,
    active: &mut ActiveEdit,
    editor_id: egui::Id,
    fill: Option<egui::Vec2>,
) -> EditOutcome {
    let cell = ui.max_rect();
    let width = fill.map_or(cell.width(), |size| size.x);
    let null_ok = active
        .rule
        .violation(&Value::Null, is_new_row(active.row))
        .is_none();
    // Entries are `None` for NULL, else an index into the rule's labels; NULL comes last.
    let mut entries: Vec<Option<usize>> = (0..active.rule.enum_values.len()).map(Some).collect();
    if null_ok {
        entries.push(None);
    }
    let current = if active.buf.is_empty() {
        None
    } else if active.kind == EditorKind::Bool {
        // Backends render booleans differently (true, t, 1…): match by meaning.
        Some(usize::from(!as_bool(&active.kind.parse(&active.buf))))
    } else {
        active
            .rule
            .enum_values
            .iter()
            .position(|v| *v == active.buf)
    };

    let hl_id = editor_id.with("highlight");
    let mut highlight = ui
        .data(|d| d.get_temp::<usize>(hl_id))
        .unwrap_or_else(|| {
            entries
                .iter()
                .position(|e| *e == current)
                .unwrap_or_default()
        })
        .min(entries.len().saturating_sub(1));
    let mut picked: Option<Option<usize>> = None;
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        highlight = (highlight + 1).min(entries.len().saturating_sub(1));
    }
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        highlight = highlight.saturating_sub(1);
    }
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        picked = entries.get(highlight).copied();
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return EditOutcome::Cancel;
    }
    ui.data_mut(|d| d.insert_temp(hl_id, highlight));

    // The cell face: current label (or NULL) with a chevron, on the editor's fill.
    let face = fill.map_or(cell, |size| egui::Rect::from_min_size(cell.min, size));
    ui.painter().rect_filled(face, 0.0, palette::CODE_BG());
    let (text, colour) = match current {
        Some(i) => (active.rule.enum_values[i].as_str(), palette::TEXT()),
        None => ("NULL", palette::TEXT_WEAK()),
    };
    ui.painter().text(
        face.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Body.resolve(ui.style()),
        colour,
    );
    let chevron = egui::Rect::from_center_size(
        face.right_center() - egui::vec2(12.0, 0.0),
        egui::Vec2::splat(14.0),
    );
    egui::Image::new(crate::icons::chevron_down())
        .fit_to_exact_size(chevron.size())
        .tint(palette::TEXT_WEAK())
        .paint_at(ui, chevron);

    let area = egui::Area::new(editor_id.with("list"))
        .order(egui::Order::Foreground)
        .fixed_pos(face.left_bottom() + egui::vec2(0.0, 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(width.max(140.0));
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (pos, entry) in entries.iter().enumerate() {
                            let label = match entry {
                                Some(i) => active.rule.enum_values[*i].as_str(),
                                None => "NULL",
                            };
                            if entry.is_none() && pos > 0 {
                                ui.separator();
                            }
                            let resp = ui.add_sized(
                                [ui.available_width(), 22.0],
                                egui::Button::selectable(*entry == current, label),
                            );
                            if resp.hovered() {
                                highlight = pos;
                            }
                            if pos == highlight {
                                resp.scroll_to_me(None);
                                // Keyboard/hover highlight is a soft fill, not an accent outline.
                                if *entry != current {
                                    ui.painter().rect_filled(
                                        resp.rect,
                                        4.0,
                                        palette::TEXT().gamma_multiply(0.08),
                                    );
                                }
                            }
                            if resp.clicked() {
                                picked = Some(*entry);
                            }
                        }
                    });
            });
        });
    ui.data_mut(|d| d.insert_temp(hl_id, highlight));

    if let Some(entry) = picked {
        active.buf = entry.map_or_else(String::new, |i| active.rule.enum_values[i].clone());
        ui.data_mut(|d| d.remove_temp::<usize>(hl_id));
        return EditOutcome::Commit { advance: None };
    }
    // A press anywhere outside both the cell and the list abandons the pick.
    let pressed = ui.input(|i| i.pointer.any_pressed());
    if pressed {
        let pos = ui.input(|i| i.pointer.interact_pos());
        if pos.is_some_and(|p| !face.contains(p) && !area.response.rect.contains(p)) {
            return EditOutcome::Cancel;
        }
    }
    EditOutcome::Continue
}

/// The multi-line text editor, as a popover anchored at the cell's top-left (a grid cell is
/// one row tall, so it can't host the text itself). At least [`EXPANDED_EDITOR_W`] wide,
/// kept on screen, and scrolling past [`EXPANDED_EDITOR_MAX_H`].
fn render_expanded(
    ui: &mut egui::Ui,
    active: &mut ActiveEdit,
    editor_id: egui::Id,
    hint: &str,
    problem: Option<&str>,
) -> egui::Response {
    let valid = problem.is_none();
    let anchor = ui.max_rect();
    let screen = ui.ctx().content_rect();
    let width = anchor
        .width()
        .max(EXPANDED_EDITOR_W)
        .min(screen.width() - 16.0);
    let x = anchor
        .left()
        .min(screen.right() - width - 8.0)
        .max(screen.left() + 8.0);
    let border = if valid {
        palette::ACCENT()
    } else {
        palette::DANGER()
    };
    egui::Area::new(editor_id.with("expanded"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(x, anchor.top()))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(palette::CODE_BG())
                .stroke(egui::Stroke::new(1.0_f32, border))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    let resp = egui::ScrollArea::vertical()
                        .max_height(EXPANDED_EDITOR_MAX_H)
                        .show(ui, |ui| {
                            let mut layouter =
                                |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                                    let mut job = crate::value_viewer::json_layout(text.as_str());
                                    job.wrap.max_width = wrap;
                                    ui.fonts_mut(|f| f.layout_job(job))
                                };
                            let json = active.json;
                            let mut edit = egui::TextEdit::multiline(&mut active.buf);
                            if json {
                                edit = edit.layouter(&mut layouter).code_editor();
                            }
                            ui.add(
                                edit.id(editor_id)
                                    .hint_text(hint)
                                    .frame(egui::Frame::NONE)
                                    // Enter commits and Shift+Enter breaks the line — both
                                    // handled by the caller — so the field inserts neither.
                                    .return_key(None)
                                    .lock_focus(true)
                                    .desired_rows(3)
                                    .desired_width(width - 12.0),
                            )
                        })
                        .inner;
                    let note = match problem {
                        Some(problem) => egui::RichText::new(problem).color(palette::DANGER()),
                        None => {
                            egui::RichText::new("Enter save · Shift+Enter new line · Esc cancel")
                                .color(palette::TEXT_FAINT())
                        }
                    };
                    ui.label(note.size(11.0));
                    resp
                })
                .inner
        })
        .inner
}

/// Why the one-line editor's input can't be saved, in a small note under the cell. Not
/// interactable, so it never takes a click (or focus) from the field or the grid.
fn render_problem(ui: &egui::Ui, editor_id: egui::Id, problem: &str) {
    let anchor = ui.max_rect();
    egui::Area::new(editor_id.with("problem"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(palette::SURFACE())
                .stroke(egui::Stroke::new(1.0_f32, palette::DANGER()))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(problem)
                            .size(11.5)
                            .color(palette::DANGER()),
                    );
                });
        });
}

/// The one-line editor that fills the grid cell or Details value box.
fn render_single_line(
    ui: &mut egui::Ui,
    active: &mut ActiveEdit,
    editor_id: egui::Id,
    hint: &str,
    valid: bool,
    fill: Option<egui::Vec2>,
) -> egui::Response {
    let embedded = fill.is_some();
    let is_details = active.origin == EditOrigin::Details;
    let mut field = egui::TextEdit::singleline(&mut active.buf)
        .hint_text(hint)
        .id(editor_id)
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
            // The editor covers the cell expanded by half the item spacing; inset its text by
            // that plus the cell's own inset so the value stays exactly where it was drawn.
            let inset = (crate::grid::CELL_INSET_X + 0.5 * ui.spacing().item_spacing.x).round();
            inner.left = inset as i8;
            inner.right = inset as i8;
            // Numbers end right in the grid so their digits line up; keep them there while
            // editing, in the same face and size, instead of jumping to the left edge.
            let numeric = active.kind.monospace_value();
            if numeric {
                field = field.horizontal_align(egui::Align::RIGHT);
            }
            if numeric || crate::fonts::grid_all_monospace(ui.ctx()) {
                field = field.font(egui::FontId::new(
                    crate::grid::GRID_MONO_SIZE,
                    egui::FontFamily::Monospace,
                ));
            }
        }
        field = field.frame(
            egui::Frame::new()
                .fill(palette::CODE_BG())
                .stroke(egui::Stroke::new(1.0_f32, border))
                .corner_radius(cr)
                .inner_margin(inner),
        );
        if !embedded {
            field = field.margin(egui::Margin::symmetric(6, 3));
        }
    }
    match fill {
        // Details keeps its centred fixed-size placement. The grid cell instead lets the field
        // stretch to the full cell width (infinite desired width → clamps to the cell) with its
        // height already grown to the cell via the frame padding above, so the border sits flush
        // with all four cell edges instead of floating as a smaller centred pill.
        Some(size) if is_details => ui.add_sized(size, field),
        Some(_) => ui.add(field.desired_width(f32::INFINITY)),
        None => ui.add(field.desired_width(f32::INFINITY)),
    }
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

/// The rows an edit at display row `disp` should also write, TablePlus-style: when `disp`
/// is part of a multi-row selection, every *other* selected row paired with the value its
/// `col` cell is typed against. Empty for a single-row selection or a cell outside it.
pub fn selection_fan_out(
    selection: &crate::grid::Selection,
    order: &[usize],
    new_rows: usize,
    result: &dbcore::QueryResult,
    disp: usize,
    col: usize,
) -> Vec<(usize, Value)> {
    if selection.len() < 2 || !selection.contains(disp) {
        return Vec::new();
    }
    selection
        .iter()
        .filter(|&d| d != disp)
        .filter_map(|d| disp_to_raw(order, new_rows, d))
        .filter_map(|raw| original_value(result, raw, col).map(|v| (raw, v)))
        .collect()
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
    if edits
        .active
        .as_ref()
        .is_some_and(|a| (a.row, a.col) != (raw, col))
    {
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

    fn cols(types: &[&str]) -> Vec<dbcore::ColumnMeta> {
        types
            .iter()
            .enumerate()
            .map(|(i, t)| dbcore::ColumnMeta {
                name: format!("c{i}"),
                type_name: (*t).into(),
            })
            .collect()
    }

    /// Opening a cell and leaving it untouched must never rewrite it: `''` and NULL both seed
    /// an empty buffer, so re-parsing would silently turn one into the other.
    #[test]
    fn untouched_editor_stages_nothing() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["varchar(20)", "INT"]));
        for original in [Value::Text(String::new()), Value::Null] {
            e.begin(0, 0, &original, EditOrigin::Grid);
            assert!(e.commit_active(&original));
            assert!(e.active.is_none());
            assert!(!e.has_pending(), "{original:?} must stay as-is");
        }
    }

    /// String columns keep what was typed — clearing gives `''`, spaces survive. Other kinds
    /// still read an emptied editor as NULL.
    #[test]
    fn string_columns_distinguish_empty_from_null() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["TEXT", "INT", "uuid"]));
        let hello = Value::Text("hello".into());

        e.begin(0, 0, &hello, EditOrigin::Grid);
        e.active.as_mut().unwrap().buf.clear();
        assert!(e.commit_active(&hello));
        assert_eq!(e.staged(0, 0), Some(&Value::Text(String::new())));

        e.begin(1, 0, &hello, EditOrigin::Grid);
        e.active.as_mut().unwrap().buf = "  ".into();
        assert!(e.commit_active(&hello));
        assert_eq!(e.staged(1, 0), Some(&Value::Text("  ".into())));

        e.begin(0, 1, &Value::Int(3), EditOrigin::Grid);
        e.active.as_mut().unwrap().buf.clear();
        assert!(e.commit_active(&Value::Int(3)));
        assert_eq!(e.staged(0, 1), Some(&Value::Null));

        // UUID edits as free text but has no empty value: emptied still means NULL.
        let id = Value::Text("0b6e…".into());
        e.begin(0, 2, &id, EditOrigin::Grid);
        e.active.as_mut().unwrap().buf.clear();
        assert!(e.commit_active(&id));
        assert_eq!(e.staged(0, 2), Some(&Value::Null));
    }

    #[test]
    fn set_cells_is_one_undo_step_and_skips_deleted_rows() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["TEXT"]));
        e.toggle_delete(1);
        let a = Value::Text("a".into());
        e.begin(0, 0, &a, EditOrigin::Grid);
        let targets = [(0, a.clone()), (1, a.clone()), (2, Value::Null)];
        e.set_cells(&targets, 0, SetTo::Null);

        assert!(e.active.is_none(), "an editor on a target cell is closed");
        assert_eq!(e.staged(0, 0), Some(&Value::Null));
        assert_eq!(e.staged(1, 0), None, "deleted row untouched");
        assert_eq!(e.staged(2, 0), None, "already NULL → no change");

        e.set_cells(&targets, 0, SetTo::Empty);
        assert_eq!(e.staged(2, 0), Some(&Value::Text(String::new())));
        assert!(e.undo());
        assert_eq!(e.staged(0, 0), Some(&Value::Null), "one undo per action");
        assert_eq!(e.staged(2, 0), None);
    }

    /// An editor opened over a multi-row selection writes every selected row on commit, as
    /// one undo step — but only if something was actually typed.
    #[test]
    fn fan_out_commit_writes_every_target_as_one_step() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["TEXT"]));
        e.toggle_delete(3);
        let targets = vec![
            (1, Value::Text("b".into())),
            (3, Value::Text("d".into())),
            (4, Value::Bytes(vec![1])),
        ];
        let a = Value::Text("a".into());

        e.begin(0, 0, &a, EditOrigin::Grid);
        e.set_fan_out(targets.clone());
        assert!(e.commit_active(&a), "untouched");
        assert!(
            !e.row_dirty(0) && !e.row_dirty(1),
            "untouched fans out nothing"
        );

        e.begin(0, 0, &a, EditOrigin::Grid);
        e.set_fan_out(targets);
        assert_eq!(e.active.as_ref().unwrap().fan_out_len(), 3);
        e.active.as_mut().unwrap().buf = "z".into();
        assert!(e.commit_active(&a));
        let z = Some(Value::Text("z".into()));
        assert_eq!(e.staged(0, 0).cloned(), z);
        assert_eq!(e.staged(1, 0).cloned(), z);
        assert_eq!(e.staged(3, 0), None, "deleted row skipped");
        assert_eq!(e.staged(4, 0), None, "binary cell skipped");

        assert!(e.undo());
        assert!(!e.row_dirty(0) && !e.row_dirty(1), "one undo reverts all");
    }

    #[test]
    fn paste_text_rejects_invalid_values() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["INT"]));
        assert!(!e.paste_text(0, 0, "abc", &Value::Int(1)));
        assert!(!e.has_pending());
        assert!(e.paste_text(0, 0, "7", &Value::Int(1)));
        assert_eq!(e.staged(0, 0), Some(&Value::Int(7)));
        assert!(
            e.paste_text(0, 0, "", &Value::Int(1)),
            "empty field is NULL"
        );
        assert_eq!(e.staged(0, 0), Some(&Value::Null));
    }

    /// Screenshot generator (ignored): the expanded multi-line editor popover over a grid
    /// cell, so its frame, wrap and key hint can be judged at the size it ships at.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_expanded_editor() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["TEXT"]));
        let value = Value::Text(
            "Shipping note:\nLeave at the side door, ring twice.\n{\"gift\": true, \"wrap\": \"blue\"}".into(),
        );
        e.begin(0, 0, &value, EditOrigin::Grid);
        let mut active = e.active.take();
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(460.0, 220.0))
            .with_pixels_per_point(2.0)
            .build_ui(move |ui| {
                if !setup {
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                ui.painter()
                    .rect_filled(ui.ctx().content_rect(), 0.0, palette::BASE());
                let cell =
                    egui::Rect::from_min_size(egui::pos2(40.0, 30.0), egui::vec2(150.0, 26.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| {
                    if let Some(active) = active.as_mut() {
                        render_editor(ui, active, Some(cell.size()));
                    }
                });
            });
        harness.run_steps(8);
        harness.snapshot("expanded_cell_editor");
    }

    fn rule(not_null: bool, required: bool, max_chars: Option<u32>) -> ColumnRule {
        ColumnRule {
            not_null,
            required,
            max_chars,
            enum_values: Vec::new(),
        }
    }

    /// NOT NULL stops a stored row being emptied to NULL (with the reason), while a new row
    /// may leave the column out — required-ness is checked at save instead.
    #[test]
    fn not_null_blocks_null_on_stored_rows_only() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["INT"]));
        e.set_rules(vec![rule(true, false, None)]);

        e.begin(0, 0, &Value::Int(3), EditOrigin::Grid);
        e.active.as_mut().unwrap().buf.clear();
        assert_eq!(
            e.active.as_ref().unwrap().check(),
            Err("Can't be NULL".into())
        );
        assert!(!e.commit_active(&Value::Int(3)), "stays open");
        assert!(!e.has_pending());

        let new = e.add_new_row();
        e.begin(new, 0, &Value::Null, EditOrigin::Grid);
        e.active.as_mut().unwrap().buf = "5".into();
        e.active.as_mut().unwrap().buf.clear();
        assert!(e.active.as_ref().unwrap().check().is_ok());
    }

    #[test]
    fn declared_length_is_enforced_while_typing() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["varchar(5)"]));
        e.set_rules(vec![rule(false, false, Some(5))]);
        let orig = Value::Text("abc".into());
        e.begin(0, 0, &orig, EditOrigin::Grid);
        // Characters, not bytes: five Thai characters fit.
        e.active.as_mut().unwrap().buf = "สวัสด".into();
        assert!(e.active.as_ref().unwrap().check().is_ok());
        e.active.as_mut().unwrap().buf = "abcdef".into();
        assert_eq!(
            e.active.as_ref().unwrap().check(),
            Err("Too long: 6 / 5 characters".into())
        );
        assert!(!e.commit_active(&orig));
        // Paste and Set Empty/NULL obey the same rules.
        assert!(!e.paste_text(0, 0, "abcdef", &orig));
        assert!(e.paste_text(0, 0, "abcde", &orig));
    }

    #[test]
    fn set_cells_skips_rows_the_rule_rejects() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["TEXT"]));
        e.set_rules(vec![rule(true, false, None)]);
        let new = e.add_new_row();
        let a = Value::Text("a".into());
        let skipped = e.set_cells(&[(0, a.clone()), (new, Value::Null)], 0, SetTo::Empty);
        assert_eq!(skipped, 0, "'' is fine in NOT NULL");
        let skipped = e.set_cells(&[(0, a), (1, Value::Text("b".into()))], 0, SetTo::Null);
        assert_eq!(skipped, 2);
        assert_eq!(e.staged(1, 0), None);
    }

    #[test]
    fn missing_required_names_the_first_empty_cell() {
        let mut e = Edits::default();
        e.set_columns(&cols(&["INT", "TEXT", "TEXT"]));
        e.set_rules(vec![
            rule(true, false, None), // NOT NULL with a default: may be omitted
            rule(true, true, None),
            rule(false, false, None),
        ]);
        let first = e.add_new_row();
        let second = e.add_new_row();
        e.stage(first, 1, Value::Text("x".into()), &Value::Null);
        assert_eq!(e.missing_required(), Some((1, 1)), "second row, column 1");
        e.stage(second, 1, Value::Text("y".into()), &Value::Null);
        assert_eq!(e.missing_required(), None);
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
        assert_eq!(
            e.staged(0, 0),
            Some(&Value::Int(5)),
            "redo re-applies the edit"
        );
        assert!(!e.can_redo());
    }

    #[test]
    fn undo_restores_previous_staged_value_not_just_original() {
        let mut e = Edits::default();
        // Two edits to the same cell: 1 → 5 → 9. Each undo peels back one step.
        e.stage(0, 0, Value::Int(5), &Value::Int(1));
        e.stage(0, 0, Value::Int(9), &Value::Int(1));
        assert!(e.undo());
        assert_eq!(
            e.staged(0, 0),
            Some(&Value::Int(5)),
            "back to the first edit"
        );
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
        assert_eq!(
            e.staged(NEW_ROW_BASE + 1, 0),
            Some(&Value::Text("c".into()))
        );

        // Undo brings "b" back in the middle, sliding "c" back up.
        assert!(e.undo());
        assert_eq!(e.new_rows, 3);
        assert_eq!(e.staged(NEW_ROW_BASE, 0), Some(&Value::Text("a".into())));
        assert_eq!(
            e.staged(NEW_ROW_BASE + 1, 0),
            Some(&Value::Text("b".into()))
        );
        assert_eq!(
            e.staged(NEW_ROW_BASE + 2, 0),
            Some(&Value::Text("c".into()))
        );
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

#[cfg(test)]
mod enum_rule_tests {
    use super::*;

    fn enum_rule(not_null: bool) -> ColumnRule {
        ColumnRule {
            not_null,
            enum_values: vec!["active".into(), "banned".into()],
            ..ColumnRule::default()
        }
    }

    #[test]
    fn only_declared_labels_are_accepted() {
        let rule = enum_rule(false);
        assert!(rule
            .violation(&Value::Text("active".into()), false)
            .is_none());
        assert!(rule.violation(&Value::Null, false).is_none());
        assert!(rule.violation(&Value::Text("nope".into()), false).is_some());
        assert!(rule.violation(&Value::Text(String::new()), false).is_some());
    }

    #[test]
    fn not_null_enum_rejects_null_on_stored_rows() {
        assert!(enum_rule(true).violation(&Value::Null, false).is_some());
    }
}
