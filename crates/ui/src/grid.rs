//! The results grid — a virtualized, resizable, sortable table built on
//! `egui_extras::TableBuilder`, styled after TablePlus: a row-number gutter on the left,
//! dense rows, click-to-select with row highlight, and click-to-sort headers.
//! Only the visible rows are rendered each frame, so it stays smooth at 100k+ rows.

use crate::components;
use crate::edit::{EditOutcome, EditorKind, Edits};
use crate::emoji::{self, EmojiAtlas};
use crate::style::palette;
use dbcore::{QueryResult, Value};
use egui_extras::{Column, TableBuilder};

/// Natural per-column width: columns expand past this to fill spare space, but never shrink
/// below it — once the total exceeds the panel the grid scrolls horizontally instead.
const COL_W: f32 = 160.0;

/// Header row height. Used both by the `TableBuilder` header and by the empty-table
/// double-click zone, which measures down from the header — they must agree.
const HEADER_H: f32 = 26.0;

/// Height of the double-click-to-add-row strip kept under the table when it's editable.
/// The strip is *reserved* (the table is shrunk to sit above it), never overlaid: an
/// overlay would be the topmost widget in egui's hit-test and would steal single and
/// double clicks from any row rendered beneath it.
const ADD_ROW_ZONE: f32 = 24.0;

/// A sort request from a column header (click or the header menu).
#[derive(Clone, Copy)]
pub enum SortCmd {
    /// Header clicked: sort by this column, flipping direction if it's already the sort.
    Toggle(usize),
    /// Menu: sort this column ascending.
    Asc(usize),
    /// Menu: sort this column descending.
    Desc(usize),
    /// Menu: drop the sort and return to the natural row order.
    Clear,
}

/// A row click reported by the grid, carrying the modifier keys held at click time so the
/// app can resolve it into a plain select, a Cmd/Ctrl toggle, or a Shift range-extend.
#[derive(Clone, Copy)]
pub struct RowClick {
    /// The clicked *display* row index (index into `order`, or a new-row slot past its end).
    pub disp: usize,
    /// The clicked column, when the click landed on a data cell (`None` for the gutter).
    /// Moves the cell cursor there so keyboard navigation continues from the click.
    pub col: Option<usize>,
    /// Shift was held → extend the selection from the anchor to this row.
    pub shift: bool,
    /// Cmd (macOS) / Ctrl (elsewhere) was held → toggle this row in/out of the selection.
    pub cmd: bool,
}

/// A spreadsheet fill-handle drag: copy the source cell's current value through the target
/// display row range in the same column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillRequest {
    pub from_disp: usize,
    pub to_disp: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug)]
struct FillDrag {
    from_disp: usize,
    target_disp: usize,
    col: usize,
}

/// A multi-row selection over *display* indices. Tracks an `anchor` (the fixed end of a Shift
/// range) and a `lead` (the most recently affected row, which drives the Details panel), so it
/// behaves like a Finder/Excel/TablePlus list: plain click selects one, Cmd/Ctrl click toggles,
/// Shift click selects a contiguous range.
#[derive(Default, Clone)]
pub struct Selection {
    rows: std::collections::BTreeSet<usize>,
    anchor: Option<usize>,
    lead: Option<usize>,
    /// The spreadsheet-style cell cursor as `(display row, column)`. Follows clicks and
    /// arrow keys; Enter/F2 opens the editor here. Independent of `rows` membership —
    /// like Excel's active cell, it can sit on an unselected row.
    cursor: Option<(usize, usize)>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn contains(&self, disp: usize) -> bool {
        self.rows.contains(&disp)
    }

    /// The most recently affected row — what the Details panel shows.
    pub fn lead(&self) -> Option<usize> {
        self.lead
    }

    /// The cell cursor as `(display row, column)`, if placed.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    /// Place the cell cursor (a data-cell click, or the app tracking an editor).
    pub fn set_cursor(&mut self, disp: usize, col: usize) {
        self.cursor = Some((disp, col));
    }

    /// Set the lead row and drag the cursor's row along with it (keeping its column), so
    /// every selection change — click, arrows, select-all — leaves the cursor coherent.
    fn set_lead(&mut self, disp: usize) {
        self.lead = Some(disp);
        let col = self.cursor.map_or(0, |(_, c)| c);
        self.cursor = Some((disp, col));
    }

    /// The selected display indices, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().copied()
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
        self.lead = None;
        self.cursor = None;
    }

    /// Plain click: select exactly `disp` and make it the anchor.
    pub fn select_one(&mut self, disp: usize) {
        self.rows.clear();
        self.rows.insert(disp);
        self.anchor = Some(disp);
        self.set_lead(disp);
    }

    /// Cmd/Ctrl click: toggle `disp` in/out, keeping the rest, and re-anchor on it.
    pub fn toggle(&mut self, disp: usize) {
        if !self.rows.insert(disp) {
            self.rows.remove(&disp);
            if self.lead == Some(disp) {
                match self.rows.iter().next_back().copied() {
                    Some(l) => self.set_lead(l),
                    None => self.lead = None,
                }
            }
        } else {
            self.set_lead(disp);
        }
        self.anchor = Some(disp);
    }

    /// Shift click: replace the selection with the contiguous range anchor..=disp. The anchor
    /// stays put so dragging the Shift end back and forth grows/shrinks the same range.
    pub fn range_to(&mut self, disp: usize) {
        let anchor = self.anchor.unwrap_or(disp);
        self.rows.clear();
        for d in anchor.min(disp)..=anchor.max(disp) {
            self.rows.insert(d);
        }
        self.anchor = Some(anchor);
        self.set_lead(disp);
    }

    /// Select every display row in `0..len` (Cmd/Ctrl+A).
    pub fn select_all(&mut self, len: usize) {
        self.rows = (0..len).collect();
        self.anchor = Some(0);
        match len.checked_sub(1) {
            Some(last) => self.set_lead(last),
            None => self.lead = None,
        }
    }

    /// Apply a click resolved by its modifiers (see [`RowClick`]).
    pub fn apply_click(&mut self, click: RowClick) {
        if click.shift {
            self.range_to(click.disp);
        } else if click.cmd {
            self.toggle(click.disp);
        } else {
            self.select_one(click.disp);
        }
        // A data-cell click pins the cursor to the exact cell (set_lead only tracked the row).
        if let Some(col) = click.col {
            self.set_cursor(click.disp, col);
        }
    }

    /// Move the cell cursor by `(dr, dc)` within `len` display rows × `ncols` columns,
    /// updating the row selection to follow (Shift/`extend` grows the range from the anchor).
    /// The first move with no cursor just *places* it on the lead row. Returns whether
    /// anything changed, so the caller knows to scroll the cursor into view.
    pub fn move_cursor(
        &mut self,
        dr: isize,
        dc: isize,
        len: usize,
        ncols: usize,
        extend: bool,
    ) -> bool {
        if len == 0 || ncols == 0 {
            return false;
        }
        let Some((row, col)) = self.cursor else {
            // First arrow press just lands the cursor on the lead row (set_lead places it).
            let seed = self.lead.unwrap_or(0).min(len - 1);
            if extend {
                self.range_to(seed);
            } else {
                self.select_one(seed);
            }
            return true;
        };
        let nr = row.saturating_add_signed(dr).min(len - 1);
        let nc = col.saturating_add_signed(dc).min(ncols - 1);
        if (nr, nc) == (row, col) {
            return false;
        }
        self.cursor = Some((nr, nc));
        if nr != row {
            if extend {
                self.range_to(nr);
            } else {
                self.select_one(nr);
            }
        } else if self.rows.is_empty() {
            // A column-only move never rewrites an existing multi-row selection, but from
            // nothing it should at least select the cursor's row.
            self.select_one(nr);
        }
        true
    }

    /// Drop any selected/anchor/lead/cursor index that no longer addresses a row (rows that
    /// filtered out or were removed). `len` is the number of addressable display rows and
    /// `ncols` the number of columns.
    pub fn clamp(&mut self, len: usize, ncols: usize) {
        self.rows.retain(|&d| d < len);
        if self.anchor.is_some_and(|a| a >= len) {
            self.anchor = None;
        }
        if self.lead.is_some_and(|l| l >= len) {
            self.lead = self.rows.iter().next_back().copied();
        }
        self.cursor = match self.cursor {
            Some((r, _)) if r >= len => None,
            Some((r, c)) if ncols > 0 => Some((r, c.min(ncols - 1))),
            _ => None,
        };
    }
}

/// What the grid reports back to the app after a frame.
#[derive(Default)]
pub struct GridResponse {
    /// A header sort request (click or menu).
    pub sort: Option<SortCmd>,
    /// A row was clicked → resolve this against the current [`Selection`].
    pub selected: Option<RowClick>,
    /// A row's context menu picked "Copy as …": the right-clicked *display* row and the
    /// chosen format. The app copies the current selection (targeting this row if it wasn't
    /// already selected).
    pub copy: Option<(usize, dbcore::CopyFormat)>,
    /// A foreign-key cell asked to be followed (Shift+click the underlined value, or the
    /// right-click "Follow →" menu): the *raw* result-row index and the column. The app resolves
    /// the FK target and opens a filtered tab on it.
    pub follow_fk: Option<(usize, usize)>,
    /// A cell was double-clicked → start editing it (*display* row index, column index).
    pub begin_edit: Option<(usize, usize)>,
    /// A boolean cell was double-clicked → flip it (*display* row index, column index).
    pub toggle: Option<(usize, usize)>,
    /// A fill handle drag completed.
    pub fill: Option<FillRequest>,
    /// The open editor should be committed (Enter pressed or focus lost). The inner value
    /// is a Tab/Shift+Tab advance request: move the cursor that way and keep editing.
    pub commit_edit: Option<Option<crate::edit::CursorDir>>,
    /// The open editor should be discarded (Escape pressed).
    pub cancel_edit: bool,
    /// Empty table space was double-clicked → append a new (insert) row.
    pub add_row: bool,
    /// The active cell editor's outline: `(rect, valid)`. Painted *after* the whole table so
    /// the next column's selection/stripe background can't clip its right edge. `valid`
    /// picks the colour (accent when the input is committable, danger when not).
    edit_border: Option<(egui::Rect, bool)>,
    /// The cell cursor's outline rect, painted in the same post-table pass as `edit_border`
    /// so column stripes/selection backgrounds can't clip it.
    cursor_border: Option<egui::Rect>,
    /// Fill-handle square for the cursor/fill range. Painted after the cursor border so it is
    /// always visible at the lower-right corner.
    fill_handle: Option<egui::Rect>,
    fill_handle_source: Option<(usize, usize)>,
    fill_range_border: Option<egui::Rect>,
}

/// What a body row at a given display index represents.
enum RowKind {
    /// A stored result row (value is the raw index into `result.rows`).
    Stored(usize),
    /// A new (insert) row being filled in (value is its [`crate::edit::NEW_ROW_BASE`] id).
    New(usize),
}

/// Render the result set. `order` maps display rows → indices into `result.rows`.
/// `selection` is the current multi-row selection. `grid_id` must be unique per tab so
/// egui's per-widget click-time memory doesn't bleed between tabs. `scroll_to` scrolls a
/// display row into view this frame (keyboard cursor moves).
#[allow(clippy::too_many_arguments)]
pub fn results_grid(
    ui: &mut egui::Ui,
    result: &QueryResult,
    order: &[usize],
    sort: Option<(usize, bool)>,
    selection: &Selection,
    edits: &mut Edits,
    editable: bool,
    grid_id: u64,
    scroll_to: Option<usize>,
    emoji: &EmojiAtlas,
    fk_cols: &[Option<String>],
) -> GridResponse {
    let mut out = GridResponse::default();
    let ncols = result.columns.len();
    if ncols == 0 {
        ui.weak("No columns to display.");
        return out;
    }

    let row_height = egui::TextStyle::Monospace.resolve(ui.style()).size + 8.0;
    // Width of the row-number gutter, sized to the largest row number.
    let digits = (order.len().max(1) as f64).log10().floor() as usize + 1;
    let gutter_w = 18.0 + 8.0 * digits as f32;

    // egui_extras' Table hardcodes horizontal scrolling off, so once the columns no longer
    // fit it just squeezes them. We detect that case and wrap the table in our own
    // horizontal ScrollArea, sizing the inner ui to the columns' natural width so they keep
    // a readable width and scroll sideways instead. When they fit, render inline so columns
    // still expand to fill the panel.
    let spacing = ui.spacing().item_spacing.x;
    let desired_total = gutter_w + ncols as f32 * (COL_W + spacing);
    let table_rect = ui.available_rect_before_wrap();
    let rendered_rows = order.len() + edits.new_rows;

    // When editable, reserve the add-row strip *below* the table rather than overlaying
    // it on top: rows can then never render under the strip, so it can't steal their
    // clicks (egui's hit-test gives ties to the last-registered widget — an overlay made
    // the bottom ~24 px of a scrolled grid a dead zone where rows couldn't be selected
    // and a double-click added a row instead of opening the editor).
    let grid_rect = if editable {
        egui::Rect::from_min_max(
            table_rect.min,
            egui::pos2(
                table_rect.right(),
                (table_rect.bottom() - ADD_ROW_ZONE).max(table_rect.top()),
            ),
        )
    } else {
        table_rect
    };

    ui.scope_builder(egui::UiBuilder::new().max_rect(grid_rect), |ui| {
        if desired_total <= ui.available_width() {
            build_grid(
                ui, result, order, sort, selection, edits, editable, gutter_w, row_height,
                &mut out, grid_id, scroll_to, emoji, fk_cols,
            );
        } else {
            egui::ScrollArea::horizontal()
                .id_salt(("results_hscroll", grid_id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(desired_total);
                    build_grid(
                        ui, result, order, sort, selection, edits, editable, gutter_w, row_height,
                        &mut out, grid_id, scroll_to, emoji, fk_cols,
                    );
                    // Horizontal keep-visible for keyboard cursor moves. This request must
                    // be issued *here* — inside this horizontal ScrollArea but outside the
                    // table — because egui scroll areas take the pending scroll targets for
                    // BOTH axes regardless of which they scroll (to stop targets leaking
                    // across areas), so anything set inside the table is swallowed by its
                    // internal vertical ScrollArea and never reaches this one.
                    if scroll_to.is_some() {
                        if let Some(rect) = out.cursor_border {
                            ui.scroll_to_rect(rect, None);
                        }
                    }
                });
        }
    });
    // While filling, draw one clean outside border around the whole target range. Otherwise
    // draw the normal active-cell cursor border.
    if let Some(rect) = out.fill_range_border {
        ui.painter().with_clip_rect(grid_rect).rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.5, palette::ACCENT()),
            egui::StrokeKind::Inside,
        );
    } else if let Some(rect) = out.cursor_border {
        ui.painter().with_clip_rect(grid_rect).rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, palette::ACCENT()),
            egui::StrokeKind::Inside,
        );
    }
    let handle_to_paint = out
        .fill_range_border
        .map(fill_handle_rect)
        .or(out.fill_handle);
    if let Some(rect) = handle_to_paint {
        let painter = ui.painter().with_clip_rect(grid_rect);
        painter.rect_filled(rect, egui::CornerRadius::same(1), palette::ACCENT());
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(1),
            egui::Stroke::new(1.0, palette::CODE_BG()),
            egui::StrokeKind::Inside,
        );
    }
    // Paint the active editor's outline last, over every cell, so the next column's
    // selection/stripe background (drawn after the edited cell) can't clip its right edge.
    // Clipped to the grid so an edge cell's outline can't stray onto a neighbouring panel.
    if let Some((rect, valid)) = out.edit_border {
        let color = if valid {
            palette::ACCENT()
        } else {
            palette::DANGER()
        };
        ui.painter().with_clip_rect(grid_rect).rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    capture_empty_table_double_click(
        ui,
        table_rect,
        rendered_rows,
        row_height,
        editable,
        grid_id,
        &mut out,
    );

    out
}

/// Build the `egui_extras` table into `ui`, reporting header/row clicks via `out`. Split out
/// of [`results_grid`] so it can render either inline or inside a horizontal scroll area.
#[allow(clippy::too_many_arguments)]
fn build_grid(
    ui: &mut egui::Ui,
    result: &QueryResult,
    order: &[usize],
    sort: Option<(usize, bool)>,
    selection: &Selection,
    edits: &mut Edits,
    editable: bool,
    gutter_w: f32,
    row_height: f32,
    out: &mut GridResponse,
    grid_id: u64,
    scroll_to: Option<usize>,
    emoji: &EmojiAtlas,
    fk_cols: &[Option<String>],
) {
    let ncols = result.columns.len();
    let fill_id = egui::Id::new(("results_grid_fill_handle", grid_id));
    let mut fill_drag = ui.data_mut(|d| d.get_temp::<FillDrag>(fill_id));
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let cursor = selection.cursor();
    let active_cell = edits.active.as_ref().map(|a| (a.row, a.col, a.origin));
    // Captured once per frame: a click is reported in the same frame, so these reflect the
    // modifiers held as the row was clicked (plain vs. Cmd/Ctrl toggle vs. Shift range).
    let modifiers = ui.input(|i| i.modifiers);
    // The grid's visible bounds, grabbed before the table narrows the clip per column. Used to
    // keep the active editor (and its outline) from spilling over adjacent panels/scrollbars.
    let grid_clip = ui.clip_rect();
    let mut builder = TableBuilder::new(ui)
        // A stable, unique id keeps the table's internal scroll/resize/row ids consistent
        // across frames — this is what prevents egui's "ID clash" outline from flickering
        // while scrolling fast (egui's own warning advises giving tables a unique id_salt).
        // grid_id is per-tab so widgets across tabs never share egui click-time memory.
        .id_salt(("results_grid", grid_id))
        // Cells must sense clicks for row selection (`row.response().clicked()`) to work;
        // the default is hover-only.
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .column(Column::exact(gutter_w)); // gutter (not resizable)
    for _ in 0..ncols {
        builder = builder.column(
            Column::initial(COL_W)
                .at_least(40.0)
                .clip(true)
                .resizable(true),
        );
    }
    // Keyboard cursor moves scroll their row into view (minimal scroll; no-op if visible).
    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, None);
    }

    builder
        .header(HEADER_H, |mut header| {
            header.col(|ui| {
                components::paint_table_header_cell(ui);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("#")
                        .color(palette::TEXT_FAINT())
                        .monospace(),
                );
            });
            for (i, col) in result.columns.iter().enumerate() {
                header.col(|ui| header_cell(ui, i, col, sort, out));
            }
        })
        .body(|body| {
            let new_rows = edits.new_rows;
            let total = order.len() + new_rows;
            body.rows(row_height, total, |mut row| {
                let disp = row.index();
                // Display index splits into: stored rows, then new rows.
                let kind = if disp < order.len() {
                    RowKind::Stored(order[disp])
                } else {
                    RowKind::New(crate::edit::NEW_ROW_BASE + (disp - order.len()))
                };
                let r = match kind {
                    RowKind::Stored(r) | RowKind::New(r) => r,
                };
                let state = edits.row_state(r);
                let cursor_col = cursor.and_then(|(row, col)| (row == disp).then_some(col));
                row.set_selected(selection.contains(disp));

                // Row-number gutter: number for stored rows, a mark for new rows. Tinted
                // green (edit/new) or red (delete) like the cells. Double-clicking the gutter
                // is a common way to start editing a row, so we treat it the same as
                // double-clicking the first data cell.
                let (_, gutter_resp) = row.col(|ui| {
                    tint_row(ui, state);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        let label = match kind {
                            RowKind::Stored(_) => format!("{}", disp + 1),
                            RowKind::New(_) => "✱".to_string(),
                        };
                        ui.weak(egui::RichText::new(label).monospace());
                    });
                });
                if let (Some(fill), Some(pointer)) = (fill_drag.as_mut(), pointer_pos) {
                    if gutter_resp.rect.y_range().contains(pointer.y) {
                        fill.target_disp = disp;
                    }
                }
                if editable
                    && gutter_resp.double_clicked()
                    && state != crate::edit::RowState::Deleted
                {
                    let first_editable =
                        (0..ncols).find(|&c| edits.col_kind(c) != EditorKind::Bool);
                    if let Some(c) = first_editable {
                        out.begin_edit = Some((disp, c));
                    }
                }

                let null = Value::Null;
                let mut clicked_col = None;
                // Set when a Shift+click on this row followed a foreign key, so the row's range
                // selection is skipped for that click (the click navigated, it didn't select).
                let mut follow_click = false;
                let filling = fill_drag.is_some();
                for c in 0..ncols {
                    // The original/stored value behind this cell (NULL for new rows).
                    let stored = match kind {
                        RowKind::Stored(r) => &result.rows[r][c],
                        _ => &null,
                    };
                    // A followable foreign-key cell: a stored, non-null value in an FK column.
                    // Shift-hovering underlines it (link); a Shift+click follows the key. `fk_ref`
                    // is the referenced table's name (for the right-click "Follow →" label).
                    let fk_ref = fk_cols.get(c).and_then(|f| f.as_deref());
                    let fk_raw = (fk_ref.is_some() && disp < order.len()
                        && !result.rows[r][c].is_null())
                    .then_some(r);
                    let mut shift_hover = false;
                    let (_, col_resp) = row.col(|ui| {
                        tint_row(ui, state);
                        // Record the cell cursor's rect for the post-table outline paint and
                        // the horizontal keep-visible scroll. Deliberately *before* the
                        // visibility early-return below and unclipped: a cursor cell that
                        // scrolled off the side is exactly the one whose rect the scroll
                        // needs (painting is clipped to the grid separately).
                        if cursor_col == Some(c) {
                            let full = ui.max_rect().expand2(0.5 * ui.spacing().item_spacing);
                            out.cursor_border = Some(full);
                            if editable
                                && state != crate::edit::RowState::Deleted
                                && !active_cell.is_some_and(|(row, col, _)| row == r && col == c)
                                && ui.is_rect_visible(full)
                                && !matches!(stored, Value::Bytes(_))
                            {
                                let handle = fill_handle_rect(full);
                                out.fill_handle = Some(handle);
                                out.fill_handle_source = Some((disp, c));
                            }
                        }
                        if let Some(fill) = fill_drag {
                            if fill.col == c
                                && in_fill_range(fill.from_disp, fill.target_disp, disp)
                            {
                                let rect = ui.max_rect().expand2(0.5 * ui.spacing().item_spacing);
                                out.fill_range_border = Some(match out.fill_range_border {
                                    Some(acc) => acc.union(rect),
                                    None => rect,
                                });
                                out.fill_handle = Some(fill_handle_rect(rect));
                            }
                        }
                        // Skip text layout, HashMap lookups, and widget allocation for
                        // columns outside the visible horizontal viewport. The cell rect is
                        // still allocated by egui_extras (needed for column sizing), but
                        // we avoid all the per-cell work for invisible columns.
                        if !ui.is_rect_visible(ui.max_rect()) {
                            return;
                        }
                        // Shift+hover over a followable FK cell: underline the value (it reads as a
                        // link) and switch to a hand cursor. `ui` here is the cell's own Ui, so the
                        // cursor can be set directly (unlike after the table, where it's borrowed).
                        shift_hover = fk_raw.is_some()
                            && modifiers.shift
                            && pointer_pos.is_some_and(|p| ui.max_rect().contains(p));
                        if shift_hover {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        let active_from_grid = active_cell.is_some_and(|(row, col, origin)| {
                            row == r && col == c && origin == crate::edit::EditOrigin::Grid
                        });
                        if active_from_grid {
                            // The cell under edit fills the whole cell; the editor is
                            // type-aware and validates numbers/dates before they can commit.
                            // (An edit begun in the Details panel renders its editor there,
                            // not here — two editors would fight over keyboard focus.)
                            if let Some(active) = edits.active.as_mut() {
                                // egui_extras paints the cell background (stripe/selection) over
                                // `max_rect` expanded by half the item spacing, so the visible
                                // cell is larger than this content rect. Render the editor onto
                                // that same expanded rect so its fill covers the whole visible
                                // cell — clamped to the grid so it never spills over an adjacent
                                // panel or the scrollbar.
                                let full = ui
                                    .max_rect()
                                    .expand2(0.5 * ui.spacing().item_spacing)
                                    .intersect(grid_clip);
                                let valid = active.kind.is_valid(&active.buf);
                                let mut outcome = EditOutcome::Continue;
                                ui.scope_builder(
                                    egui::UiBuilder::new()
                                        .max_rect(full)
                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                    |ui| {
                                        ui.set_clip_rect(full);
                                        outcome = crate::edit::render_editor(
                                            ui,
                                            active,
                                            Some(full.size()),
                                        );
                                    },
                                );
                                // Defer the outline to a post-table pass: the next column's
                                // selection/stripe background is painted *after* this cell and
                                // would otherwise cover the right edge, leaving only three sides.
                                out.edit_border = Some((full, valid));
                                match outcome {
                                    EditOutcome::Commit { advance } => {
                                        out.commit_edit = Some(advance)
                                    }
                                    EditOutcome::Cancel => out.cancel_edit = true,
                                    EditOutcome::Continue => {}
                                }
                            }
                        } else {
                            // Show the staged value if present, else the stored one. Foreign-key
                            // cells with a real (non-null) value read as accent-coloured links,
                            // underlined while Shift-hovered to signal the click will follow.
                            let staged = edits.staged(r, c);
                            let value = staged.unwrap_or(stored);
                            let link = fk_ref.is_some() && !value.is_null();
                            cell(ui, value, staged.is_some(), link, shift_hover, emoji);
                        }
                    });

                    // Entering cell-edit (binary cells aren't editable; deleted rows are on
                    // their way out). Booleans toggle in place; everything else opens the inline
                    // editor. `col_resp` is the egui_extras cell `Ui` response, which senses
                    // clicks across the entire cell rect (it calls `set_min_size` on the cell) —
                    // a single, stable hit target, so clicks register reliably anywhere.
                    //
                    // From idle it takes a double-click (a single click selects). But once any
                    // editor is already open, a *single* click moves the editor straight to the
                    // clicked cell — spreadsheet-style, so editing many cells in a row stays
                    // fluid and never depends on landing a clean double-click on a moving target.
                    // Never re-trigger on the cell already being edited — a stray click on its
                    // border would otherwise reset the editor buffer mid-typing.
                    if !filling && (col_resp.clicked() || col_resp.double_clicked()) {
                        clicked_col = Some(c);
                    }
                    // Our own double-click detection, keyed on *cell identity* instead of
                    // egui's widget id + ≤6pt pointer distance: two clicks on the same
                    // (disp, c) within the double-click delay count, even if the first click's
                    // selection change shifted the layout or the pointer drifted across the
                    // cell. Cleared after firing so a triple click can't fire twice.
                    let mut manual_dbl = false;
                    if !filling && col_resp.clicked() && edits.active.is_none() {
                        let id = egui::Id::new(("grid_last_click", grid_id));
                        let ctx = &col_resp.ctx;
                        let now = ctx.input(|i| i.time);
                        let delay = ctx.options(|o| o.input_options.max_double_click_delay);
                        let prev: Option<(usize, usize, f64)> = ctx.data_mut(|d| d.get_temp(id));
                        if prev.is_some_and(|(pd, pc, t)| pd == disp && pc == c && now - t <= delay)
                        {
                            manual_dbl = true;
                            ctx.data_mut(|d| d.remove::<(usize, usize, f64)>(id));
                        } else {
                            ctx.data_mut(|d| d.insert_temp(id, (disp, c, now)));
                        }
                    }
                    let active_here = active_cell.is_some_and(|(row, col, _)| row == r && col == c);
                    let start_edit = !active_here
                        && !filling
                        && (col_resp.double_clicked()
                            || manual_dbl
                            || (edits.active.is_some() && col_resp.clicked()));
                    if editable
                        && start_edit
                        && state != crate::edit::RowState::Deleted
                        && !matches!(stored, Value::Bytes(_))
                    {
                        if edits.col_kind(c) == EditorKind::Bool {
                            out.toggle = Some((disp, c));
                        } else {
                            out.begin_edit = Some((disp, c));
                        }
                    }

                    // Shift+click a Shift-hovered (underlined) FK cell → follow the key, and skip
                    // this click's row range-select (it navigated). Gated on no open editor so a
                    // single click still moves an active editor as usual. The right-click "Follow →"
                    // menu below is the discoverable, modifier-free path.
                    if let Some(raw) = fk_raw {
                        if shift_hover && col_resp.clicked() && edits.active.is_none() {
                            out.follow_fk = Some((raw, c));
                            follow_click = true;
                        }
                    }
                    // Per-cell context menu: "Follow →" (FK cells) plus the row copy actions.
                    // Copy targets this row; the app copies the whole selection, or just this
                    // row when it was right-clicked while unselected (TablePlus-style).
                    col_resp.context_menu(|ui| {
                        if let (Some(raw), Some(ref_t)) = (fk_raw, fk_ref) {
                            if ui.button(format!("↗  Follow → {ref_t}")).clicked() {
                                out.follow_fk = Some((raw, c));
                                ui.close();
                            }
                            ui.separator();
                        }
                        ui.label(
                            egui::RichText::new("Copy selected rows")
                                .small()
                                .color(palette::TEXT_FAINT()),
                        );
                        for fmt in [
                            dbcore::CopyFormat::Tsv,
                            dbcore::CopyFormat::Csv,
                            dbcore::CopyFormat::Json,
                            dbcore::CopyFormat::Insert,
                        ] {
                            if ui.button(format!("Copy as {}", fmt.label())).clicked() {
                                out.copy = Some((disp, fmt));
                                ui.close(); // egui 0.34 replacement for the deprecated close_menu()
                            }
                        }
                    });
                }

                let row_resp = row.response();
                if !filling && !follow_click && row_resp.clicked() {
                    out.selected = Some(RowClick {
                        disp,
                        col: clicked_col,
                        shift: modifiers.shift,
                        cmd: modifiers.command,
                    });
                }
            });
        });

    if let (Some(handle), Some((disp, col))) = (out.fill_handle, out.fill_handle_source) {
        let resp = ui.interact(
            handle,
            egui::Id::new(("grid_fill_handle", grid_id, disp, col)),
            egui::Sense::drag(),
        );
        if resp.drag_started() {
            fill_drag = Some(FillDrag {
                from_disp: disp,
                target_disp: disp,
                col,
            });
        }
    }

    if let Some(fill) = fill_drag {
        if ui.input(|i| i.pointer.any_released()) {
            if fill.target_disp != fill.from_disp {
                out.fill = Some(FillRequest {
                    from_disp: fill.from_disp,
                    to_disp: fill.target_disp,
                    col: fill.col,
                });
            }
            ui.data_mut(|d| d.remove::<FillDrag>(fill_id));
        } else {
            ui.data_mut(|d| d.insert_temp(fill_id, fill));
        }
    } else {
        ui.data_mut(|d| d.remove::<FillDrag>(fill_id));
    }
}

fn fill_handle_rect(cell: egui::Rect) -> egui::Rect {
    const SIZE: f32 = 8.0;
    egui::Rect::from_min_size(
        egui::pos2(cell.right() - SIZE - 2.0, cell.bottom() - SIZE - 2.0),
        egui::vec2(SIZE, SIZE),
    )
}

fn in_fill_range(from: usize, to: usize, row: usize) -> bool {
    row >= from.min(to) && row <= from.max(to)
}

/// One column header: the name (click to toggle sort), an accent underline + arrow when it's
/// the active sort, and a `⌄` menu (also reachable by right-click) with explicit
/// sort/clear/copy actions — TablePlus-style.
fn header_cell(
    ui: &mut egui::Ui,
    i: usize,
    col: &dbcore::ColumnMeta,
    sort: Option<(usize, bool)>,
    out: &mut GridResponse,
) {
    components::paint_table_header_cell(ui);
    let sorted_dir = match sort {
        Some((c, asc)) if c == i => Some(asc),
        _ => None,
    };
    // Accent underline marks the active sort column (drawn over the default header border).
    if sorted_dir.is_some() {
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 1.0,
            egui::Stroke::new(2.0, palette::ACCENT()),
        );
    }
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(6.0);
        let arrow = match sorted_dir {
            Some(true) => "  ↑",
            Some(false) => "  ↓",
            None => "",
        };
        let color = if sorted_dir.is_some() {
            palette::ACCENT()
        } else {
            palette::TEXT()
        };
        let text = egui::RichText::new(format!("{}{arrow}", col.name))
            .strong()
            .color(color);
        let name = ui
            .add(
                egui::Label::new(text)
                    .sense(egui::Sense::click())
                    .selectable(false),
            )
            .on_hover_text(format!(
                "{}  ·  click to sort · right-click for options",
                col.type_name
            ));
        if name.clicked() {
            out.sort = Some(SortCmd::Toggle(i));
        }
        name.context_menu(|ui| header_menu(ui, i, col, sorted_dir.is_some(), out));

        // A small frameless ⋮ icon pinned to the right edge opens the column menu — far
        // lighter than a full button, and it reads as "more actions here".
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let dots = egui::Image::new(crate::icons::more_vert())
                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                .tint(palette::TEXT_FAINT())
                .sense(egui::Sense::click());
            let menu = ui.add(dots).on_hover_text("Column options");
            egui::Popup::menu(&menu)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| header_menu(ui, i, col, sorted_dir.is_some(), out));
        });
    });
}

/// The per-column header menu (shared by the `⌄` button and the right-click context menu).
fn header_menu(
    ui: &mut egui::Ui,
    col: usize,
    meta: &dbcore::ColumnMeta,
    is_sorted: bool,
    out: &mut GridResponse,
) {
    ui.set_min_width(190.0);
    // Column identity at the top, so the menu doubles as a "what is this column" tooltip.
    ui.label(egui::RichText::new(&meta.name).strong());
    if !meta.type_name.is_empty() {
        ui.label(
            egui::RichText::new(&meta.type_name)
                .color(palette::TEXT_FAINT())
                .small(),
        );
    }
    ui.separator();
    if ui.button("Sort Ascending   ↑").clicked() {
        out.sort = Some(SortCmd::Asc(col));
        ui.close();
    }
    if ui.button("Sort Descending  ↓").clicked() {
        out.sort = Some(SortCmd::Desc(col));
        ui.close();
    }
    if ui
        .add_enabled(is_sorted, egui::Button::new("Clear Sort"))
        .clicked()
    {
        out.sort = Some(SortCmd::Clear);
        ui.close();
    }
    ui.separator();
    if ui.button("Copy column name").clicked() {
        ui.ctx().copy_text(meta.name.clone());
        ui.close();
    }
    if ui.button("Copy column type").clicked() {
        ui.ctx().copy_text(meta.type_name.clone());
        ui.close();
    }
}

/// Turn the blank space under the table rows into an invisible add-row target. The table
/// itself stops [`ADD_ROW_ZONE`] above the panel bottom (see [`results_grid`]), so the zone
/// never overlaps a row: it covers the reserved strip plus any empty space above it when
/// the rows don't fill the panel.
fn capture_empty_table_double_click(
    ui: &mut egui::Ui,
    table_rect: egui::Rect,
    rendered_rows: usize,
    row_height: f32,
    editable: bool,
    grid_id: u64,
    out: &mut GridResponse,
) {
    if !editable {
        return;
    }
    // egui_extras positions each row at row_height + item_spacing.y intervals (see
    // TableBody::rows), so content_bottom must use the same stride or the zone would start
    // inside the last row's space when the table doesn't fill the panel.
    let row_step = row_height + ui.spacing().item_spacing.y;
    let content_bottom = table_rect.top() + HEADER_H + rendered_rows as f32 * row_step;
    let zone_top = content_bottom.min(table_rect.bottom() - ADD_ROW_ZONE);
    if zone_top >= table_rect.bottom() {
        return;
    }
    let zone_rect = egui::Rect::from_min_max(
        egui::pos2(table_rect.left(), zone_top),
        egui::pos2(table_rect.right(), table_rect.bottom()),
    );
    // grid_id is included so the click-time memory is per-tab, matching the table's own id_salt.
    let resp = ui.interact(
        zone_rect,
        egui::Id::new(("results_grid_empty_add_row", grid_id)),
        egui::Sense::click(),
    );
    if resp.double_clicked() {
        out.add_row = true;
    }
}

/// Paint a faint wash over the current cell to flag its pending state: green for edited or
/// new rows (a pending write), red for rows marked for deletion. Clean rows are untouched.
fn tint_row(ui: &egui::Ui, state: crate::edit::RowState) {
    use crate::edit::RowState;
    let tint = match state {
        RowState::Clean => return,
        RowState::Edited | RowState::New => {
            let s = palette::SUCCESS();
            egui::Color32::from_rgba_unmultiplied(s.r(), s.g(), s.b(), 28)
        }
        RowState::Deleted => {
            let d = palette::DANGER();
            egui::Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), 32)
        }
    };
    ui.painter().rect_filled(ui.max_rect(), 0.0, tint);
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use dbcore::{ColumnMeta, QueryResult, QueryStats};

    fn rows(sel: &Selection) -> Vec<usize> {
        sel.iter().collect()
    }

    /// Plain → Cmd toggle → Shift range mirror the Finder/Excel mental model, and `lead`
    /// always tracks the most recently affected row (what the Details panel shows).
    #[test]
    fn selection_click_modes() {
        let mut s = Selection::default();

        // Plain click selects exactly one and anchors there.
        s.apply_click(RowClick {
            disp: 3,
            col: None,
            shift: false,
            cmd: false,
        });
        assert_eq!(rows(&s), [3]);
        assert_eq!(s.lead(), Some(3));

        // Cmd/Ctrl click adds without dropping the rest, and re-anchors on the new row.
        s.apply_click(RowClick {
            disp: 5,
            col: None,
            shift: false,
            cmd: true,
        });
        assert_eq!(rows(&s), [3, 5]);
        assert_eq!(s.lead(), Some(5));

        // Cmd/Ctrl click on a selected row removes it; lead falls back to a survivor.
        s.apply_click(RowClick {
            disp: 5,
            col: None,
            shift: false,
            cmd: true,
        });
        assert_eq!(rows(&s), [3]);
        assert_eq!(s.lead(), Some(3));

        // Shift extends from the anchor — the last *non-Shift* click (row 5 above), Finder-style.
        s.apply_click(RowClick {
            disp: 6,
            col: None,
            shift: true,
            cmd: false,
        });
        assert_eq!(rows(&s), [5, 6]);
        assert_eq!(s.lead(), Some(6));

        // A second Shift click re-extends from the *same* anchor (5), not the previous end (6).
        s.apply_click(RowClick {
            disp: 1,
            col: None,
            shift: true,
            cmd: false,
        });
        assert_eq!(rows(&s), [1, 2, 3, 4, 5]);
        assert_eq!(s.lead(), Some(1));

        // A plain click collapses back to one row and re-anchors there.
        s.apply_click(RowClick {
            disp: 9,
            col: None,
            shift: false,
            cmd: false,
        });
        assert_eq!(rows(&s), [9]);
        assert_eq!(s.lead(), Some(9));
    }

    /// The cell cursor follows clicks (exact cell for data cells, row-only for the gutter),
    /// moves with arrow deltas clamped to the grid, and drags the row selection along.
    #[test]
    fn cursor_follows_clicks_and_moves() {
        let mut s = Selection::default();
        // A data-cell click pins the cursor to the exact cell.
        s.apply_click(RowClick {
            disp: 2,
            col: Some(1),
            shift: false,
            cmd: false,
        });
        assert_eq!(s.cursor(), Some((2, 1)));
        // A gutter click (no column) moves the row but keeps the column.
        s.apply_click(RowClick {
            disp: 4,
            col: None,
            shift: false,
            cmd: false,
        });
        assert_eq!(s.cursor(), Some((4, 1)));

        // Arrow moves update the cursor and re-select its row (10 rows × 3 cols).
        assert!(s.move_cursor(1, 0, 10, 3, false));
        assert_eq!(s.cursor(), Some((5, 1)));
        assert_eq!(rows(&s), [5]);
        // Column moves keep the row selection.
        assert!(s.move_cursor(0, 1, 10, 3, false));
        assert_eq!(s.cursor(), Some((5, 2)));
        assert_eq!(rows(&s), [5]);
        // Clamped at the edges: a no-op move reports false (no scroll needed).
        assert!(!s.move_cursor(0, 1, 10, 3, false));
        assert!(s.move_cursor(-10, 0, 10, 3, false));
        assert_eq!(s.cursor(), Some((0, 2)));
        assert!(!s.move_cursor(-1, 0, 10, 3, false));

        // Shift+arrows extend the range from the anchor, cursor tracking the moving end.
        assert!(s.move_cursor(2, 0, 10, 3, true));
        assert_eq!(rows(&s), [0, 1, 2]);
        assert_eq!(s.cursor(), Some((2, 2)));
        assert_eq!(s.lead(), Some(2));
    }

    /// With no cursor placed yet, the first arrow press *lands* it on the lead row instead
    /// of moving; clamp drops a cursor whose row vanished and clamps its column.
    #[test]
    fn cursor_seeds_on_first_move_and_clamps() {
        let mut s = Selection::default();
        assert!(s.move_cursor(1, 0, 5, 2, false));
        assert_eq!(s.cursor(), Some((0, 0)));
        assert_eq!(rows(&s), [0]);

        s.set_cursor(4, 1);
        s.clamp(3, 2); // row 4 no longer addressable
        assert_eq!(s.cursor(), None);
        s.set_cursor(2, 1);
        s.clamp(3, 1); // column 1 no longer exists
        assert_eq!(s.cursor(), Some((2, 0)));
    }

    #[test]
    fn selection_select_all_and_clamp() {
        let mut s = Selection::default();
        s.select_all(4);
        assert_eq!(rows(&s), [0, 1, 2, 3]);
        assert_eq!(s.len(), 4);

        // Shrinking the addressable range drops out-of-range rows and re-homes the lead.
        s.clamp(2, 4);
        assert_eq!(rows(&s), [0, 1]);
        assert_eq!(s.lead(), Some(1));

        s.clamp(0, 4);
        assert!(s.is_empty());
        assert_eq!(s.lead(), None);
    }

    fn fake_result(rows: usize, cols: usize) -> QueryResult {
        let columns = (0..cols)
            .map(|c| ColumnMeta {
                name: format!("col{c}"),
                type_name: "TEXT".into(),
            })
            .collect();
        let data = (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| Value::Int((r * cols + c) as i64))
                    .collect()
            })
            .collect();
        QueryResult {
            columns,
            rows: data,
            stats: QueryStats::default(),
            truncated: false,
        }
    }

    fn collect_clash_text(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    let s = t.galley.text();
                    if s.contains('🔥') {
                        out.push(s.to_string());
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        for cs in shapes {
            walk(&cs.shape, out);
        }
    }

    /// Render the grid headlessly across a few frames while injecting scroll, and capture
    /// any egui "ID clash" markers (🔥) so we can pinpoint the offending widget.
    #[test]
    fn probe_id_clash_while_scrolling() {
        let ctx = egui::Context::default();
        let result = fake_result(2000, 5);
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));

        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..5 {
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(400.0, 300.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -40.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut edits = Edits::default();
            let out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let _ = results_grid(
                        ui,
                        &result,
                        &order,
                        None,
                        &Selection::default(),
                        &mut edits,
                        false,
                        0,
                        None,
                        &EmojiAtlas::default(),
                        &[],
                    );
                });
            });
            collect_clash_text(&out.shapes, &mut clashes);
        }

        assert!(
            clashes.is_empty(),
            "ID clashes detected in results grid:\n{}",
            clashes.join("\n")
        );
    }

    /// The active editor's accent border must be painted on top of every cell so the next
    /// column's (selection/stripe) background can't clip its right edge — the regression that
    /// left the edited cell with only three visible sides.
    #[test]
    fn editor_border_is_painted_on_top() {
        let ctx = egui::Context::default();
        let result = fake_result(10, 3);
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut sel = Selection::default();
        sel.select_one(0); // edited row selected → its cells paint a selection background
        let mut edits = Edits::default();
        edits.set_columns(&result.columns);
        edits.begin(0, 1, &Value::Int(1), crate::edit::EditOrigin::Grid);

        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let _ = results_grid(
                    ui,
                    &result,
                    &order,
                    None,
                    &sel,
                    &mut edits,
                    true,
                    0,
                    None,
                    &EmojiAtlas::default(),
                    &[],
                );
            });
        });

        let accent = palette::ACCENT();
        // Last accent-stroked rect = the on-top border. No filled cell background may be
        // painted after it that would cover its right edge.
        let border = out
            .shapes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, cs)| match &cs.shape {
                egui::epaint::Shape::Rect(r)
                    if r.stroke.color == accent && r.stroke.width > 0.0 =>
                {
                    Some((i, r.rect))
                }
                _ => None,
            });
        let (bi, brect) = border.expect("accent editor border must be painted");
        let covered_after = out.shapes.iter().skip(bi + 1).any(|cs| match &cs.shape {
            egui::epaint::Shape::Rect(r) => {
                r.fill.a() > 0
                    && r.rect.left() <= brect.right()
                    && r.rect.right() >= brect.right() - 1.0
                    && r.rect.top() <= brect.center().y
                    && r.rect.bottom() >= brect.center().y
            }
            _ => false,
        });
        assert!(
            !covered_after,
            "editor right border is overpainted by a later cell background"
        );
    }

    fn find_text_pos(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<egui::Pos2> {
        fn walk(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == needle => Some(t.pos),
                egui::epaint::Shape::Vec(v) => v.iter().find_map(|s| walk(s, needle)),
                _ => None,
            }
        }
        shapes.iter().find_map(|cs| walk(&cs.shape, needle))
    }

    /// Two slowish clicks landing ~20pt apart in the *same cell* must still begin an edit.
    /// egui's own `double_clicked()` (same widget id + ≤6pt pointer distance) is defeated by
    /// exactly this — which is what made double-click-to-edit feel unreliable — so the grid
    /// keeps its own cell-identity click bookkeeping. This is the regression test for it.
    #[test]
    fn double_click_same_cell_survives_pointer_drift() {
        let ctx = egui::Context::default();
        let result = fake_result(10, 3);
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut edits = Edits::default();
        edits.set_columns(&result.columns);
        let selection = Selection::default();

        let mut begin: Option<(usize, usize)> = None;
        let run = |events: Vec<egui::Event>,
                   time: f64,
                   edits: &mut Edits,
                   begin: &mut Option<(usize, usize)>| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let out = results_grid(
                        ui,
                        &result,
                        &order,
                        None,
                        &selection,
                        edits,
                        true,
                        0,
                        None,
                        &EmojiAtlas::default(),
                        &[],
                    );
                    if out.begin_edit.is_some() {
                        *begin = out.begin_edit;
                    }
                });
            })
        };

        // Find cell (row 1, col 1) — its value is 1*3+1 = 4 — and click twice, drifting
        // 20pt right between the clicks, well past egui's 6pt same-click tolerance but
        // still inside the ≥160pt-wide cell, within the double-click delay (0.3s).
        let out = run(vec![], 0.0, &mut edits, &mut begin);
        let text = find_text_pos(&out.shapes, "4").expect("cell text painted");
        let p1 = text + egui::vec2(2.0, 4.0);
        let p2 = text + egui::vec2(22.0, 4.0);
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        run(
            vec![egui::Event::PointerMoved(p1)],
            0.01,
            &mut edits,
            &mut begin,
        );
        run(vec![press(p1, true)], 0.02, &mut edits, &mut begin);
        run(vec![press(p1, false)], 0.03, &mut edits, &mut begin);
        run(
            vec![egui::Event::PointerMoved(p2)],
            0.05,
            &mut edits,
            &mut begin,
        );
        run(vec![press(p2, true)], 0.06, &mut edits, &mut begin);
        run(vec![press(p2, false)], 0.07, &mut edits, &mut begin);

        assert_eq!(
            begin,
            Some((1, 1)),
            "two drifted clicks on the same cell must begin an edit"
        );
    }

    /// The cell cursor paints an accent outline on its cell (post-table, so stripes can't
    /// clip it) — the visible anchor for arrow-key navigation.
    #[test]
    fn cursor_outline_is_painted() {
        let ctx = egui::Context::default();
        let result = fake_result(10, 3);
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut sel = Selection::default();
        sel.select_one(0);
        sel.set_cursor(0, 1);
        let mut edits = Edits::default();
        edits.set_columns(&result.columns);

        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let _ = results_grid(
                    ui,
                    &result,
                    &order,
                    None,
                    &sel,
                    &mut edits,
                    true,
                    0,
                    None,
                    &EmojiAtlas::default(),
                    &[],
                );
            });
        });

        let accent = palette::ACCENT();
        let outline = out.shapes.iter().any(|cs| match &cs.shape {
            egui::epaint::Shape::Rect(r) => r.stroke.color == accent && r.stroke.width > 0.0,
            _ => false,
        });
        assert!(outline, "cursor cell must be outlined in the accent colour");
    }

    /// With an editable, scrollable result the table must stop above the add-row strip.
    /// (It used to extend underneath it, and the invisible strip — registered last, hence
    /// topmost in egui's hit-test — stole single and double clicks from the rows below it.)
    #[test]
    fn editable_grid_reserves_add_row_strip() {
        let ctx = egui::Context::default();
        let result = fake_result(500, 3); // tall enough to scroll
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        // The strip starts ADD_ROW_ZONE above the panel bottom; the central panel's frame
        // margin only pulls the table bottom further up, so this bound is conservative.
        let strip_top = screen.bottom() - ADD_ROW_ZONE;

        let mut edits = Edits::default();
        let mut offenders: Vec<String> = Vec::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let _ = results_grid(
                        ui,
                        &result,
                        &order,
                        None,
                        &Selection::default(),
                        &mut edits,
                        true,
                        0,
                        None,
                        &EmojiAtlas::default(),
                        &[],
                    );
                });
            });
            // Any cell text allowed to paint below the strip top means a row is rendered
            // under the add-row zone. Shapes clipped above the strip can't paint there.
            for cs in &out.shapes {
                if cs.clip_rect.bottom() <= strip_top {
                    continue;
                }
                if let egui::epaint::Shape::Text(t) = &cs.shape {
                    if t.pos.y > strip_top && !t.galley.text().is_empty() {
                        offenders.push(format!("{:?} at y={}", t.galley.text(), t.pos.y));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "table rows rendered under the add-row strip:\n{}",
            offenders.join("\n")
        );
    }
}

/// Render a single cell, dimming NULLs and monospacing numbers. A `staged` value (an edit
/// not yet saved) is drawn in the success colour so it stands out from stored data. Free-text
/// values that contain emoji are drawn through [`emoji_cell`] so the emoji show in colour.
fn cell(
    ui: &mut egui::Ui,
    value: &Value,
    staged: bool,
    link: bool,
    underline: bool,
    emoji: &EmojiAtlas,
) -> egui::Response {
    let color = if staged {
        palette::SUCCESS()
    } else if value.is_null() {
        palette::TEXT_FAINT()
    } else if link {
        // Foreign-key value: accent-tinted so it reads as a followable link (TablePlus-style).
        palette::ACCENT()
    } else {
        palette::TEXT()
    };
    // The label is deliberately non-interactive (no click sense, not selectable). The whole
    // cell is one click-sensing surface — the egui_extras cell `Ui` itself (see the row loop).
    // A click-sensing or selectable label here would be a second, text-width-only hit target
    // overlapping the cell, and double-clicks landing astride the text/empty boundary would
    // split across two widget ids and silently fail to register. One surface = reliable.
    // `underline` marks a Shift-hovered FK value so it reads as a clickable link.
    let label = |ui: &mut egui::Ui, text: egui::RichText| {
        let text = text.color(color);
        let text = if underline { text.underline() } else { text };
        ui.add(egui::Label::new(text).selectable(false))
    };
    let resp = match value {
        Value::Null => label(ui, egui::RichText::new("NULL").italics()),
        Value::Bool(v) => label(ui, egui::RichText::new(if *v { "true" } else { "false" })),
        Value::Int(_) | Value::Float(_) => {
            label(ui, egui::RichText::new(value.display()).monospace())
        }
        Value::Text(text) if emoji::contains_emoji(text) => emoji_cell(ui, text, color, emoji),
        Value::Text(text) => label(ui, egui::RichText::new(text)),
        Value::Bytes(bytes) => label(ui, egui::RichText::new(format!("[{} bytes]", bytes.len()))),
    };
    // A Shift-hovered, followable FK value: append a small ↗ after the text so the cell reads as
    // a navigable link. Purely a marker — the whole cell is the click target (Shift+click follows).
    if underline {
        ui.add(
            egui::Label::new(egui::RichText::new(" ↗").color(palette::ACCENT())).selectable(false),
        );
    }
    resp
}

/// Render a text value that contains emoji: plain runs as labels, each emoji grapheme as an
/// inline colour image sized to the line height. Like [`cell`], every piece is non-interactive
/// so the cell `Ui` stays the single click-sensing surface; the returned response is unused.
/// When the atlas has no colour glyph (or isn't available), that grapheme falls back to text.
fn emoji_cell(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    emoji: &EmojiAtlas,
) -> egui::Response {
    let h = ui.text_style_height(&egui::TextStyle::Body);
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for run in emoji::segment(text) {
            match run {
                emoji::Run::Text(t) if !t.is_empty() => {
                    ui.add(egui::Label::new(egui::RichText::new(t).color(color)).selectable(false));
                }
                emoji::Run::Text(_) => {}
                emoji::Run::Emoji(g) => match emoji.texture(ui.ctx(), g) {
                    Some(id) => {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            id,
                            egui::vec2(h, h),
                        )));
                    }
                    None => {
                        ui.add(
                            egui::Label::new(egui::RichText::new(g).color(color)).selectable(false),
                        );
                    }
                },
            }
        }
    });
    inner.response
}
