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
    /// Shift was held → extend the selection from the anchor to this row.
    pub shift: bool,
    /// Cmd (macOS) / Ctrl (elsewhere) was held → toggle this row in/out of the selection.
    pub cmd: bool,
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

    /// The selected display indices, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().copied()
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
        self.lead = None;
    }

    /// Plain click: select exactly `disp` and make it the anchor.
    pub fn select_one(&mut self, disp: usize) {
        self.rows.clear();
        self.rows.insert(disp);
        self.anchor = Some(disp);
        self.lead = Some(disp);
    }

    /// Cmd/Ctrl click: toggle `disp` in/out, keeping the rest, and re-anchor on it.
    pub fn toggle(&mut self, disp: usize) {
        if !self.rows.insert(disp) {
            self.rows.remove(&disp);
            if self.lead == Some(disp) {
                self.lead = self.rows.iter().next_back().copied();
            }
        } else {
            self.lead = Some(disp);
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
        self.lead = Some(disp);
    }

    /// Select every display row in `0..len` (Cmd/Ctrl+A).
    pub fn select_all(&mut self, len: usize) {
        self.rows = (0..len).collect();
        self.anchor = Some(0);
        self.lead = len.checked_sub(1);
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
    }

    /// Drop any selected/anchor/lead index that no longer addresses a row (rows that filtered
    /// out or were removed). `len` is the number of addressable display rows.
    pub fn clamp(&mut self, len: usize) {
        self.rows.retain(|&d| d < len);
        if self.anchor.is_some_and(|a| a >= len) {
            self.anchor = None;
        }
        if self.lead.is_some_and(|l| l >= len) {
            self.lead = self.rows.iter().next_back().copied();
        }
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
    /// A cell was double-clicked → start editing it (raw row index, column index).
    pub begin_edit: Option<(usize, usize)>,
    /// A boolean cell was double-clicked → flip it (raw row index, column index).
    pub toggle: Option<(usize, usize)>,
    /// The open editor should be committed (Enter pressed or focus lost).
    pub commit_edit: bool,
    /// The open editor should be discarded (Escape pressed).
    pub cancel_edit: bool,
    /// Empty table space was double-clicked → append a new (insert) row.
    pub add_row: bool,
    /// The active cell editor's outline: `(rect, valid)`. Painted *after* the whole table so
    /// the next column's selection/stripe background can't clip its right edge. `valid`
    /// picks the colour (accent when the input is committable, danger when not).
    edit_border: Option<(egui::Rect, bool)>,
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
/// egui's per-widget click-time memory doesn't bleed between tabs.
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
    emoji: &EmojiAtlas,
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
                &mut out, grid_id, emoji,
            );
        } else {
            egui::ScrollArea::horizontal()
                .id_salt(("results_hscroll", grid_id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(desired_total);
                    build_grid(
                        ui, result, order, sort, selection, edits, editable, gutter_w, row_height,
                        &mut out, grid_id, emoji,
                    );
                });
        }
    });
    // Paint the active editor's outline last, over every cell, so the next column's
    // selection/stripe background (drawn after the edited cell) can't clip its right edge.
    // Clipped to the grid so an edge cell's outline can't stray onto a neighbouring panel.
    if let Some((rect, valid)) = out.edit_border {
        let color = if valid { palette::ACCENT() } else { palette::DANGER() };
        ui.painter().with_clip_rect(grid_rect).rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    capture_empty_table_double_click(ui, table_rect, rendered_rows, row_height, editable, grid_id, &mut out);

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
    emoji: &EmojiAtlas,
) {
    let ncols = result.columns.len();
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
                if editable && gutter_resp.double_clicked() && state != crate::edit::RowState::Deleted {
                    let first_editable = (0..ncols)
                        .find(|&c| edits.col_kind(c) != EditorKind::Bool);
                    if let Some(c) = first_editable {
                        out.begin_edit = Some((r, c));
                    }
                }

                let null = Value::Null;
                for c in 0..ncols {
                    // The original/stored value behind this cell (NULL for new rows).
                    let stored = match kind {
                        RowKind::Stored(r) => &result.rows[r][c],
                        _ => &null,
                    };
                    let (_, col_resp) = row.col(|ui| {
                        tint_row(ui, state);
                        // Skip text layout, HashMap lookups, and widget allocation for
                        // columns outside the visible horizontal viewport. The cell rect is
                        // still allocated by egui_extras (needed for column sizing), but
                        // we avoid all the per-cell work for invisible columns.
                        if !ui.is_rect_visible(ui.max_rect()) {
                            return;
                        }
                        if edits.is_active_from(r, c, crate::edit::EditOrigin::Grid) {
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
                                    egui::UiBuilder::new().max_rect(full).layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                    ),
                                    |ui| {
                                        ui.set_clip_rect(full);
                                        outcome =
                                            crate::edit::render_editor(ui, active, Some(full.size()));
                                    },
                                );
                                // Defer the outline to a post-table pass: the next column's
                                // selection/stripe background is painted *after* this cell and
                                // would otherwise cover the right edge, leaving only three sides.
                                out.edit_border = Some((full, valid));
                                match outcome {
                                    EditOutcome::Commit => out.commit_edit = true,
                                    EditOutcome::Cancel => out.cancel_edit = true,
                                    EditOutcome::Continue => {}
                                }
                            }
                        } else {
                            // Show the staged value if present, else the stored one.
                            let staged = edits.staged(r, c);
                            cell(ui, staged.unwrap_or(stored), staged.is_some(), emoji);
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
                    let active_here = edits
                        .active
                        .as_ref()
                        .is_some_and(|a| a.row == r && a.col == c);
                    let start_edit = !active_here
                        && (col_resp.double_clicked()
                            || (edits.active.is_some() && col_resp.clicked()));
                    if editable
                        && start_edit
                        && state != crate::edit::RowState::Deleted
                        && !matches!(stored, Value::Bytes(_))
                    {
                        if edits.col_kind(c) == EditorKind::Bool {
                            out.toggle = Some((r, c));
                        } else {
                            out.begin_edit = Some((r, c));
                        }
                    }
                }

                let row_resp = row.response();
                if row_resp.clicked() {
                    out.selected = Some(RowClick {
                        disp,
                        shift: modifiers.shift,
                        cmd: modifiers.command,
                    });
                }
                // Right-click → "Copy as …". The app copies the whole selection, or just this
                // row when it was right-clicked while unselected (TablePlus-style).
                row_resp.context_menu(|ui| {
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
            });
        });
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
        s.apply_click(RowClick { disp: 3, shift: false, cmd: false });
        assert_eq!(rows(&s), [3]);
        assert_eq!(s.lead(), Some(3));

        // Cmd/Ctrl click adds without dropping the rest, and re-anchors on the new row.
        s.apply_click(RowClick { disp: 5, shift: false, cmd: true });
        assert_eq!(rows(&s), [3, 5]);
        assert_eq!(s.lead(), Some(5));

        // Cmd/Ctrl click on a selected row removes it; lead falls back to a survivor.
        s.apply_click(RowClick { disp: 5, shift: false, cmd: true });
        assert_eq!(rows(&s), [3]);
        assert_eq!(s.lead(), Some(3));

        // Shift extends from the anchor — the last *non-Shift* click (row 5 above), Finder-style.
        s.apply_click(RowClick { disp: 6, shift: true, cmd: false });
        assert_eq!(rows(&s), [5, 6]);
        assert_eq!(s.lead(), Some(6));

        // A second Shift click re-extends from the *same* anchor (5), not the previous end (6).
        s.apply_click(RowClick { disp: 1, shift: true, cmd: false });
        assert_eq!(rows(&s), [1, 2, 3, 4, 5]);
        assert_eq!(s.lead(), Some(1));

        // A plain click collapses back to one row and re-anchors there.
        s.apply_click(RowClick { disp: 9, shift: false, cmd: false });
        assert_eq!(rows(&s), [9]);
        assert_eq!(s.lead(), Some(9));
    }

    #[test]
    fn selection_select_all_and_clamp() {
        let mut s = Selection::default();
        s.select_all(4);
        assert_eq!(rows(&s), [0, 1, 2, 3]);
        assert_eq!(s.len(), 4);

        // Shrinking the addressable range drops out-of-range rows and re-homes the lead.
        s.clamp(2);
        assert_eq!(rows(&s), [0, 1]);
        assert_eq!(s.lead(), Some(1));

        s.clamp(0);
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
                    let _ = results_grid(ui, &result, &order, None, &Selection::default(), &mut edits, false, 0, &EmojiAtlas::default());
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

        let raw = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let _ = results_grid(ui, &result, &order, None, &sel, &mut edits, true, 0, &EmojiAtlas::default());
            });
        });

        let accent = palette::ACCENT();
        // Last accent-stroked rect = the on-top border. No filled cell background may be
        // painted after it that would cover its right edge.
        let border = out.shapes.iter().enumerate().rev().find_map(|(i, cs)| match &cs.shape {
            egui::epaint::Shape::Rect(r) if r.stroke.color == accent && r.stroke.width > 0.0 => {
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
        assert!(!covered_after, "editor right border is overpainted by a later cell background");
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
                    let _ = results_grid(ui, &result, &order, None, &Selection::default(), &mut edits, true, 0, &EmojiAtlas::default());
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
fn cell(ui: &mut egui::Ui, value: &Value, staged: bool, emoji: &EmojiAtlas) -> egui::Response {
    let color = if staged {
        palette::SUCCESS()
    } else if value.is_null() {
        palette::TEXT_FAINT()
    } else {
        palette::TEXT()
    };
    // The label is deliberately non-interactive (no click sense, not selectable). The whole
    // cell is one click-sensing surface — the egui_extras cell `Ui` itself (see the row loop).
    // A click-sensing or selectable label here would be a second, text-width-only hit target
    // overlapping the cell, and double-clicks landing astride the text/empty boundary would
    // split across two widget ids and silently fail to register. One surface = reliable.
    let label = |ui: &mut egui::Ui, text: egui::RichText| {
        ui.add(egui::Label::new(text.color(color)).selectable(false))
    };
    match value {
        Value::Null => label(ui, egui::RichText::new("NULL").italics()),
        Value::Int(_) | Value::Float(_) => label(ui, egui::RichText::new(value.display()).monospace()),
        // Only free text can carry emoji, so only this path consults the atlas.
        other => {
            let text = other.display();
            if emoji::contains_emoji(&text) {
                emoji_cell(ui, &text, color, emoji)
            } else {
                label(ui, egui::RichText::new(text))
            }
        }
    }
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
