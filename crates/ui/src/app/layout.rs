//! The frame loop: `eframe::App::update` and the panel layout it drives.

use super::*;
use crate::style::palette;

impl eframe::App for DbGuiApp {
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
    /// Draw one frame into the given root ui. Split out from `eframe::App::ui` so it can be
    /// driven headlessly in tests (no `eframe::Frame` needed).
    pub(super) fn draw(&mut self, ui_root: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let ctx = ui_root.ctx().clone();
        self.poll_messages(&ctx);

        // First-run welcome page: replace the entire window until "Get Started" is clicked.
        if self.show_welcome {
            let mut actions = Vec::new();
            self.draw_welcome_page(ui_root, &mut actions);
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
            self.settings_dialog(&ctx, &mut actions);
            self.update_dialog(&ctx, &mut actions);
            self.whats_new_dialog(&ctx, &mut actions);

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
            if self.busy != Busy::Idle
                || self.update.is_busy()
                || !self.pending_page_counts.is_empty()
            {
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
            return;
        }

        // Global shortcut: Cmd/Ctrl+Enter runs the query.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::RunQuery);
        }
        // Cmd/Ctrl+S opens the SQL preview dialog for staged cell edits.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            actions.push(Action::PreviewEdits);
        }
        // Cmd/Ctrl+R reloads the current result (re-runs the tab's SQL), dropping any
        // unsaved cell edits — the reloaded result starts from a clean edit slate.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::R)) {
            actions.push(Action::RunQuery);
        }
        // Esc discards unsaved cell edits (revert to the stored values) when no cell editor
        // is open — the open-editor case is handled inside `render_editor` (cancel that
        // cell only). Skipped while the filter bar is up, which uses Esc to close itself.
        // Recorded as one undo step so an accidental discard can be taken back with Cmd/Ctrl+Z.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
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
        let typing_now = ctx.memory(|m| m.focused().is_some());
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
                        if !deleted && !bytes {
                            if tab.edits.col_kind(col) == crate::edit::EditorKind::Bool {
                                if let Some(orig) = crate::edit::original_value(result, raw, col) {
                                    tab.edits.toggle_bool(raw, col, &orig);
                                }
                            } else {
                                crate::edit::begin_cell_edit(&mut tab.edits, result, raw, col);
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
        // Cmd/Ctrl+I beautifies the active tab's SQL (TablePlus-style).
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::I)) {
            actions.push(Action::BeautifySql);
        }
        // Cmd/Ctrl+T opens a new query tab; Cmd/Ctrl+W closes the active one.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
            actions.push(Action::NewTab);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            actions.push(Action::CloseTab(self.active_query_tab));
        }
        // Cmd/Ctrl+F toggles the filter bar (when there's a result to filter); Esc hides it.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
            && self.tab().result.is_some()
        {
            let visible = self.tab().filter.visible;
            self.tab_mut().filter.visible = !visible;
        }
        if self.tab().filter.visible && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tab_mut().filter.visible = false;
        }

        // Order matters: top/bottom/left/right carve space, central takes the rest. The status
        // bar is carved first so it pins to the very bottom edge. Side panels are carved before
        // the SQL editor so they run the full height; the editor stays confined to the central
        // column. Its edge is contextual: code-first tabs dock it above the result, data-first
        // tabs keep it below the grid.
        self.top_bar(ui_root, frame, &mut actions);
        self.query_tab_bar(ui_root, &mut actions);
        self.status_bar(ui_root, &mut actions);
        if self.show_connection_tabs {
            self.connection_tabs(ui_root, &mut actions);
        }
        if self.show_schema_panel {
            self.left_panel(ui_root, &mut actions);
        }
        // History sits outermost on the right, so the details panel stays next to the grid.
        if self.history_open {
            self.history_panel(ui_root, &mut actions);
        }
        if self.show_details_panel {
            self.right_panel(ui_root);
        }
        let editor_placement = query_editor_placement(self.tab().kind);
        let saved_queries_workspace = self.show_query_console
            && self.show_saved_queries
            && self.tab().kind == crate::components::QueryTabKind::Query;
        if self.show_query_console && !saved_queries_workspace {
            self.query_console(ui_root, editor_placement, &mut actions);
        }
        if !saved_queries_workspace {
            // A top panel after left/right carves the strip directly above the grid.
            self.filter_bar(ui_root);
            // Keep result controls next to the query toolbar: below it on code-first tabs,
            // and between the grid and bottom editor on data-first tabs.
            self.view_mode_bar(ui_root, editor_placement, &mut actions);
        }
        self.central_panel(ui_root, &mut actions);
        self.connection_dialog(&ctx, &mut actions);
        self.settings_dialog(&ctx, &mut actions);
        self.commit_preview_dialog(&ctx, &mut actions);
        self.favorite_name_dialog(&ctx, &mut actions);
        self.danger_confirm_dialog(&ctx, &mut actions);
        self.import_dialog(&ctx, &mut actions);
        self.schema_preview_dialog(&ctx, &mut actions);
        self.update_dialog(&ctx, &mut actions);
        self.whats_new_dialog(&ctx, &mut actions);

        let structural = actions.iter().any(|a| {
            matches!(
                a,
                Action::NewTab
                    | Action::CloseTab(_)
                    | Action::CloseOtherTabs(_)
                    | Action::CloseTabsToRight(_)
                    | Action::CloseAllTabs
                    | Action::SelectTab(_)
                    | Action::Connect(_)
                    | Action::BindConnection(_)
                    | Action::OpenTable { .. }
                    | Action::OpenDefinition { .. }
                    | Action::FollowForeignKey { .. }
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

        // Keep animating the spinner while background work is in flight.
        if self.busy != Busy::Idle || self.update.is_busy() || !self.pending_page_counts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}
