//! Query console rendering and interaction.

use super::editor_assist::FOLD_CHEVRON_W;
use super::editor_cursors::highlight_shapes;
use crate::app::{Action, DbGuiApp, QueryEditorPlacement, QueryTab};
use crate::style;
use crate::style::palette;

impl DbGuiApp {
    /// Paint after the result surface, on the workspace layer so popovers and dialogs cover it.
    pub(in crate::app) fn query_workspace_border(&self, root: &egui::Ui) {
        let footer_id = egui::Id::new((
            "query_footer",
            self.tab().id,
            QueryEditorPlacement::Top,
            self.split_tab.is_some(),
        ));
        if let Some(panel) = egui::containers::panel::PanelState::load(root.ctx(), footer_id) {
            let rect = panel.rect;
            root.painter().hline(
                (rect.left() + style::WORKSPACE_GUTTER as f32)
                    ..=(rect.right() - style::WORKSPACE_GUTTER as f32),
                rect.bottom() - 0.5,
                egui::Stroke::new(1.0_f32, palette::BORDER()),
            );
        }
    }

    /// SQL editor with syntax highlighting and a Run button. Query/definition tabs dock it
    /// above their output; table/view tabs keep it below their data grid.
    pub(in crate::app) fn query_console(
        &mut self,
        root: &mut egui::Ui,
        placement: QueryEditorPlacement,
        actions: &mut Vec<Action>,
    ) {
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let kind = self.tabs[idx].kind;
        let available = root.available_height();
        let (contextual_default, min_size, max_ratio) = match kind {
            crate::components::QueryTabKind::Query => {
                ((available * 0.38).clamp(190.0, 420.0), 160.0, 0.65)
            }
            crate::components::QueryTabKind::Function
            | crate::components::QueryTabKind::Procedure
            | crate::components::QueryTabKind::Trigger => {
                ((available * 0.55).clamp(220.0, 520.0), 180.0, 0.75)
            }
            crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View => {
                (190.0, 96.0, 0.55)
            }
            // Diagram tabs never draw the console (`draw` skips it); inert defaults.
            crate::components::QueryTabKind::Diagram => (190.0, 96.0, 0.55),
        };
        // Always leave a useful result strip on compact windows. On larger windows the ratio
        // cap prevents either surface from swallowing the other one.
        let max_size = (available * max_ratio)
            .min((available - 80.0).max(min_size))
            .max(min_size);
        let default_size = self.tabs[idx]
            .editor_size
            .unwrap_or(contextual_default)
            .clamp(min_size, max_size);
        // On data-first tabs the result-mode bar and query actions belong to the editor's
        // bottom stack. Including them in the resizable panel puts the drag edge above the
        // whole stack instead of between its controls and the SQL editor.
        let mode_bar_height = if placement == QueryEditorPlacement::Bottom
            && matches!(
                kind,
                crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
            ) {
            38.0
        } else {
            0.0
        };
        let bottom_chrome = if placement == QueryEditorPlacement::Bottom {
            36.0 + mode_bar_height
        } else {
            0.0
        };
        let panel_min_size = min_size + bottom_chrome;
        let panel_max_size = (max_size + bottom_chrome).min((available - 80.0).max(panel_min_size));
        let panel_default_size =
            (default_size + bottom_chrome).clamp(panel_min_size, panel_max_size);
        // Use a distinct egui panel identity while the whole workspace is split. This prevents
        // a remembered single-pane splitter position from making the two columns start at
        // different heights.
        let split_layout = self.split_tab.is_some();
        let panel_id = egui::Id::new(("query_console", tab_id, placement, split_layout));
        let footer_id = egui::Id::new(("query_footer", tab_id, placement, split_layout));
        let footer = |app: &mut Self,
                      root: &mut egui::Ui,
                      dock: QueryEditorPlacement,
                      actions: &mut Vec<Action>| {
            let panel = match dock {
                QueryEditorPlacement::Top => egui::Panel::top(footer_id),
                QueryEditorPlacement::Bottom => egui::Panel::bottom(footer_id),
            };
            // On code-first tabs this bar is the header of the result surface. The editor's
            // bottom margin forms the two-point resize gutter above it.
            let (height, frame) = if placement == QueryEditorPlacement::Top {
                (
                    40.0,
                    egui::Frame::new()
                        .fill(palette::PANEL())
                        .corner_radius(egui::CornerRadius {
                            nw: style::radius::LG,
                            ne: style::radius::LG,
                            sw: 0,
                            se: 0,
                        })
                        .outer_margin(egui::Margin {
                            left: style::WORKSPACE_GUTTER,
                            right: style::WORKSPACE_GUTTER,
                            top: 0,
                            bottom: 0,
                        })
                        .inner_margin(egui::Margin::symmetric(8, 0)),
                )
            } else {
                (
                    36.0,
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 0)),
                )
            };
            panel
                .exact_size(height)
                .frame(frame)
                .show_separator_line(false)
                .show_inside(root, |ui| app.query_workspace_bar(ui, actions));
        };

        let panel = match placement {
            QueryEditorPlacement::Top => egui::Panel::top(panel_id),
            QueryEditorPlacement::Bottom => egui::Panel::bottom(panel_id),
        };
        let editor_frame = if placement == QueryEditorPlacement::Top {
            // The bottom stroke would become a full-width rule immediately above the
            // narrow resize gutter. Preserve the editor's content inset without it.
            style::workspace_frame(palette::CODE_BG())
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::same(5))
        } else {
            style::workspace_frame(palette::CODE_BG())
        };
        let response = panel
            .resizable(true)
            .default_size(panel_default_size)
            .min_size(panel_min_size)
            .max_size(panel_max_size)
            .frame(editor_frame)
            .show_separator_line(false)
            .show_inside(root, |ui| {
                if placement == QueryEditorPlacement::Bottom {
                    if mode_bar_height > 0.0 {
                        self.view_mode_bar(ui, QueryEditorPlacement::Top, false, actions);
                    }
                    footer(self, ui, QueryEditorPlacement::Top, actions);
                }
                self.query_parameter_panel(ui);
                self.sql_editor_body(ui, idx);
            });

        // Code-first tabs keep their action row directly below the top-docked editor. Data-first
        // tabs rendered it inside the resizable stack above, together with the result-mode bar.
        if placement == QueryEditorPlacement::Top {
            footer(self, root, QueryEditorPlacement::Top, actions);
            // Panel::top leaves a two-point layout gap before CentralPanel. Close it so the
            // SQL workspace bar and the result surface paint as one uninterrupted panel.
            root.add_space(-(style::WORKSPACE_GUTTER_Y as f32));
            if let Some(handle) = root.ctx().read_response(panel_id.with("__resize")) {
                let edge = handle.rect.center().y;
                // egui paints a hover/drag separator inside the editor even when its
                // separator is disabled. Cover only that line with the editor fill;
                // the next two points remain the actual dark resize gutter.
                root.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(handle.rect.left(), edge - 4.0),
                        egui::pos2(handle.rect.right(), edge - 2.0),
                    ),
                    0.0,
                    palette::CODE_BG(),
                );
                root.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(handle.rect.left(), edge - 2.0),
                        egui::pos2(handle.rect.right(), edge),
                    ),
                    0.0,
                    style::workspace_gap(),
                );
                let dot_color = if handle.hovered() || handle.dragged() {
                    palette::TEXT_WEAK()
                } else {
                    palette::TEXT_FAINT()
                };
                for offset in [-5.0, 0.0, 5.0] {
                    root.painter().circle_filled(
                        egui::pos2(handle.rect.center().x + offset, edge - 1.0),
                        1.0,
                        dot_color,
                    );
                }
            }
        } else {
            style::workspace_resize_grip(root, panel_id, true);
        }

        let rendered_size = response.response.rect.height() - bottom_chrome;
        let splitter_dragged = root
            .ctx()
            .read_response(panel_id.with("__resize"))
            .is_some_and(|r| r.dragged());
        match self.tabs[idx].editor_size {
            None => self.tabs[idx].editor_size = Some(rendered_size),
            Some(previous) if splitter_dragged && (previous - rendered_size).abs() > 0.5 => {
                self.tabs[idx].editor_size = Some(rendered_size);
                self.workspace_dirty = true;
            }
            Some(_) => {}
        }
    }

    /// The tab's SQL editor — gutter with folds, highlighting, find, split pane, multi-cursor,
    /// autocomplete, ghost text, hover and diagnostics — filling `ui`. Shared by the query
    /// console and by the New View draft, whose defining `SELECT` lives in the tab's `sql`.
    pub(in crate::app) fn sql_editor_body(&mut self, ui: &mut egui::Ui, idx: usize) {
        let tab_id = self.tabs[idx].id;
        let wrap_lines = self.editor_wrap_lines;
        let mut font = egui::TextStyle::Monospace.resolve(ui.style());
        font.size = self.editor_font_size;
        let editor_id = egui::Id::new(("sql_editor", tab_id, "primary"));

        if self.tabs[idx].find.open {
            self.editor_find_bar(ui, editor_id);
        }

        if self.tabs[idx].editor_split && self.split_tab.is_none() {
            let split_id = egui::Id::new(("sql_editor_split", tab_id));
            let available_width = ui.available_width();
            let default_width = self.tabs[idx]
                .editor_split_size
                .unwrap_or(available_width * 0.5)
                .clamp(220.0, (available_width - 220.0).max(220.0));
            let split = egui::Panel::right(split_id)
                .resizable(true)
                .default_size(default_width)
                .min_size(220.0)
                .max_size((available_width - 220.0).max(220.0))
                .show_inside(ui, |ui| self.split_sql_editor(ui, &font));
            style::workspace_resize_grip(ui, split_id, false);
            let width = split.response.rect.width();
            let dragged = ui
                .ctx()
                .read_response(split_id.with("__resize"))
                .is_some_and(|response| response.dragged());
            if self.tabs[idx].editor_split_size.is_none() || dragged {
                self.tabs[idx].editor_split_size = Some(width);
                if dragged {
                    self.workspace_dirty = true;
                }
            }
        }

        // Autocomplete: while the popup is open, steal its navigation keys before the
        // editor renders so arrows/Enter/Tab drive the suggestion list instead of the
        // text cursor. Ctrl+Space force-opens it (also when the prefix is empty).
        let mut nav = crate::autocomplete::NavKeys::default();
        if self.tabs[idx].editor_assist.autocomplete.open {
            ui.input_mut(|i| {
                nav.down |= i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                nav.up |= i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                nav.accept |= i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                nav.accept |= i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                nav.dismiss |= i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
        let force = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));

        // Cmd/Ctrl+/ toggles line comments over the selection, like VS Code. Consumed
        // here — before the editor — so the keystroke never lands as a literal '/'; the
        // edit itself is applied after the render, once we have the live cursor range.
        let toggle_comment =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash));

        // Ghost text (fish-shell autosuggestion): when the popup is closed and a
        // suggestion was trailing the caret last frame, Tab accepts it. Stolen here,
        // before the editor, so the keystroke drives the suggestion, not a literal tab.
        let editor_focused_before = ui
            .ctx()
            .memory(|memory| memory.focused() == Some(editor_id));
        let plain_tab = editor_focused_before
            && !self.tabs[idx].editor_assist.autocomplete.open
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        let snippet_handled = plain_tab
            && (self.advance_snippet_placeholder(ui.ctx(), editor_id)
                || self.expand_snippet_at_caret(ui.ctx(), editor_id));
        let accept_ghost = self.ghost_suggestions_enabled
            && plain_tab
            && !snippet_handled
            && self.tabs[idx].editor_assist.ghost_suggestion.is_some();

        // Fill the panel's height instead of shrinking to the text: otherwise a long
        // query would grow the scroll area and push the whole panel taller, fighting
        // the size the user dragged it to. With `auto_shrink` off the editor keeps the
        // panel's height and scrolls its content internally.
        egui::Frame::new()
            .fill(palette::CODE_BG())
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt("sql_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        // Size the editor to fill the panel: a `TextEdit` only grows to its
                        // `desired_rows` (or its content), so a fixed row count would leave the
                        // dragged-open panel mostly empty. Derive the row count from the space the
                        // scroll area gives us so the box tracks the resize; content longer than
                        // that scrolls internally.
                        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
                        // Leave room for the editor's own vertical margin so the widget doesn't
                        // overflow the viewport by a few pixels and trigger a permanent scrollbar.
                        let avail = ui.available_height();
                        let rows = (avail / row_height).floor().max(5.0) as usize;

                        // Fold parsing and text layout are cached per tab. Taking the cache
                        // out temporarily lets the editor mutate `sql` while the derived view
                        // stays borrowed locally; it goes back before this frame ends.
                        let idx = self.active_query_tab;
                        let mut editor_cache = std::mem::take(&mut self.tabs[idx].sql_editor_cache);
                        let QueryTab {
                            sql_revision,
                            sql,
                            folds,
                            ..
                        } = &mut self.tabs[idx];
                        editor_cache.refresh(*sql_revision, sql, folds);
                        let color_key = crate::highlight::sql_colors();
                        let view_text = editor_cache.view.text.as_str();
                        let markers = editor_cache.markers.as_slice();
                        let layout_cache = &mut editor_cache.layout;
                        let mut layouter =
                            |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                                let cacheable = buf.as_str() == view_text;
                                if cacheable {
                                    if let Some(cached) = layout_cache.as_ref().filter(|c| {
                                        c.colors == color_key
                                            && c.font == font
                                            && c.wrap_width_bits == wrap_width.to_bits()
                                    }) {
                                        return cached.galley.clone();
                                    }
                                }
                                let mut job = crate::highlight::highlight_sql_folded(
                                    buf.as_str(),
                                    font.clone(),
                                    markers,
                                );
                                job.wrap.max_width = if wrap_lines {
                                    wrap_width
                                } else {
                                    f32::INFINITY
                                };
                                let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
                                if cacheable {
                                    *layout_cache = Some(crate::app::SqlEditorLayoutCache {
                                        colors: color_key,
                                        font: font.clone(),
                                        wrap_width_bits: wrap_width.to_bits(),
                                        galley: galley.clone(),
                                    });
                                }
                                galley
                            };

                        let line_count = editor_cache.view.source_lines.len();
                        let digits = self.tabs[idx].sql.lines().count().max(1).to_string().len();
                        let digit_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
                        let gutter_width = digits as f32 * digit_width + 14.0 + FOLD_CHEVRON_W;
                        let multi_changed = self.apply_multi_cursor_input(ui.ctx(), editor_id);
                        if !self.tabs[idx].editor_assist.autocomplete.open
                            && ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::COMMAND, egui::Key::D)
                            })
                        {
                            self.add_next_cursor(ui.ctx(), editor_id);
                        }
                        let previous_primary = self.tabs[idx].primary_cursor.clone();
                        let previous_sql_chars = self.tabs[idx].sql.chars().count();
                        // `.show()` (not `ui.add`) exposes the galley + cursor so the popup can
                        // anchor under the caret and we can move the caret after an insertion.
                        let (gutter_rect, output, shifts, refused_undo, under_text) = ui
                            .horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let gutter_height = rows.max(line_count) as f32 * row_height;
                                let (gutter_rect, gutter_resp) = ui.allocate_exact_size(
                                    egui::vec2(gutter_width, gutter_height),
                                    egui::Sense::hover(),
                                );
                                gutter_resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Label,
                                        true,
                                        "SQL line numbers",
                                    )
                                });

                                // The editor edits the folded view; the buffer maps every
                                // keystroke back onto the tab's real SQL.
                                let mut buffer = crate::fold::Buffer::new(
                                    &mut self.tabs[idx].sql,
                                    &editor_cache.view,
                                );
                                // A slot painted before the text: find matches and extra
                                // cursors' selections go here, under the glyphs like the
                                // editor's own selection, instead of covering them.
                                let under_text = ui.painter().add(egui::Shape::Noop);
                                let output = egui::TextEdit::multiline(&mut buffer)
                                    .id(editor_id)
                                    .code_editor()
                                    .frame(egui::Frame::NONE)
                                    .margin(egui::Margin::ZERO)
                                    .desired_rows(rows)
                                    .desired_width(f32::INFINITY)
                                    .layouter(&mut layouter)
                                    .show(ui);
                                let (shifts, refused_undo) = buffer.finish();
                                (gutter_rect, output, shifts, refused_undo, under_text)
                            })
                            .inner;

                        // Keep the collapsed regions over the same text after an edit. An
                        // undo that could not be mapped back opens every fold instead, so
                        // pressing it again undoes against the whole query.
                        for (at, delta) in shifts {
                            self.tabs[idx].folds = self.tabs[idx]
                                .folds
                                .iter()
                                .map(|anchor| {
                                    if *anchor > at {
                                        anchor.saturating_add_signed(delta)
                                    } else {
                                        *anchor
                                    }
                                })
                                .collect();
                        }
                        if refused_undo {
                            self.tabs[idx].folds.clear();
                        }

                        // Line numbers and fold chevrons, painted after the editor so they
                        // can follow the galley's own rows (and stay aligned when a long
                        // line wraps).
                        self.fold_gutter(
                            ui,
                            gutter_rect,
                            &editor_cache.view,
                            &editor_cache.regions,
                            &output.galley,
                            output.galley_pos,
                            &font,
                            row_height,
                        );

                        let resp = &output.response.response;
                        let focused = resp.has_focus();
                        if focused {
                            self.tabs[idx].editor_pane = crate::app::EditorPane::Primary;
                            self.split_focus = self.split_tab == Some(idx);
                        }
                        let text_changed = resp.changed() || multi_changed;

                        // Clicking the `⋯ N lines` stand-in opens what it hides — the
                        // caret lands inside the marker, which is answer enough.
                        if resp.clicked() {
                            let hidden = output
                                .cursor_range
                                .and_then(|r| editor_cache.view.marker_hiding(r.primary.index));
                            if let Some(at) = hidden {
                                if let Some(region) =
                                    editor_cache.regions.iter().find(|r| r.hide.start == at)
                                {
                                    self.tabs[idx].folds.remove(&region.anchor);
                                }
                            }
                        }
                        if text_changed {
                            // Editing the SQL means the rows currently on screen may no longer
                            // map back to one table, so they turn read-only; the next Run
                            // re-derives editability from the new SQL (`derive_edit_source`).
                            // A previewed tab becomes permanent (just like other editors).
                            let tab = self.tab_mut();
                            tab.edits.source = None;
                            tab.preview = false;
                            if !multi_changed {
                                tab.extra_cursors.clear();
                            }
                            tab.mark_sql_changed();
                            self.workspace_dirty = true;
                        }

                        // Caret position (char index + on-screen rect) drives the popup.
                        // Everything downstream — completion, ghost text, diagnostics —
                        // reasons about the real SQL, so the caret is mapped out of the
                        // folded view first. The rect stays on screen, where it belongs.
                        let cursor = output.cursor_range.map(|r| r.primary);
                        let cursor_char = cursor.map(|c| editor_cache.view.to_source(c.index));
                        let cursor_rect = cursor.map(|c| {
                            output
                                .galley
                                .pos_from_cursor(c)
                                .translate(output.galley_pos.to_vec2())
                        });
                        let mut under: Vec<egui::Shape> = Vec::new();
                        if self.tabs[idx].find.open {
                            Self::paint_find_matches(
                                ui,
                                &mut under,
                                &mut self.tabs[idx].find,
                                &editor_cache.view,
                                &output.galley,
                                output.galley_pos,
                            );
                        }
                        if let Some(caret) = cursor_char {
                            Self::paint_matching_brackets(
                                ui,
                                &self.tabs[idx].sql,
                                caret,
                                &editor_cache.view,
                                &output.galley,
                                output.galley_pos,
                            );
                        }

                        let source_range = output.cursor_range.map(|range| {
                            let sorted = range.as_sorted_char_range();
                            editor_cache.view.to_source(sorted.start)
                                ..editor_cache.view.to_source_end(sorted.end)
                        });
                        if text_changed && !self.tabs[idx].snippet_placeholders.is_empty() {
                            let active = self.tabs[idx].snippet_placeholder;
                            let placeholder =
                                self.tabs[idx].snippet_placeholders.get(active).cloned();
                            if let (Some(placeholder), Some(cursor)) =
                                (placeholder, source_range.as_ref())
                            {
                                if previous_primary.start >= placeholder.start
                                    && previous_primary.end <= placeholder.end
                                {
                                    let new_len = self.tabs[idx].sql.chars().count();
                                    let delta = new_len as isize - previous_sql_chars as isize;
                                    self.tabs[idx].snippet_placeholders[active].end =
                                        cursor.end.max(placeholder.start);
                                    for later in self.tabs[idx]
                                        .snippet_placeholders
                                        .iter_mut()
                                        .skip(active + 1)
                                    {
                                        later.start = later.start.saturating_add_signed(delta);
                                        later.end = later.end.saturating_add_signed(delta);
                                    }
                                } else {
                                    self.tabs[idx].snippet_placeholders.clear();
                                }
                            }
                        }
                        if focused || resp.clicked() {
                            if let Some(source_range) = source_range.clone() {
                                self.tabs[idx].primary_cursor = source_range;
                            }
                        }
                        if self.tabs[idx].restore_editor_focus
                            == Some((tab_id, crate::app::EditorPane::Primary))
                        {
                            resp.request_focus();
                            self.tabs[idx].restore_editor_focus = None;
                        }
                        if resp.clicked() {
                            let command_click = ui.input(|input| input.modifiers.command);
                            if command_click {
                                if !self.tabs[idx].extra_cursors.contains(&previous_primary) {
                                    self.tabs[idx].extra_cursors.push(previous_primary);
                                }
                                self.tabs[idx].folds.clear();
                            } else {
                                self.clear_extra_cursors();
                            }
                        }
                        if !self.tabs[idx].extra_cursors.is_empty() {
                            let offset = output.galley_pos.to_vec2();
                            for range in &self.tabs[idx].extra_cursors {
                                let start = editor_cache.view.to_display(range.start);
                                let end = editor_cache.view.to_display_clamped(range.end);
                                let Some(start) = start else {
                                    continue;
                                };
                                let from = output
                                    .galley
                                    .pos_from_cursor(egui::text::CCursor::new(start))
                                    .translate(offset);
                                let to = output
                                    .galley
                                    .pos_from_cursor(egui::text::CCursor::new(end))
                                    .translate(offset);
                                if range.start != range.end {
                                    let rect = egui::Rect::from_min_max(
                                        egui::pos2(from.left(), from.top()),
                                        egui::pos2(to.right().max(from.right()), from.bottom()),
                                    );
                                    highlight_shapes(
                                        &mut under,
                                        rect,
                                        palette::SELECTION(),
                                        palette::ACCENT().gamma_multiply(0.45),
                                    );
                                }
                                // The caret sits at the selection's end, like the
                                // primary cursor after Cmd/Ctrl+D.
                                ui.painter().vline(
                                    to.left(),
                                    egui::Rangef::new(to.top(), to.bottom()),
                                    egui::Stroke::new(1.5_f32, palette::ACCENT()),
                                );
                            }
                        }
                        ui.painter().set(under_text, egui::Shape::Vec(under));

                        let ctx = ui.ctx().clone();

                        // Comment toggle rewrites the buffer and repositions the caret;
                        // skip the suggestion machinery this frame so it doesn't run on a
                        // stale caret. Needs the editor's live selection, so it only fires
                        // when the editor reported one (i.e. it is focused).
                        let toggled = toggle_comment
                            && output.cursor_range.is_some_and(|range| {
                                let chars = range.as_sorted_char_range();
                                self.toggle_line_comment(
                                    &ctx,
                                    editor_id,
                                    editor_cache.view.to_source(chars.start)
                                        ..editor_cache.view.to_source_end(chars.end),
                                );
                                true
                            });

                        let assisted = !toggled
                            && text_changed
                            && focused
                            && cursor_char.is_some_and(|caret| {
                                self.apply_typing_assist(&ctx, editor_id, caret)
                            });

                        if !toggled && !assisted {
                            if self.autocomplete_enabled || force {
                                self.update_autocomplete(
                                    &ctx,
                                    editor_id,
                                    focused,
                                    text_changed,
                                    force,
                                    cursor_char,
                                    cursor_rect,
                                    nav,
                                );
                            } else {
                                self.tabs[idx].editor_assist.autocomplete.open = false;
                            }

                            if self.ghost_suggestions_enabled {
                                self.update_ghost(
                                    ui,
                                    &ctx,
                                    editor_id,
                                    focused,
                                    cursor_char,
                                    cursor_rect,
                                    &font,
                                    accept_ghost,
                                );
                            } else {
                                self.tabs[idx].editor_assist.ghost_suggestion = None;
                                self.tabs[idx].editor_assist.ghost_key = None;
                            }
                        }

                        if !assisted {
                            self.show_symbol_hover(
                                ui,
                                &output.galley,
                                output.galley_pos,
                                &editor_cache.view,
                            );
                            self.update_diagnostics(
                                ui,
                                &output.galley,
                                output.galley_pos,
                                text_changed,
                                cursor_char,
                                &editor_cache.view,
                            );
                        }
                        self.tabs[idx].sql_editor_cache = editor_cache;
                    });
            });
    }
}
