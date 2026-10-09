//! The frame loop: `eframe::App::update` and the panel layout it drives.

use super::*;
use crate::style::{self, palette};

/// A split column never gets narrower than this.
const MIN_PANE_WIDTH: f32 = 220.0;
/// Width of the draggable strip centred on the seam between two split columns.
const SPLIT_HANDLE_WIDTH: f32 = 8.0;

/// Split `total` across panes by `ratios`, never giving one less than `min`. Panes that would
/// fall short are pinned to `min` and the rest share what remains in proportion.
pub(super) fn pane_widths(ratios: &[f32], total: f32, min: f32) -> Vec<f32> {
    let count = ratios.len();
    if count == 0 {
        return Vec::new();
    }
    if total <= min * count as f32 {
        return vec![total / count as f32; count];
    }
    let mut widths = vec![0.0_f32; count];
    let mut pinned = vec![false; count];
    loop {
        let pinned_total: f32 = (0..count).filter(|i| pinned[*i]).map(|i| widths[i]).sum();
        let free_ratio: f32 = (0..count).filter(|i| !pinned[*i]).map(|i| ratios[i]).sum();
        let mut changed = false;
        for i in 0..count {
            if pinned[i] {
                continue;
            }
            widths[i] = ratios[i] / free_ratio * (total - pinned_total);
            if widths[i] < min {
                widths[i] = min;
                pinned[i] = true;
                changed = true;
            }
        }
        if !changed {
            return widths;
        }
    }
}

impl eframe::App for DbGuiApp {
    #[cfg(target_os = "macos")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.native_menu_input(ctx, input);
    }

    // eframe 0.34 hands us a root `Ui`; panels are added with `show_inside`.
    fn ui(&mut self, ui_root: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui_root, Some(frame));
    }

    /// Match the window clear colour to the active theme so hairline panel gaps don't flash
    /// eframe's default near-black clear (reads as a thick black bar on light themes).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        crate::theme::current().base.to_normalized_gamma_f32()
    }
}

impl DbGuiApp {
    fn split_drop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        workspace: egui::Rect,
        actions: &mut Vec<Action>,
    ) {
        let count = self.pane_count();
        let can_add_pane =
            count < Self::MAX_PANES && workspace.width() / (count as f32 + 1.0) >= MIN_PANE_WIDTH;
        if !self.is_split() && !can_add_pane {
            if ui.input(|input| input.pointer.any_released()) {
                self.tab_drag = None;
            }
            return;
        }
        // Use geometry rather than a hover Response: the floating tab is a Tooltip-layer Area
        // under the pointer and must not occlude the workspace drop target on alternating frames.
        let pointer = ui.ctx().pointer_interact_pos();
        let pointer_in_workspace = pointer.is_some_and(|pointer| workspace.contains(pointer));
        let table_drag = pointer_in_workspace
            .then(|| egui::DragAndDrop::payload::<SchemaTableDrag>(ui.ctx()))
            .flatten();
        let tab_drag = (!self.is_split() && pointer_in_workspace)
            .then_some(self.tab_drag)
            .flatten();
        if table_drag.is_none() && tab_drag.is_none() {
            if ui.input(|input| input.pointer.any_released()) {
                self.tab_drag = None;
            }
            return;
        }

        // Drop zones: every existing split column takes the drop as another tab, and the right
        // edge of the last column opens a new one (the right half before any split exists).
        let columns = self.pane_columns(workspace);
        let mut zones: Vec<(usize, egui::Rect)> = Vec::new();
        if self.is_split() {
            let last = *columns.last().unwrap_or(&workspace);
            for (pane, column) in columns.iter().enumerate().skip(1) {
                let mut zone = *column;
                if pane == count - 1 && can_add_pane {
                    zone.max.x = last.right() - last.width() * 0.35;
                }
                zones.push((pane, zone));
            }
            if can_add_pane {
                let edge = egui::Rect::from_min_max(
                    egui::pos2(last.right() - last.width() * 0.35, last.top()),
                    last.max,
                );
                zones.push((count, edge));
            }
        } else {
            let half = egui::Rect::from_min_max(
                egui::pos2(workspace.center().x, workspace.top()),
                workspace.max,
            );
            zones.push((1, half));
        }
        // Several zones can sit side by side, so inset by the same half-seam the cards use:
        // neighbouring highlights then leave the usual four-point gap instead of twenty.
        let inset = if self.is_split() {
            style::WORKSPACE_GUTTER as f32
        } else {
            10.0
        };
        let zones: Vec<(usize, egui::Rect)> = zones
            .into_iter()
            .map(|(pane, zone)| (pane, zone.shrink(inset)))
            .collect();
        let hovered = pointer.and_then(|pointer| {
            zones
                .iter()
                .find(|(_, zone)| zone.contains(pointer))
                .map(|(pane, _)| *pane)
        });
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("workspace_split_drop_overlay"),
        ));
        for (pane, zone) in &zones {
            painter.rect_filled(
                *zone,
                8.0,
                palette::ACCENT().gamma_multiply(if hovered == Some(*pane) { 0.22 } else { 0.10 }),
            );
        }

        let released = ui.input(|input| input.pointer.any_released());
        let Some(pane) = hovered else {
            if released {
                self.tab_drag = None;
            }
            return;
        };
        let table_release = if released {
            egui::DragAndDrop::take_payload::<SchemaTableDrag>(ui.ctx())
        } else {
            None
        };
        if let Some(payload) = table_release {
            actions.push(Action::OpenSplitSchemaTable {
                payload: payload.as_ref().clone(),
                pane,
            });
        } else if released {
            if let Some(drag) = tab_drag {
                actions.push(Action::OpenSplitTab {
                    id: drag.id,
                    primary_id: drag.origin_active_id,
                    pane,
                });
            }
        }
        if released {
            self.tab_drag = None;
        }
    }

    /// The column rectangles of a split workspace, left to right. Shared by drawing and the
    /// drop overlay so both agree on where each pane is.
    fn pane_columns(&self, workspace: egui::Rect) -> Vec<egui::Rect> {
        let widths = pane_widths(&self.split_ratios, workspace.width(), MIN_PANE_WIDTH);
        let mut left = workspace.left();
        widths
            .into_iter()
            .map(|width| {
                let column = egui::Rect::from_min_max(
                    egui::pos2(left, workspace.top()),
                    egui::pos2(left + width, workspace.bottom()),
                );
                left += width;
                column
            })
            .collect()
    }

    fn draw_workspace_pane(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let editor_placement = query_editor_placement(self.tab().kind);
        let diagram_tab = self.tab().kind.owns_workspace();
        let designing = self.tab().schema_editor.is_some();
        let sql_authoring_tab = matches!(
            self.tab().kind,
            crate::components::QueryTabKind::Query
                | crate::components::QueryTabKind::Function
                | crate::components::QueryTabKind::Procedure
                | crate::components::QueryTabKind::Trigger
        );
        let console_visible =
            self.show_query_console && sql_authoring_tab && !diagram_tab && !designing;
        let show_view_mode_bar =
            (!console_visible || editor_placement == QueryEditorPlacement::Top || designing)
                && (self.tab().kind != crate::components::QueryTabKind::Query || !designing)
                && !diagram_tab;
        if console_visible {
            self.query_console(root, editor_placement, actions);
        }
        if !diagram_tab && !designing {
            self.batch_result_bar(root);
        }
        if !diagram_tab && !designing {
            self.filter_bar(root);
        }
        if show_view_mode_bar {
            self.view_mode_bar(root, editor_placement, actions);
        }
        self.central_panel(root, actions);
        if console_visible && editor_placement == QueryEditorPlacement::Top {
            self.query_workspace_border(root);
        }
    }

    fn draw_split_workspace(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let primary = self.active_query_tab;
        if !self.is_split() || self.split_panes.iter().any(|idx| *idx >= self.tabs.len()) {
            self.draw_workspace_pane(root, actions);
            return;
        }
        let area = root.available_rect_before_wrap();
        let columns = self.pane_columns(area);
        // Pressing anywhere in a column focuses it, so Details, Run and the shortcuts follow the
        // pane the user is working in. Applied after drawing so a text editor that still holds
        // egui focus this frame cannot claim it back.
        let pressed_in = root
            .ctx()
            .input(|input| {
                input
                    .pointer
                    .any_pressed()
                    .then(|| input.pointer.interact_pos())
            })
            .flatten()
            .and_then(|pointer| columns.iter().position(|column| column.contains(pointer)));
        for (pane, column) in columns.iter().enumerate() {
            let tab_idx = if pane == 0 {
                primary
            } else {
                self.split_panes[pane - 1]
            };
            root.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(*column)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    self.active_query_tab = tab_idx;
                    self.split_pane_tab_bar(ui, tab_idx, pane, actions);
                    self.draw_workspace_pane(ui, actions);
                },
            );
        }
        self.active_query_tab = primary;
        if let Some(pane) = pressed_in {
            self.focused_pane = pane;
        }
        root.advance_cursor_after_rect(area);

        // Dividers sit on the seam the cards already leave between columns, so a split costs
        // no more space than any other pair of neighbouring cards.
        for (divider_index, column) in columns[..columns.len() - 1].iter().enumerate() {
            let seam_x = column.right();
            let divider = egui::Rect::from_center_size(
                egui::pos2(seam_x, area.center().y),
                egui::vec2(SPLIT_HANDLE_WIDTH, area.height()),
            );
            let response = root.interact(
                divider,
                egui::Id::new(("workspace_split_divider", divider_index)),
                egui::Sense::drag(),
            );
            let grip_color = if response.hovered() || response.dragged() {
                palette::TEXT_WEAK()
            } else {
                palette::TEXT_FAINT()
            };
            for offset in [-5.0, 0.0, 5.0] {
                root.painter().circle_filled(
                    divider.center() + egui::vec2(0.0, offset),
                    1.0,
                    grip_color,
                );
            }
            if response.dragged() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    self.drag_split_divider(divider_index, pointer.x, area);
                }
            }
            if response.hovered() || response.dragged() {
                root.ctx()
                    .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        }
    }

    /// Move the seam between pane `index` and `index + 1` to `pointer_x`, trading width only
    /// between those two neighbours.
    fn drag_split_divider(&mut self, index: usize, pointer_x: f32, area: egui::Rect) {
        let total = area.width();
        let mut widths = pane_widths(&self.split_ratios, total, MIN_PANE_WIDTH);
        let left_edge = area.left() + widths[..index].iter().sum::<f32>();
        let pair = widths[index] + widths[index + 1];
        let new_left = (pointer_x - left_edge).clamp(MIN_PANE_WIDTH, pair - MIN_PANE_WIDTH);
        widths[index] = new_left;
        widths[index + 1] = pair - new_left;
        self.split_ratios = widths.iter().map(|width| width / total).collect();
        self.workspace_dirty = true;
    }

    /// Draw one frame into the given root ui. Split out from `eframe::App::ui` so it can be
    /// driven headlessly in tests (no `eframe::Frame` needed).
    pub(super) fn draw(&mut self, ui_root: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let ctx = ui_root.ctx().clone();
        self.poll_messages(&ctx);
        self.prune_query_jobs();
        self.refresh_query_busy();
        self.fill_empty_result_columns();
        self.sync_edit_rules();
        if !self.pending_quit && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending_leave.is_none() {
                self.apply_action(Action::Quit);
            }
        }
        // A confirmation (unsaved changes, discard) is a modal over the normal screen — the
        // sidebar, tabs and grid stay where they are. While it is up the keyboard belongs to
        // it alone, so nothing typed or shortcut-pressed reaches the editors underneath;
        // Escape answers it with "Cancel".
        if self.pending_leave.is_some() {
            let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            ctx.input_mut(|i| {
                i.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Copy
                            | egui::Event::Cut
                    )
                });
            });
            if escape {
                self.pending_leave = None;
            }
        }
        if self.pending_quit {
            self.maybe_save_workspace(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.show_welcome {
            self.open_anything_shortcut(&ctx);
        }

        // First-run welcome page: replace the entire window until "Get Started" is clicked.
        if self.show_welcome {
            let mut actions = Vec::new();
            self.draw_welcome_page(ui_root, &mut actions);
            for action in actions {
                self.apply_action(action);
            }
            return;
        }

        // Settings behaves like a utility tab: keep the app chrome and query-tab strip, then
        // let the settings surface own the remaining workspace until another tab is selected.
        if self.settings_open {
            let mut actions = Vec::new();
            self.top_bar(ui_root, frame, &mut actions);
            self.query_tab_bar(ui_root, &mut actions);
            self.status_bar(ui_root, &mut actions);
            self.draw_settings_page(ui_root, &mut actions);
            self.open_anything_dialog(&ctx);
            for action in actions {
                self.apply_action(action);
            }
            return;
        }

        let mut actions: Vec<Action> = Vec::new();

        // A workspace may intentionally have no tabs. Keep only the global chrome and the
        // tab strip visible; the + button (or Cmd/Ctrl+T) is the explicit entry point into a
        // query. This branch also protects the rest of the frame, which operates on an active
        // tab by design.
        if self.tabs.is_empty() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
                actions.push(Action::NewTab);
            }

            self.top_bar(ui_root, frame, &mut actions);
            self.query_tab_bar(ui_root, &mut actions);
            self.status_bar(ui_root, &mut actions);
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette::BASE()))
                .show_inside(ui_root, |_ui| {});

            // Global dialogs remain available from the title bar even before a query tab exists.
            self.connection_dialog(&ctx, &mut actions);
            self.update_dialog(&ctx, &mut actions);
            self.whats_new_dialog(&ctx, &mut actions);
            self.open_anything_dialog(&ctx);

            let structural = actions
                .iter()
                .any(|action| matches!(action, Action::NewTab | Action::DeleteConnection(_)));
            for action in actions {
                self.apply_action(action);
            }
            if let Some(text) = self.copy_buffer.take() {
                ctx.copy_text(text);
            }
            if self.pending_quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            self.maybe_save_workspace(structural);
            if self.workspace_dirty {
                ctx.request_repaint_after(std::time::Duration::from_millis(1600));
            }
            if self.busy != Busy::Idle || self.update.is_busy() {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            return;
        }

        // A New Table / View / Trigger editor is a draft that has taken over the tab. The SQL
        // behind it is not on screen, so running or "reloading" it would execute a query the
        // user cannot see — those shortcuts do nothing there.
        let draft_open = self.draft_editor_open();
        // Cmd/Ctrl+Enter runs the selection/current statement; Shift adds the whole buffer.
        if !draft_open && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(if ctx.input(|i| i.modifiers.shift) {
                Action::RunQuery
            } else {
                Action::RunCurrentQuery
            });
        }
        let editing_table_schema = self.schema_pending.is_none()
            && matches!(self.tab().view, TabView::Structure | TabView::Indexes)
            && matches!(
                self.tab().schema_editor.as_ref(),
                Some(crate::schema::ObjectEditor::Table(_))
            );
        // A New/Edit Table, View or Trigger editor owns the tab and has no Apply/Cancel buttons:
        // Cmd/Ctrl+S applies it and Esc leaves it. (The ER designer keeps its own buttons.)
        let object_editor = if self.schema_pending.is_none() {
            self.tab().schema_editor.as_ref()
        } else {
            None
        };
        let design_editor = matches!(
            object_editor,
            Some(crate::schema::ObjectEditor::Table(editor))
                if matches!(
                    editor.mode,
                    crate::schema::SchemaEditorMode::DesignNew
                        | crate::schema::SchemaEditorMode::DesignEdit
                )
        );
        let object_editor_open = object_editor.is_some() && !editing_table_schema;
        // Cmd/Ctrl+S commits whichever grid is being edited: row DML in Data, table DDL in
        // Structure/Indexes and in the object editors. Production connections still review
        // through Guardian.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            actions.push(
                if editing_table_schema || (object_editor_open && !design_editor) {
                    Action::GenerateSchema
                } else if object_editor_open {
                    Action::SaveErdTable
                } else {
                    Action::PreviewEdits
                },
            );
        }
        // Cmd/Ctrl+R reloads the current result. Structure edits need an explicit decision:
        // silently rebuilding the editor would lose pending DDL changes.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::R)) {
            let dirty_schema = self
                .tab()
                .schema_editor
                .as_ref()
                .and_then(|editor| match editor {
                    crate::schema::ObjectEditor::Table(editor) => Some(editor.has_changes()),
                    _ => None,
                })
                .unwrap_or(false);
            if editing_table_schema && dirty_schema {
                self.schema_reload_pending = Some(self.tab().id);
            } else if editing_table_schema {
                actions.push(Action::ReloadTableStructure);
            } else if draft_open {
                actions.extend(self.draft_exit_action(false));
            } else {
                actions.push(Action::RunQuery);
            }
        }
        // Esc discards unsaved cell edits (revert to the stored values) when no cell editor
        // is open — the open-editor case is handled inside `render_editor` (cancel that
        // cell only). Skipped while the filter bar is up, which uses Esc to close itself.
        // Recorded as one undo step so an accidental discard can be taken back with Cmd/Ctrl+Z.
        let typing_now = ctx.memory(|m| m.focused().is_some());
        // Esc leaves an object editor (a draft, like a dialog). In a text field the first Esc
        // only drops focus, so a half-typed name is never thrown away by a stray keypress.
        if object_editor_open
            && !design_editor
            && !typing_now
            && !egui::Popup::is_any_open(&ctx)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            actions.extend(self.draft_exit_action(true));
        }
        let discard_schema = editing_table_schema
            && !typing_now
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if discard_schema {
            actions.push(Action::DiscardSchemaChanges);
        } else if self.open_anything.is_none()
            // Esc in a text field (SQL editor, find widget) is that field's, never a discard.
            && !typing_now
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && self.tab().edits.active.is_none()
            && self.tab().edits.has_pending()
            && !self.tab().filter.visible
        {
            self.tab_mut().edits.discard_all();
            self.tab_mut().recompute_view();
            self.status_msg = "Discarded unsaved edits (⌘Z to undo)".to_string();
            self.error = None;
            self.workspace_dirty = true;
        }
        // Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z redoes, the last staged-edit change (cell edit,
        // delete mark, new row, fill, paste, discard). Only when no text field is focused —
        // an open cell editor / SQL console handles its own in-field undo. Shift+Z is matched
        // first so a redo isn't also read as an undo.
        if !typing_now && self.tab().edits.editable() {
            let (undo, redo) = ctx.input_mut(|i| {
                let redo = i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                );
                let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                (undo, redo)
            });
            if redo {
                actions.push(Action::Redo);
            } else if undo {
                actions.push(Action::Undo);
            }
        }
        // Backspace/Delete on the selected rows (when nothing is being typed) marks every
        // selected stored row for deletion (red) and drops any selected pending new rows.
        // `focused()` is `Some` while any text field — a cell editor, the SQL console, the
        // field filter — has focus, so this never steals a real backspace keystroke.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing
            && self.tab().edits.editable()
            && self.tab().edits.active.is_none()
            && self.tab().view == TabView::Data
            && self.tab().schema_editor.is_none()
            && ctx
                .input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete))
            && !self.tab().selection.is_empty()
        {
            let order_len = self.tab().row_order.len();
            let selected: Vec<usize> = self.tab().selection.iter().collect();
            // One undo group so the whole multi-row delete takes a single Cmd/Ctrl+Z.
            self.tab_mut().edits.begin_undo_group();
            // Mark stored rows for deletion. New (insert) rows are removed instead, highest
            // display index first so the renumbering of the rows above each removal never
            // invalidates an index we still have to process.
            for &disp in &selected {
                if disp < order_len {
                    let raw = self.tab().row_order[disp];
                    self.tab_mut().edits.toggle_delete(raw);
                }
            }
            let mut removed_new = false;
            for &disp in selected.iter().rev() {
                if disp >= order_len {
                    let new_id = crate::edit::NEW_ROW_BASE + (disp - order_len);
                    self.tab_mut().edits.remove_new_row(new_id);
                    removed_new = true;
                }
            }
            self.tab_mut().edits.end_undo_group();
            // Removing new rows shifts the trailing display indices; clear the selection so it
            // can't point at the wrong (renumbered) rows. Stored-only deletes keep their
            // selection so the marked rows stay highlighted.
            if removed_new {
                self.tab_mut().selection.clear();
            }
        }
        // Arrow keys drive the grid's cell cursor, spreadsheet-style, when nothing is being
        // typed: ↑/↓ move rows (Shift extends the selection from the anchor), ←/→ move
        // columns. Enter or F2 opens the editor on the cursor cell (Enter toggles booleans
        // in place). All keys are *consumed* so nothing else — in particular the freshly
        // opened editor, which would otherwise see this very Enter press later in the same
        // frame and instantly commit itself — reacts to them.
        if !typing
            && self.tab().result.is_some()
            && self.tab().edits.active.is_none()
            && self.tab().view == TabView::Data
            && self.tab().schema_editor.is_none()
        {
            let (mut dr, mut dc, mut extend) = (0isize, 0isize, false);
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) {
                    dr += 1;
                    extend = true;
                }
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) {
                    dr -= 1;
                    extend = true;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    dr += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    dr -= 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    dc -= 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    dc += 1;
                }
            });
            if dr != 0 || dc != 0 {
                let tab = self.tab_mut();
                let len = tab.row_order.len() + tab.edits.new_rows;
                let ncols = tab.result.as_ref().map_or(0, |r| r.column_count());
                if tab.selection.move_cursor(dr, dc, len, ncols, extend) {
                    tab.pending_scroll = tab.selection.cursor().map(|(r, _)| r);
                }
            }
            let open_editor = ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::F2)
            });
            if open_editor && self.tab().edits.editable() {
                let tab = self.tab_mut();
                if let (Some((disp, col)), Some(result)) =
                    (tab.selection.cursor(), tab.result.as_ref())
                {
                    if let Some(raw) =
                        crate::edit::disp_to_raw(&tab.row_order, tab.edits.new_rows, disp)
                    {
                        let deleted = tab.edits.row_state(raw) == crate::edit::RowState::Deleted;
                        let bytes = crate::edit::original_value(result, raw, col)
                            .is_some_and(|v| matches!(v, dbcore::Value::Bytes(_)));
                        // Inside a multi-row selection the edit applies to every selected row.
                        let fan_out = crate::edit::selection_fan_out(
                            &tab.selection,
                            &tab.row_order,
                            tab.edits.new_rows,
                            result,
                            disp,
                            col,
                        );
                        if !deleted && !bytes {
                            if tab.edits.col_kind(col) == crate::edit::EditorKind::Bool {
                                if let Some(orig) = crate::edit::original_value(result, raw, col) {
                                    // Every selected row takes the cursor cell's flipped value.
                                    tab.edits.begin_undo_group();
                                    tab.edits.toggle_bool(raw, col, &orig);
                                    let flipped =
                                        tab.edits.staged(raw, col).cloned().unwrap_or(orig);
                                    for (row, orig) in &fan_out {
                                        if tab.edits.row_state(*row)
                                            != crate::edit::RowState::Deleted
                                        {
                                            tab.edits.stage(*row, col, flipped.clone(), orig);
                                        }
                                    }
                                    tab.edits.end_undo_group();
                                }
                            } else {
                                crate::edit::begin_cell_edit(&mut tab.edits, result, raw, col);
                                tab.edits.set_fan_out(fan_out);
                            }
                        }
                    }
                }
            }
            // Type-to-edit: printable text on the cursor cell opens its editor with the typed
            // text *replacing* the value, spreadsheet-style. The Text events are removed so
            // the editor, which takes focus this same frame, doesn't insert them twice.
            let typed = ctx.input(|i| {
                if i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd {
                    return None;
                }
                let text: String = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
            });
            if let Some(text) = typed.filter(|_| {
                self.tab().edits.editable()
                    && self.tab().edits.active.is_none()
                    && self.open_anything.is_none()
                    && self.commit_pending.is_none()
            }) {
                let tab = self.tab_mut();
                if let (Some((disp, col)), Some(result)) =
                    (tab.selection.cursor(), tab.result.as_ref())
                {
                    if let Some(raw) =
                        crate::edit::disp_to_raw(&tab.row_order, tab.edits.new_rows, disp)
                    {
                        let deleted = tab.edits.row_state(raw) == crate::edit::RowState::Deleted;
                        let bytes = crate::edit::original_value(result, raw, col)
                            .is_some_and(|v| matches!(v, dbcore::Value::Bytes(_)));
                        let bool_col = tab.edits.col_kind(col) == crate::edit::EditorKind::Bool;
                        if !deleted && !bytes && !bool_col {
                            let fan_out = crate::edit::selection_fan_out(
                                &tab.selection,
                                &tab.row_order,
                                tab.edits.new_rows,
                                result,
                                disp,
                                col,
                            );
                            crate::edit::begin_cell_edit(&mut tab.edits, result, raw, col);
                            tab.edits.set_fan_out(fan_out);
                            if let Some(active) = tab.edits.active.as_mut() {
                                // An enum is picked from its list; typed text can't seed it.
                                if !active.is_enum() {
                                    active.buf = text;
                                }
                                ctx.input_mut(|i| {
                                    i.events.retain(|e| !matches!(e, egui::Event::Text(_)))
                                });
                            }
                        }
                    }
                }
            }
        }
        // Cmd/Ctrl+A selects every row in the grid — but only when not typing, so it keeps
        // its native "select all text" meaning inside the SQL console or any field editor.
        if !typing
            && self.tab().result.is_some()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A))
        {
            let len = self.tab().row_order.len() + self.tab().edits.new_rows;
            self.tab_mut().selection.select_all(len);
        }
        // Cmd/Ctrl+C copies the selected rows as TSV (spreadsheet-native, and what paste reads
        // back). The OS turns the copy shortcut into an `Event::Copy` (a raw `Key::C` press
        // never arrives for it on macOS), so match the event — and only when not typing, so a
        // focused text field keeps its native copy.
        if !typing
            && !self.tab().selection.is_empty()
            && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
        {
            actions.push(Action::CopyRows(dbcore::CopyFormat::Tsv));
        }
        // Cmd/Ctrl+V pastes clipboard rows (TSV) as new insert rows in an editable table. Paste
        // also arrives as an `Event::Paste(text)`; `!typing` lets a focused cell/field paste
        // its text natively instead.
        if !typing {
            let pasted = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
            });
            if let Some(text) = pasted {
                actions.push(Action::PasteRows(text));
            }
        }
        // Cmd/Ctrl+D duplicates the selected rows as new insert rows. Consumed here, before
        // the SQL editor renders, so an unfocused editor doesn't read it as "add next cursor".
        if !typing
            && self.tab().edits.editable()
            && self.tab().view == TabView::Data
            && !self.tab().selection.is_empty()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D))
        {
            actions.push(Action::DuplicateRows);
        }
        // Cmd/Ctrl+I beautifies the active tab's SQL (TablePlus-style).
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::I)) {
            actions.push(Action::BeautifySql);
        }
        // Cmd/Ctrl+T opens a new query tab; Cmd/Ctrl+W closes the active one.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
            if self.is_split() {
                actions.push(Action::NewSplitPaneTab(self.focused_pane));
            } else {
                actions.push(Action::NewTab);
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            if self.is_split() {
                actions.push(Action::CloseSplitPaneTab {
                    idx: self.focused_tab_idx(),
                    pane: self.focused_pane,
                });
            } else {
                actions.push(Action::CloseTab(self.active_query_tab));
            }
        }
        // Cmd/Ctrl+F belongs to the SQL editor while text has focus; outside the editor it
        // keeps the existing result-filter shortcut.
        let sql_editor_tab = self.tab_has_sql_editor();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            if typing && sql_editor_tab {
                self.open_find(&ctx, false);
            } else if self.tab().result.is_some() {
                actions.push(Action::ToggleFilter(self.tab().id));
            }
        }
        if typing
            && sql_editor_tab
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::H))
        {
            self.open_find(&ctx, true);
        }
        if self.open_anything.is_none()
            && self.tab().filter.visible
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.tab_mut().filter.visible = false;
        }
        // Back after following a foreign key: Cmd/Ctrl+[ outside text fields (where it
        // outdents), or the mouse's back button.
        if !self.tab().nav_back.is_empty() {
            let in_text = ctx.memory(|m| m.focused().is_some());
            let back = ctx.input(|i| {
                (!in_text && i.modifiers.command && i.key_pressed(egui::Key::OpenBracket))
                    || i.pointer.button_pressed(egui::PointerButton::Extra1)
            });
            if back {
                actions.push(Action::NavigateBack);
            }
        }

        // Order matters: top/bottom/left/right carve space, central takes the rest. The status
        // bar is carved first so it pins to the very bottom edge. Side panels are carved before
        // the SQL editor so they run the full height; the editor stays confined to the central
        // column. Table/View tabs are data workspaces, so SQL authoring stays in Query tabs.
        self.top_bar(ui_root, frame, &mut actions);
        if !self.is_split() {
            self.query_tab_bar(ui_root, &mut actions);
        }
        self.status_bar(ui_root, &mut actions);
        // Paint the shared seam colour once behind the docks. Panel outer margins are transparent
        // by design, so this keeps the gap visibly darker than either adjacent surface.
        let workspace_rect = ui_root.available_rect_before_wrap();
        ui_root.painter().rect_filled(
            workspace_rect,
            egui::CornerRadius::ZERO,
            style::workspace_gap(),
        );
        // Give the tabs the same four-point seam as adjacent cards: two points from the
        // workspace inset and two from each card's own top margin.
        let gutter = style::WORKSPACE_GUTTER as f32;
        // macOS draws its one-point window rim over the content's outer edge, eating into the
        // left and right gutters. Add it back so the side seams match the seams between docks.
        let rim = if cfg!(target_os = "macos") { 1.0 } else { 0.0 };
        let side = egui::vec2(gutter + rim, gutter);
        let dock_rect =
            egui::Rect::from_min_max(workspace_rect.min + side, workspace_rect.max - side);
        let mut workspace_root = ui_root.new_child(
            egui::UiBuilder::new()
                .id_salt("workspace_docks")
                .max_rect(dock_rect),
        );
        if self.show_connection_tabs {
            self.connection_tabs(&mut workspace_root, &mut actions);
        }
        if self.show_schema_panel {
            self.left_panel(&mut workspace_root, &mut actions);
        }
        if self.show_details_panel != self.details_flag_seen {
            self.details_flag_seen = self.show_details_panel;
            self.details_dismissed = None;
        }
        if self.show_details_panel {
            self.right_panel(&mut workspace_root, &mut actions);
        }
        let workspace_drop_rect = workspace_root.available_rect_before_wrap();
        if self.is_split() {
            self.draw_split_workspace(&mut workspace_root, &mut actions);
            if self.pane_count() >= 3 && self.show_details_panel {
                let columns = self.pane_columns(workspace_drop_rect);
                if let Some(column) = columns.get(self.focused_pane) {
                    self.details_drawer(&ctx, *column, &mut actions);
                }
            }
        } else {
            let editor_placement = query_editor_placement(self.tab().kind);
            // A Diagram tab is just the canvas: no SQL editor, no filter or result-mode bars.
            let diagram_tab = self.tab().kind.owns_workspace();
            // An open object designer (Create/Edit Table, View, Trigger, Routine) owns the whole
            // tab the same way: the SQL console and result bars would only crowd the form.
            let designing = self.tab().schema_editor.is_some();
            let sql_authoring_tab = matches!(
                self.tab().kind,
                crate::components::QueryTabKind::Query
                    | crate::components::QueryTabKind::Function
                    | crate::components::QueryTabKind::Procedure
                    | crate::components::QueryTabKind::Trigger
            );
            let console_visible =
                self.show_query_console && sql_authoring_tab && !diagram_tab && !designing;
            let show_view_mode_bar =
                (!console_visible || editor_placement == QueryEditorPlacement::Top || designing)
                    && (self.tab().kind != crate::components::QueryTabKind::Query || !designing)
                    && !diagram_tab;
            if console_visible {
                self.query_console(&mut workspace_root, editor_placement, &mut actions);
            }
            // Live log is a workspace-level bottom dock, not part of the SQL editor. Keeping it
            // independent makes it stay put across Data / Structure / Indexes and places it below
            // query results instead of between the editor and its toolbar.
            if self.show_live_log && !diagram_tab {
                self.live_log_panel(&mut workspace_root, self.tab().id);
            }
            if !diagram_tab && !designing {
                self.batch_result_bar(&mut workspace_root);
                // A top panel after left/right carves the strip directly above the grid.
                self.filter_bar(&mut workspace_root);
            }
            // Carve the result-mode dock after Live log so it sits above the log's resize edge.
            if show_view_mode_bar {
                self.view_mode_bar(&mut workspace_root, editor_placement, &mut actions);
            }
            self.central_panel(&mut workspace_root, &mut actions);
            if console_visible && editor_placement == QueryEditorPlacement::Top {
                self.query_workspace_border(&workspace_root);
            }
        }
        self.split_drop_overlay(&mut workspace_root, workspace_drop_rect, &mut actions);
        self.connection_dialog(&ctx, &mut actions);
        self.schema_reload_dialog(&ctx, &mut actions);
        self.foreign_key_dialog(&ctx, &mut actions);
        self.commit_preview_dialog(&ctx, &mut actions);
        self.key_chooser_dialog(&ctx, &mut actions);
        self.favorite_name_dialog(&ctx, &mut actions);
        self.favorite_folder_dialog(&ctx, &mut actions);
        self.danger_confirm_dialog(&ctx, &mut actions);
        self.import_dialog(&ctx, &mut actions);
        self.backup_dialog(&ctx, &mut actions);
        self.unsaved_changes_dialog(&ctx, &mut actions);
        if self
            .value_viewer
            .as_ref()
            .is_some_and(|viewer| viewer.show(&ctx))
        {
            self.value_viewer = None;
        }
        self.update_dialog(&ctx, &mut actions);
        self.whats_new_dialog(&ctx, &mut actions);
        self.open_anything_dialog(&ctx);

        let structural = actions.iter().any(|a| {
            matches!(
                a,
                Action::NewTab
                    | Action::CloseTab(_)
                    | Action::CloseOtherTabs(_)
                    | Action::CloseTabsToRight(_)
                    | Action::CloseAllTabs
                    | Action::NewSplitPaneTab(_)
                    | Action::SelectSplitPaneTab { .. }
                    | Action::CloseSplitPaneTab { .. }
                    | Action::SelectTab(_)
                    | Action::Connect(_)
                    | Action::BindConnection(_)
                    | Action::OpenTable { .. }
                    | Action::OpenDefinition { .. }
                    | Action::FollowForeignKey { .. }
                    | Action::NavigateBack
                    | Action::DeleteConnection(_)
            )
        });
        for action in actions {
            self.apply_action(action);
        }

        // Flush any text an action staged for the clipboard (e.g. copied result rows) now that
        // the egui Context is in hand.
        if let Some(text) = self.copy_buffer.take() {
            ctx.copy_text(text);
        }

        if self.pending_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Persist the workspace: immediately after structural changes, otherwise on a throttle
        // (so typing SQL into a tab is eventually saved without writing every frame).
        self.maybe_save_workspace(structural);
        if self.workspace_dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(1600));
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        self.schedule_update_check(&ctx);

        // Keep background progress and incoming query rows responsive.
        if self.busy != Busy::Idle || self.update.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.trim_memory_when_idle(&ctx);
    }
}

impl DbGuiApp {
    /// Whether the active tab is showing an unsaved New/Edit View, Trigger, Routine or New
    /// Table editor (as opposed to an existing table's Structure view, which reloads from the
    /// database, or the ER designer).
    pub(super) fn draft_editor_open(&self) -> bool {
        use crate::schema::{ObjectEditor, SchemaEditorMode};
        self.schema_pending.is_none()
            && match self.tab().schema_editor.as_ref() {
                Some(ObjectEditor::Table(editor)) => editor.mode == SchemaEditorMode::New,
                Some(_) => true,
                None => false,
            }
    }
}
