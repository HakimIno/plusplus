use super::{Action, Busy, ConnField, ConnTestState, DbGuiApp, QueryTab, TabView};
use crate::filter::{self, FilterEvent};
use crate::grid::results_grid;
use crate::icons;
use crate::style::{self, palette};
use crate::theme::ThemeId;
use crate::title_bar;

fn field_test_status(state: &ConnTestState, field: ConnField) -> Option<bool> {
    match state {
        ConnTestState::Success => Some(true),
        ConnTestState::Failed { fields, .. } if fields.contains(&field) => Some(false),
        _ => None,
    }
}

fn with_field_status<R>(
    ui: &mut egui::Ui,
    status: Option<bool>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        if let Some(ok) = status {
            let (stroke_color, fill_color) = if ok {
                (
                    egui::Color32::from_rgb(58, 178, 108),
                    egui::Color32::from_rgba_unmultiplied(58, 178, 108, 42),
                )
            } else {
                let danger = palette::DANGER();
                (
                    danger,
                    egui::Color32::from_rgba_unmultiplied(danger.r(), danger.g(), danger.b(), 48),
                )
            };
            let stroke = egui::Stroke::new(1.5, stroke_color);
            let visuals = ui.visuals_mut();
            visuals.extreme_bg_color = fill_color;
            visuals.widgets.inactive.bg_fill = fill_color;
            visuals.widgets.inactive.bg_stroke = stroke;
            visuals.widgets.hovered.bg_fill = fill_color;
            visuals.widgets.hovered.bg_stroke = stroke;
            visuals.widgets.active.bg_fill = fill_color;
            visuals.widgets.active.bg_stroke = stroke;
        }
        add(ui)
    })
    .inner
}

fn status_text_input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    status: Option<bool>,
) -> egui::Response {
    with_field_status(ui, status, |ui| style::text_input(ui, text, hint, width))
}

fn connection_color_to_egui(color: dbcore::ConnectionColor) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

fn egui_to_connection_color(color: egui::Color32) -> dbcore::ConnectionColor {
    dbcore::ConnectionColor::new(color.r(), color.g(), color.b())
}

fn mix_color(base: egui::Color32, accent: egui::Color32, accent_weight: f32) -> egui::Color32 {
    let accent_weight = accent_weight.clamp(0.0, 1.0);
    let base_weight = 1.0 - accent_weight;
    let mix = |base: u8, accent: u8| {
        (base as f32 * base_weight + accent as f32 * accent_weight).round() as u8
    };
    egui::Color32::from_rgb(
        mix(base.r(), accent.r()),
        mix(base.g(), accent.g()),
        mix(base.b(), accent.b()),
    )
}

impl DbGuiApp {
    pub(super) fn top_bar(
        &mut self,
        root: &mut egui::Ui,
        frame: Option<&eframe::Frame>,
        actions: &mut Vec<Action>,
    ) {
        let chrome_inset = title_bar::traffic_lights_inset(root.ctx(), frame);
        let bar_height = title_bar::height(chrome_inset);
        let marker_color = self.active_title_bar_color().map(connection_color_to_egui);
        let breadcrumb_fill = marker_color.map(|color| mix_color(palette::SURFACE(), color, 0.34));

        egui::Panel::top("top_bar")
            .resizable(false)
            .exact_size(bar_height)
            .show_inside(root, |ui| {
                let bar_rect = ui.max_rect();
                let cols = title_bar::columns(bar_rect, chrome_inset);
                let connected = self.active().is_some();
                let has_result = self.tab().result.is_some();
                let breadcrumb = self.breadcrumb_text();

                title_bar::column(ui, cols.left, |ui| {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add_space(chrome_inset.max(6.0));
                            if super::widgets::toolbar_icon_button(
                                ui,
                                icons::plus(),
                                "New connection",
                            )
                            .clicked()
                            {
                                actions.push(Action::NewConnection);
                            }
                            if super::widgets::toolbar_icon_button(
                                ui,
                                icons::disconnect(),
                                "Disconnect",
                            )
                            .clicked()
                                && connected
                            {
                                actions.push(Action::Disconnect);
                            }
                            if has_result {
                                super::widgets::toolbar_sep(ui);
                                if super::widgets::toolbar_icon_button(
                                    ui,
                                    icons::filter(),
                                    "Filter results",
                                )
                                .clicked()
                                {
                                    let visible = self.tab().filter.visible;
                                    self.tab_mut().filter.visible = !visible;
                                }
                            }
                        },
                    );
                });

                title_bar::column(ui, cols.center, |ui| {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            title_bar::breadcrumb(ui, &breadcrumb, breadcrumb_fill);
                        },
                    );
                });

                title_bar::column(ui, cols.right, |ui| {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(6.0);
                            if super::widgets::toolbar_icon_button(
                                ui,
                                icons::settings(),
                                "Settings",
                            )
                            .clicked()
                            {
                                actions.push(Action::OpenSettings);
                            }
                            if super::widgets::layout_toggle(
                                ui,
                                self.show_details_panel,
                                super::widgets::LayoutSide::Details,
                                "Details panel",
                            )
                            .clicked()
                            {
                                self.show_details_panel = !self.show_details_panel;
                            }
                            if super::widgets::layout_toggle(
                                ui,
                                self.show_schema_panel,
                                super::widgets::LayoutSide::Schema,
                                "Schema panel",
                            )
                            .clicked()
                            {
                                self.show_schema_panel = !self.show_schema_panel;
                            }
                            if super::widgets::layout_toggle(
                                ui,
                                self.show_query_console,
                                super::widgets::LayoutSide::Query,
                                "Query console",
                            )
                            .clicked()
                            {
                                self.show_query_console = !self.show_query_console;
                            }
                            if super::widgets::layout_toggle(
                                ui,
                                self.show_connection_tabs,
                                super::widgets::LayoutSide::Connections,
                                "Connection tabs",
                            )
                            .clicked()
                            {
                                self.show_connection_tabs = !self.show_connection_tabs;
                            }
                        },
                    );
                });
            });
    }

    /// Horizontal strip of query tabs (with a × per tab) plus a + button, directly below the
    /// title bar. Switching a tab swaps the whole editor/result/connection view.
    pub(super) fn query_tab_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("query_tabs")
            .resizable(false)
            .exact_size(34.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .fill(palette::PANEL()),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("query_tab_scroll")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Rects collected per frame so the drag handler below can map
                            // the pointer to an insertion slot.
                            let mut rects = Vec::with_capacity(self.tabs.len());
                            let pointer_x = ui.ctx().pointer_interact_pos().map(|p| p.x);
                            for idx in 0..self.tabs.len() {
                                let selected = idx == self.active_query_tab;
                                let label = self.tab_label(idx);
                                let kind = self.tab_kind(idx);
                                let preview = self.tabs[idx].preview;
                                // While this tab is dragged, its chip floats with its
                                // left edge tracking the pointer (minus the grab offset).
                                let drag_float_x = match (self.tab_drag, pointer_x) {
                                    (Some(drag), Some(px)) if drag.id == self.tabs[idx].id => {
                                        Some(px - drag.grab_x)
                                    }
                                    _ => None,
                                };
                                let resp = super::widgets::query_tab_item(
                                    ui,
                                    &label,
                                    kind,
                                    selected,
                                    preview,
                                    drag_float_x,
                                );
                                if resp.close {
                                    actions.push(Action::CloseTab(idx));
                                } else if resp.pinned {
                                    actions.push(Action::PinTab(idx));
                                } else if resp.clicked {
                                    actions.push(Action::SelectTab(idx));
                                } else if resp.drag_started {
                                    // Grabbing a tab selects it (TablePlus-style) and
                                    // starts the reorder, tracked by stable id so the
                                    // grab survives the index changing mid-drag.
                                    self.tab_drag = Some(super::TabDrag {
                                        id: self.tabs[idx].id,
                                        grab_x: pointer_x.unwrap_or(resp.rect.left())
                                            - resp.rect.left(),
                                    });
                                    actions.push(Action::SelectTab(idx));
                                }
                                rects.push(resp.rect);
                                ui.add_space(2.0);
                            }
                            self.handle_tab_drag(ui, &rects, actions);
                            if super::widgets::toolbar_icon_button(
                                ui,
                                icons::plus(),
                                "New query tab (Cmd/Ctrl+T)",
                            )
                            .clicked()
                            {
                                actions.push(Action::NewTab);
                            }
                        });
                    });
            });
    }

    /// While a query tab is being dragged, live-reorder it into the slot under its
    /// floating chip (the strip re-lays-out next frame, so the swap is immediately
    /// visible). The drag ends when the primary button is released.
    fn handle_tab_drag(&mut self, ui: &egui::Ui, rects: &[egui::Rect], actions: &mut Vec<Action>) {
        let Some(drag) = self.tab_drag else { return };
        if !ui.input(|i| i.pointer.primary_down()) {
            self.tab_drag = None;
            return;
        }
        let Some(from) = self.tabs.iter().position(|t| t.id == drag.id) else {
            // The dragged tab vanished (e.g. closed via shortcut mid-drag).
            self.tab_drag = None;
            return;
        };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        // Insertion slot = how many *other* chips sit (by centre) left of the floating
        // chip's centre. Using the floating chip — not the bare pointer — makes the swap
        // fire exactly when the dragged tab visually overlaps a neighbour past its
        // midpoint, Chrome-style, regardless of where inside the tab it was grabbed.
        let float_center = pointer.x - drag.grab_x + rects[from].width() * 0.5;
        let to = rects
            .iter()
            .enumerate()
            .filter(|(i, r)| *i != from && float_center > r.center().x)
            .count();
        if to != from {
            actions.push(Action::MoveTab { from, to });
        }
    }

    /// Thin status strip pinned to the very bottom edge: row count / selection / errors.
    pub(super) fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show_inside(root, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if let Some(err) = &self.error {
                    icons::show_colored(ui, icons::warning(), 13.0, palette::DANGER());
                    ui.label(egui::RichText::new(err).size(11.0).color(palette::DANGER()));
                } else {
                    if self.busy != Busy::Idle {
                        ui.add(egui::Spinner::new().size(11.0));
                        ui.add_space(4.0);
                    }
                    icons::show_weak(ui, icons::table(), 12.0);
                    ui.label(
                        egui::RichText::new(&self.status_msg)
                            .size(11.0)
                            .color(palette::TEXT_WEAK()),
                    );
                    let tab = self.tab();
                    if let Some(res) = &tab.result {
                        if tab.filter.is_active() && tab.row_order.len() != res.row_count() {
                            ui.colored_label(palette::TEXT_FAINT(), "·");
                            icons::show_colored(ui, icons::filter(), 13.0, palette::ACCENT());
                            ui.colored_label(
                                palette::ACCENT(),
                                format!("{} of {} rows", tab.row_order.len(), res.row_count()),
                            );
                        }
                    }
                    if let (Some(sel), true) = (tab.selected_row, tab.result.is_some()) {
                        ui.colored_label(palette::TEXT_FAINT(), "·");
                        ui.colored_label(palette::TEXT_WEAK(), format!("row {}", sel + 1));
                    }
                }
            });
            ui.add_space(3.0);
        });
    }

    /// SQL editor at the very bottom, with syntax highlighting and a Run button.
    pub(super) fn query_console(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::bottom("query_console")
            .resizable(true)
            .default_size(150.0)
            .min_size(72.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::section_header(ui, "Query");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_run = self.active().is_some() && self.busy == Busy::Idle;
                        if icons::primary_button(ui, icons::play(), "Run", can_run)
                            .on_hover_text("Cmd/Ctrl+Enter")
                            .clicked()
                        {
                            actions.push(Action::RunQuery);
                        }
                        ui.add_space(6.0);
                        // Beautify formats in the bound connection's dialect; with no live
                        // connection it still works, falling back to generic SQL.
                        let dialect_label =
                            self.active().map(|a| a.db.kind().label()).unwrap_or("SQL");
                        let has_sql = !self.tab().sql.trim().is_empty();
                        let resp = super::widgets::beautify_button(
                            ui,
                            &mut self.beautify,
                            has_sql,
                            dialect_label,
                        );
                        if resp.clicked {
                            actions.push(Action::BeautifySql);
                        }
                        if resp.prefs_changed {
                            self.persist_settings();
                        }
                    });
                });
                ui.add_space(6.0);

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = crate::highlight::highlight_sql(buf.as_str(), font.clone());
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };

                // Fill the panel's height instead of shrinking to the text: otherwise a long
                // query would grow the scroll area and push the whole panel taller, fighting
                // the size the user dragged it to. With `auto_shrink` off the editor keeps the
                // panel's height and scrolls its content internally.
                egui::ScrollArea::vertical()
                    .id_salt("sql_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        // Size the editor to fill the panel: a `TextEdit` only grows to its
                        // `desired_rows` (or its content), so a fixed row count would leave the
                        // dragged-open panel mostly empty. Derive the row count from the space the
                        // scroll area gives us so the box tracks the resize; content longer than
                        // that scrolls internally.
                        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                        // Leave room for the editor's own vertical margin so the widget doesn't
                        // overflow the viewport by a few pixels and trigger a permanent scrollbar.
                        let avail = ui.available_height() - 2.0 * ui.spacing().item_spacing.y;
                        let rows = (avail / row_height).floor().max(5.0) as usize;
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut self.tab_mut().sql)
                                .code_editor()
                                .desired_rows(rows)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .hint_text("Write SQL here, then press Run (Cmd/Ctrl+Enter)"),
                        );
                        if resp.changed() {
                            // Editing the SQL means the rows currently on screen may no longer
                            // map back to one table, so they turn read-only; the next Run
                            // re-derives editability from the new SQL (`derive_edit_source`).
                            // A previewed tab becomes permanent (just like other editors).
                            let tab = self.tab_mut();
                            tab.edits.source = None;
                            tab.preview = false;
                            self.workspace_dirty = true;
                        }
                    });
            });
    }

    /// Right-hand Details panel: the selected row's columns and values.
    pub(super) fn right_panel(&mut self, root: &mut egui::Ui) {
        // The details panel only makes sense for a selected row; with nothing selected we
        // hide it entirely so the grid gets the full width (rather than showing an empty
        // placeholder panel).
        let idx = self.active_query_tab;
        let tab = &mut self.tabs[idx];
        // The selected row belongs to the data grid, which Structure mode hides.
        if tab.view == TabView::Structure {
            return;
        }
        let row_idx = match (tab.result.as_ref(), tab.selected_row) {
            (Some(_), Some(disp)) if disp < tab.row_order.len() => tab.row_order[disp],
            _ => return,
        };
        let editable = tab.edits.editable();
        // Split the borrow so the closure can hold the result immutably and edits mutably.
        let QueryTab { result, edits, .. } = tab;
        let res = result.as_ref().expect("row_idx implies a result");
        // Disjoint field borrows alongside `tab` above.
        let details_filter = &mut self.details_filter;
        let details_date_pick = &mut self.details_date_pick;

        egui::Panel::right("details_panel")
            .resizable(true)
            .default_size(260.0)
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                style::section_header(ui, "Details");
                // Live field filter, TablePlus-style: typing narrows the stacked fields
                // below by column name. Icon sits inside the field via `icon_text_input`.
                style::icon_text_input(
                    ui,
                    details_filter,
                    "Search for field…",
                    icons::search(),
                    ui.available_width(),
                );
                ui.add_space(4.0);

                // Stacked fields (name + type above an input-styled value box). The box is
                // full-width, so it tracks the panel as it is resized.
                // `auto_shrink([false, _])` keeps the inner ui at the panel width.
                let query = details_filter.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("details_scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (c, col) in res.columns.iter().enumerate() {
                            if !query.is_empty() && !col.name.to_lowercase().contains(&query) {
                                continue;
                            }
                            let kind = edits.col_kind(c);
                            ui.add_space(6.0);
                            // Header: column name on the left, a colour-coded type badge
                            // pinned to the right edge so types scan as a column.
                            ui.horizontal(|ui| {
                                ui.strong(&col.name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| style::type_badge(ui, &col.type_name, kind_color(kind)),
                                );
                            });
                            let value = &res.rows[row_idx][c];
                            details_value_box(
                                ui,
                                edits,
                                kind,
                                row_idx,
                                c,
                                value,
                                editable,
                                details_date_pick,
                            );
                            ui.add_space(4.0);
                        }
                    });
            });
    }

    pub(super) fn connection_tabs(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("connection_tabs")
            .resizable(false)
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .fill(palette::PANEL()),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                let list_h = ui.available_height();

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), list_h),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("active_connection_tabs")
                            .show(ui, |ui| {
                                let bound_id = self.tabs[self.active_query_tab].conn_id.clone();
                                let mut rects = Vec::with_capacity(self.connections.len());
                                let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
                                for (idx, conn) in self.connections.iter().enumerate() {
                                    let live = self
                                        .active_connections
                                        .iter()
                                        .any(|active| active.config_id == conn.id);
                                    // Highlight the connection the active tab is bound to.
                                    let selected = bound_id.as_deref() == Some(conn.id.as_str());
                                    let drag_float_y = match (&self.connection_drag, pointer_y) {
                                        (Some(drag), Some(py)) if drag.id == conn.id => {
                                            Some(py - drag.grab_y)
                                        }
                                        _ => None,
                                    };
                                    let resp = super::widgets::connection_tab_item(
                                        ui,
                                        &conn.name,
                                        selected,
                                        live,
                                        drag_float_y,
                                    )
                                    .on_hover_text(conn.target_summary());
                                    if resp.drag_started() {
                                        self.connection_drag = Some(super::ConnectionDrag {
                                            id: conn.id.clone(),
                                            grab_y: pointer_y.unwrap_or(resp.rect.top())
                                                - resp.rect.top(),
                                        });
                                    }
                                    if resp.clicked() {
                                        if live {
                                            actions.push(Action::BindConnection(idx));
                                        } else {
                                            actions.push(Action::Connect(idx));
                                        }
                                    }
                                    resp.context_menu(|ui| {
                                        let connect_label =
                                            if live { "Reconnect" } else { "Connect" };
                                        if icons::button(ui, icons::connect(), connect_label, true)
                                            .clicked()
                                        {
                                            actions.push(Action::Connect(idx));
                                            ui.close();
                                        }
                                        if icons::button(ui, icons::edit(), "Edit…", true).clicked()
                                        {
                                            actions.push(Action::EditConnection(idx));
                                            ui.close();
                                        }
                                        if live
                                            && icons::button(
                                                ui,
                                                icons::disconnect(),
                                                "Disconnect",
                                                true,
                                            )
                                            .clicked()
                                        {
                                            actions.push(Action::DisconnectConn(idx));
                                            ui.close();
                                        }
                                        if icons::button(ui, icons::trash(), "Delete", true)
                                            .clicked()
                                        {
                                            actions.push(Action::DeleteConnection(idx));
                                            ui.close();
                                        }
                                    });
                                    rects.push(resp.rect);
                                    ui.add_space(2.0);
                                }

                                self.handle_connection_drag(ui, &rects, actions);

                                if self.connections.is_empty() {
                                    ui.vertical_centered(|ui| {
                                        icons::show_weak(ui, icons::database(), 16.0);
                                    });
                                }
                            });
                    },
                );
            });
    }

    /// While a saved connection is dragged, live-reorder it into the vertical slot under
    /// the pointer. The persisted connection list order follows the visible order.
    fn handle_connection_drag(
        &mut self,
        ui: &egui::Ui,
        rects: &[egui::Rect],
        actions: &mut Vec<Action>,
    ) {
        let Some(drag) = self.connection_drag.clone() else {
            return;
        };
        if !ui.input(|i| i.pointer.primary_down()) {
            self.connection_drag = None;
            return;
        }
        let Some(from) = self.connections.iter().position(|c| c.id == drag.id) else {
            self.connection_drag = None;
            return;
        };
        if from >= rects.len() {
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let float_center = pointer.y - drag.grab_y + rects[from].height() * 0.5;
        let to = rects
            .iter()
            .enumerate()
            .filter(|(i, r)| *i != from && float_center > r.center().y)
            .count();
        if to != from {
            actions.push(Action::MoveConnection { from, to });
        }
    }

    pub(super) fn left_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(280.0)
            .min_size(200.0)
            .max_size(360.0)
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.add_space(8.0);
                style::section_header(ui, "Schema");
                style::icon_text_input(
                    ui,
                    &mut self.schema_filter,
                    "filter tables…",
                    icons::filter(),
                    ui.available_width(),
                );
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("schema_scroll")
                    .show(ui, |ui| {
                        // Keep tree content within the panel — long names must not widen it.
                        ui.set_width(ui.available_width());
                        self.schema_tree(ui, actions);
                    });
            });
    }

    fn schema_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            ui.add_space(4.0);
            ui.colored_label(
                palette::TEXT_FAINT(),
                "Connect to a database to browse its schema.",
            );
            return;
        };

        ui.horizontal(|ui| {
            icons::show(ui, icons::database(), icons::SIZE);
            style::truncated_label(
                ui,
                &active.schema.database_name,
                None,
                false,
                egui::Sense::hover(),
            );
        });
        ui.add_space(2.0);

        let filter = self.schema_filter.to_lowercase();
        for table in &active.schema.tables {
            if !filter.is_empty() && !table.name.to_lowercase().contains(&filter) {
                continue;
            }
            let id = ui.make_persistent_id(("tbl", table.name.as_str()));
            let (_toggle, header, _body) =
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                )
                .show_header(ui, |ui| {
                    icons::show_weak(ui, icons::table(), 15.0);
                    ui.add_space(2.0);
                    style::truncated_label(ui, &table.name, None, false, egui::Sense::click())
                })
                .body(|ui| {
                    for col in &table.columns {
                        ui.horizontal(|ui| {
                            let glyph = if col.primary_key {
                                icons::key()
                            } else {
                                icons::column()
                            };
                            icons::show_weak(ui, glyph, 13.0);
                            ui.add_space(2.0);
                            style::truncated_label(
                                ui,
                                &col.name,
                                None,
                                false,
                                egui::Sense::hover(),
                            );
                            let nn = if col.nullable { "" } else { " · not null" };
                            let meta = format!("{}{nn}", col.data_type);
                            style::truncated_label(
                                ui,
                                &meta,
                                Some(&meta),
                                true,
                                egui::Sense::hover(),
                            );
                        });
                    }
                    if !table.indexes.is_empty() {
                        ui.add_space(3.0);
                        for idx in &table.indexes {
                            ui.horizontal(|ui| {
                                icons::show_weak(ui, icons::index(), 13.0);
                                ui.add_space(2.0);
                                let u = if idx.unique { "unique " } else { "" };
                                let detail =
                                    format!("{u}{} ({})", idx.name, idx.columns.join(", "));
                                style::truncated_label(
                                    ui,
                                    &detail,
                                    Some(&detail),
                                    true,
                                    egui::Sense::hover(),
                                );
                            });
                        }
                    }
                });

            let resp = header
                .inner
                .on_hover_text("Click to preview · double-click to open");
            // Single-click previews (reuses the italic preview tab); double-click pins.
            let pin = resp.double_clicked();
            if resp.clicked() || pin {
                // Carry the table + its primary key so the previewed rows become editable.
                let source = crate::edit::EditSource {
                    schema: table.schema.clone(),
                    table: table.name.clone(),
                    pk_cols: table
                        .columns
                        .iter()
                        .filter(|c| c.primary_key)
                        .map(|c| c.name.clone())
                        .collect(),
                };
                actions.push(Action::OpenTable {
                    sql: active.db.kind().preview_query(&table.qualified(), 100),
                    source,
                    pin,
                });
            }
        }
    }

    /// The TablePlus-style filter strip directly above the grid. Only shown when toggled on
    /// and a result with columns is loaded. Edits mutate `self.filter` directly; an Apply or
    /// Clear rebuilds the view.
    pub(super) fn filter_bar(&mut self, root: &mut egui::Ui) {
        let idx = self.active_query_tab;
        // The filter applies to data rows; it has no meaning over the Structure view.
        if !self.tabs[idx].filter.visible || self.tabs[idx].view == TabView::Structure {
            return;
        }
        let col_names: Vec<String> = match &self.tabs[idx].result {
            Some(res) if res.column_count() > 0 => {
                res.columns.iter().map(|c| c.name.clone()).collect()
            }
            _ => return,
        };

        let mut event: Option<FilterEvent> = None;
        egui::Panel::top("filter_bar")
            .resizable(false)
            .show_inside(root, |ui| {
                event = filter::ui(ui, &mut self.tabs[idx].filter, &col_names);
            });

        match event {
            Some(FilterEvent::Apply) => self.tabs[idx].recompute_view(),
            Some(FilterEvent::Clear) => {
                self.tabs[idx].filter.reset();
                self.tabs[idx].recompute_view();
            }
            None => {}
        }
    }

    /// TablePlus-style Data / Structure switch directly below the grid. Only shown for
    /// table tabs whose introspected info is available; everything else is forced back to
    /// Data so a tab can't get stuck on an empty Structure view.
    pub(super) fn view_mode_bar(&mut self, root: &mut egui::Ui) {
        let idx = self.active_query_tab;
        if self.structure_table(idx).is_none() {
            self.tabs[idx].view = TabView::Data;
            return;
        }
        egui::Panel::bottom("view_mode_bar")
            .resizable(false)
            .exact_size(30.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .fill(palette::PANEL()),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.horizontal(|ui| {
                    let view = &mut self.tabs[idx].view;
                    for (mode, label) in
                        [(TabView::Data, "Data"), (TabView::Structure, "Structure")]
                    {
                        if ui
                            .selectable_label(*view == mode, egui::RichText::new(label).size(11.0))
                            .clicked()
                        {
                            *view = mode;
                        }
                    }
                });
            });
    }

    pub(super) fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        // Structure mode replaces the whole grid with the table's introspected definition.
        // `view_mode_bar` already forced the view back to Data when no table info exists,
        // so the lookup here always succeeds in Structure mode.
        if self.tabs[idx].view == TabView::Structure {
            if let Some(info) = self.structure_table(idx).cloned() {
                egui::CentralPanel::default().show_inside(root, |ui| {
                    structure_view(ui, &info);
                });
                return;
            }
        }
        let editable = self.tabs[idx].edits.editable();
        let status_msg = &self.status_msg;
        let tab_id = self.tabs[idx].id;
        let loading = self.querying_tab_id == Some(tab_id);
        let QueryTab {
            result,
            row_order,
            sort,
            selected_row,
            edits,
            ..
        } = &mut self.tabs[idx];
        let sort = *sort;
        egui::CentralPanel::default().show_inside(root, |ui| match result.as_ref() {
            Some(result) if result.column_count() > 0 => {
                let resp =
                    results_grid(ui, result, row_order, sort, *selected_row, edits, editable);
                if let Some(col) = resp.sort {
                    actions.push(Action::SortBy(col));
                }
                if let Some(row) = resp.selected {
                    *selected_row = Some(row);
                }
                // The value a cell edit is typed against: NULL for new (insert) rows, which
                // have no stored value; the stored cell otherwise.
                let original = |r: usize, c: usize| -> Option<dbcore::Value> {
                    if crate::edit::is_new_row(r) {
                        Some(dbcore::Value::Null)
                    } else {
                        result.rows.get(r).and_then(|row| row.get(c)).cloned()
                    }
                };
                // Commit/cancel the open editor before opening a new one, so switching cells
                // doesn't drop the in-progress edit. Values are typed against the stored cell.
                if resp.commit_edit {
                    let cell = edits.active.as_ref().map(|a| (a.row, a.col));
                    if let Some((ar, ac)) = cell {
                        match original(ar, ac) {
                            Some(orig) => {
                                let _ = edits.commit_active(&orig);
                            }
                            None => edits.cancel_active(),
                        }
                    }
                }
                if resp.cancel_edit {
                    edits.cancel_active();
                }
                if let Some((r, c)) = resp.begin_edit {
                    // Continue editing from the staged value if present, else the original.
                    let seed = edits
                        .staged(r, c)
                        .cloned()
                        .or_else(|| original(r, c));
                    if let Some(seed) = seed {
                        edits.begin(r, c, &seed, crate::edit::EditOrigin::Grid);
                    }
                }
                // A boolean cell flips in place rather than opening an editor.
                if let Some((r, c)) = resp.toggle {
                    if let Some(orig) = original(r, c) {
                        edits.toggle_bool(r, c, &orig);
                    }
                }
                // Double-clicking empty table space appends a new (insert) row.
                if resp.add_row {
                    edits.add_new_row();
                }
            }
            Some(_) => {
                style::empty_state(ui, icons::table(), "No columns", status_msg);
            }
            None if loading => {
                style::loading_state(ui, status_msg);
            }
            None => {
                style::empty_illustration(ui, icons::empty_results());
            }
        });
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.settings_open {
            return;
        }

        let mut open = true;
        let mut close = false;
        let mut chosen = self.theme;

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                style::section_header(ui, "Appearance");
                ui.label(egui::RichText::new("Theme").color(palette::TEXT_WEAK()));
                ui.add_space(6.0);

                for id in ThemeId::ALL {
                    ui.radio_value(&mut chosen, id, id.label());
                }

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if icons::button(ui, icons::close(), "Close", true).clicked() {
                        close = true;
                    }
                });
            });

        if chosen != self.theme {
            self.set_theme(ctx, chosen);
        }
        if !open || close {
            actions.push(Action::CloseSettings);
        }
    }

    pub(super) fn connection_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.is_new {
            "New Connection"
        } else {
            "Edit Connection"
        };
        let mut open = true;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let test_state = editor.test_state.clone();
                let mut form_changed = false;
                egui::Grid::new("conn_form")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        let field_w = 240.0;

                        ui.label("Name");
                        form_changed |= status_text_input(
                            ui,
                            &mut editor.config.name,
                            "",
                            field_w,
                            field_test_status(&test_state, ConnField::Name),
                        )
                        .changed();
                        ui.end_row();

                        ui.label("Type");
                        let previous_kind = editor.config.kind;
                        egui::ComboBox::from_id_salt("kind")
                            .selected_text(editor.config.kind.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::Postgres,
                                    "PostgreSQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::MySql,
                                    "MySQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::MariaDb,
                                    "MariaDB",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::SqlServer,
                                    "SQL Server",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::Sqlite,
                                    "SQLite",
                                );
                            });
                        if editor.config.kind != previous_kind {
                            editor.config.port = editor.config.kind.default_port();
                            form_changed = true;
                        }
                        ui.end_row();

                        ui.label("Title bar color");
                        ui.horizontal(|ui| {
                            let mut color = editor
                                .config
                                .title_bar_color
                                .map(connection_color_to_egui)
                                .unwrap_or_else(palette::ACCENT);
                            if egui::color_picker::color_edit_button_srgba(
                                ui,
                                &mut color,
                                egui::color_picker::Alpha::Opaque,
                            )
                            .changed()
                            {
                                editor.config.title_bar_color =
                                    Some(egui_to_connection_color(color));
                                form_changed = true;
                            }
                            if editor.config.title_bar_color.is_none() {
                                ui.label("Default");
                            }
                            if ui.button("Clear").clicked() {
                                if editor.config.title_bar_color.take().is_some() {
                                    form_changed = true;
                                }
                            }
                        });
                        ui.end_row();

                        if editor.config.kind.is_server() {
                            ui.label("Host");
                            form_changed |= status_text_input(
                                ui,
                                &mut editor.config.host,
                                "",
                                field_w,
                                field_test_status(&test_state, ConnField::Host),
                            )
                            .changed();
                            ui.end_row();

                            ui.label("Port");
                            form_changed |= with_field_status(
                                ui,
                                field_test_status(&test_state, ConnField::Port),
                                |ui| {
                                    ui.add_sized(
                                        egui::vec2(80.0, style::CONTROL_H),
                                        egui::DragValue::new(&mut editor.config.port),
                                    )
                                },
                            )
                            .changed();
                            ui.end_row();

                            ui.label("User");
                            form_changed |= status_text_input(
                                ui,
                                &mut editor.config.user,
                                "",
                                field_w,
                                field_test_status(&test_state, ConnField::User),
                            )
                            .changed();
                            ui.end_row();

                            ui.label("Password");
                            form_changed |= with_field_status(
                                ui,
                                field_test_status(&test_state, ConnField::Password),
                                |ui| {
                                    ui.add_sized(
                                        egui::vec2(field_w, style::CONTROL_H),
                                        egui::TextEdit::singleline(&mut editor.password)
                                            .password(true)
                                            .vertical_align(egui::Align::Center)
                                            .margin(egui::Margin::symmetric(6, 0)),
                                    )
                                },
                            )
                            .changed();
                            ui.end_row();

                            ui.label("Database");
                            form_changed |= status_text_input(
                                ui,
                                &mut editor.config.database,
                                "",
                                field_w,
                                field_test_status(&test_state, ConnField::Database),
                            )
                            .changed();
                            ui.end_row();
                        } else {
                            ui.label("File");
                            ui.horizontal(|ui| {
                                form_changed |= status_text_input(
                                    ui,
                                    &mut editor.config.sqlite_path,
                                    "/path/to/database.sqlite",
                                    field_w,
                                    field_test_status(&test_state, ConnField::SqlitePath),
                                )
                                .changed();
                                if ui.button("Browse…").clicked() {
                                    actions.push(Action::BrowseSqlitePath);
                                }
                            });
                            ui.end_row();
                        }
                    });

                ui.add_space(4.0);
                match &editor.test_state {
                    ConnTestState::Testing(_) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Testing connection…");
                        });
                    }
                    ConnTestState::Success => {
                        ui.colored_label(
                            egui::Color32::from_rgb(58, 178, 108),
                            "Connection test succeeded",
                        );
                    }
                    ConnTestState::Failed { message, .. } => {
                        ui.colored_label(palette::DANGER(), message);
                    }
                    ConnTestState::Untested => {}
                }
                if form_changed && !matches!(editor.test_state, ConnTestState::Testing(_)) {
                    editor.test_state = ConnTestState::Untested;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let testing = matches!(editor.test_state, ConnTestState::Testing(_));
                    if icons::button(ui, icons::connect(), "Test", !testing).clicked() {
                        actions.push(Action::TestConnection);
                    }
                    if icons::button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SaveConnection);
                    }
                    if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDialog);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelDialog);
        }
    }
}

/// The Structure view of a table tab: its introspected columns and indexes as two
/// read-only grids, styled after the results grid (TablePlus's "Structure" mode).
fn structure_view(ui: &mut egui::Ui, info: &dbcore::TableInfo) {
    use egui_extras::{Column, TableBuilder};

    let row_height = egui::TextStyle::Monospace.resolve(ui.style()).size + 8.0;
    let header = |ui: &mut egui::Ui, title: &str| {
        ui.add(egui::Label::new(egui::RichText::new(title).strong()).selectable(false));
    };

    egui::ScrollArea::vertical()
        .id_salt("structure_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            style::section_header(ui, "Columns");
            ui.add_space(2.0);
            TableBuilder::new(ui)
                .id_salt("structure_columns")
                .striped(true)
                .resizable(true)
                .vscroll(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .auto_shrink([false, true])
                .column(Column::exact(30.0)) // row-number gutter
                .column(Column::initial(220.0).at_least(60.0).clip(true))
                .column(Column::initial(160.0).at_least(60.0).clip(true))
                .column(Column::initial(90.0).at_least(60.0).clip(true))
                .column(Column::remainder().at_least(60.0).clip(true))
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        ui.add_space(4.0);
                        ui.weak("#");
                    });
                    for title in ["column_name", "data_type", "nullable", "key"] {
                        h.col(|ui| header(ui, title));
                    }
                })
                .body(|mut body| {
                    for (i, col) in info.columns.iter().enumerate() {
                        body.row(row_height, |mut row| {
                            row.col(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(4.0);
                                        ui.weak(
                                            egui::RichText::new(format!("{}", i + 1)).monospace(),
                                        );
                                    },
                                );
                            });
                            row.col(|ui| {
                                if col.primary_key {
                                    icons::show_colored(ui, icons::key(), 13.0, palette::ACCENT());
                                    ui.add_space(2.0);
                                }
                                ui.label(&col.name);
                            });
                            row.col(|ui| {
                                ui.label(egui::RichText::new(&col.data_type).monospace());
                            });
                            row.col(|ui| {
                                if col.nullable {
                                    ui.label("YES");
                                } else {
                                    ui.colored_label(palette::TEXT_WEAK(), "NO");
                                }
                            });
                            row.col(|ui| {
                                if col.primary_key {
                                    ui.colored_label(palette::ACCENT(), "PRIMARY");
                                }
                            });
                        });
                    }
                });

            if !info.indexes.is_empty() {
                ui.add_space(12.0);
                style::section_header(ui, "Indexes");
                ui.add_space(2.0);
                TableBuilder::new(ui)
                    .id_salt("structure_indexes")
                    .striped(true)
                    .resizable(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .auto_shrink([false, true])
                    .column(Column::exact(30.0))
                    .column(Column::initial(260.0).at_least(60.0).clip(true))
                    .column(Column::initial(90.0).at_least(60.0).clip(true))
                    .column(Column::remainder().at_least(60.0).clip(true))
                    .header(24.0, |mut h| {
                        h.col(|ui| {
                            ui.add_space(4.0);
                            ui.weak("#");
                        });
                        for title in ["index_name", "unique", "columns"] {
                            h.col(|ui| header(ui, title));
                        }
                    })
                    .body(|mut body| {
                        for (i, idx) in info.indexes.iter().enumerate() {
                            body.row(row_height, |mut row| {
                                row.col(|ui| {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_space(4.0);
                                            ui.weak(
                                                egui::RichText::new(format!("{}", i + 1))
                                                    .monospace(),
                                            );
                                        },
                                    );
                                });
                                row.col(|ui| {
                                    icons::show_weak(ui, icons::index(), 13.0);
                                    ui.add_space(2.0);
                                    ui.label(&idx.name);
                                });
                                row.col(|ui| {
                                    if idx.unique {
                                        ui.colored_label(palette::ACCENT(), "UNIQUE");
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(idx.columns.join(", "));
                                });
                            });
                        }
                    });
            }
        });
}

/// Semantic colour for a column's editor kind, used by the Details panel's type badges:
/// numbers amber, booleans green, dates/times blue, free text neutral.
fn kind_color(kind: crate::edit::EditorKind) -> egui::Color32 {
    use crate::edit::EditorKind as K;
    match kind {
        K::Int | K::Float | K::Decimal => palette::WARNING(),
        K::Bool => palette::SUCCESS(),
        K::Date | K::Time | K::DateTime => palette::ACCENT(),
        K::Text => palette::TEXT_FAINT(),
    }
}

/// Render one Details-panel value as an input-styled box (TablePlus-look): a bordered
/// full-width field showing the value, with a ⌄ actions menu at its right edge. While the
/// cell is actively being edited the box is replaced by the validated text editor.
///
/// Height of a Details-panel value box (display and edit modes share this).
const DETAILS_VALUE_H: f32 = 26.0;

/// Type-aware behaviour:
/// - clicking the box starts editing (booleans toggle instead);
/// - the ⌄ menu offers Copy plus, when editable: Edit, type-specific quick-sets
///   (TRUE/FALSE, Today/Now, an inline calendar picker for DATE), Set NULL, and Revert;
/// - numbers and date/times render monospace; NULL/bytes render faint; staged (unsaved)
///   values render green with a green border until saved.
#[allow(clippy::too_many_arguments)]
fn details_value_box(
    ui: &mut egui::Ui,
    edits: &mut crate::edit::Edits,
    kind: crate::edit::EditorKind,
    row_idx: usize,
    c: usize,
    value: &dbcore::Value,
    editable: bool,
    date_pick: &mut Option<(usize, usize)>,
) {
    use crate::edit::EditorKind as K;

    if edits.is_active_from(row_idx, c, crate::edit::EditOrigin::Details) {
        // Keep the same painted box as display mode; only swap the inner label for a
        // frameless editor so focus doesn't add a second border and resize the row.
        let h = DETAILS_VALUE_H;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        // Border turns red while the typed value is invalid for the column, matching the
        // red text — clear feedback that the edit can't be committed yet.
        let valid = edits
            .active
            .as_ref()
            .map_or(true, |a| a.kind.is_valid(&a.buf));
        let border = if valid {
            palette::ACCENT()
        } else {
            palette::DANGER()
        };
        if ui.is_rect_visible(rect) {
            ui.painter().rect(
                rect,
                egui::CornerRadius::same(5),
                palette::CODE_BG(),
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
        }
        let mut outcome = crate::edit::EditOutcome::Continue;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                ui.set_clip_rect(rect);
                if let Some(active) = edits.active.as_mut() {
                    outcome = crate::edit::render_editor(ui, active, Some(rect.size()));
                }
            },
        );
        match outcome {
            crate::edit::EditOutcome::Commit => {
                let _ = edits.commit_active(value);
            }
            crate::edit::EditOutcome::Cancel => edits.cancel_active(),
            _ => {}
        }
        return;
    }

    let staged = edits.staged(row_idx, c).cloned();
    let shown = staged.clone().unwrap_or_else(|| value.clone());
    let is_staged = staged.is_some();
    let can_edit = editable && !matches!(value, dbcore::Value::Bytes(_));

    // --- the box: one allocation, a separate hit zone for the ⌄ at the right edge ---
    let h = DETAILS_VALUE_H;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let chev_w = 20.0;
    let chev_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - chev_w, rect.top()), rect.max);
    let chev_resp = ui.interact(chev_rect, resp.id.with("actions"), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() || chev_resp.hovered();
        let stroke_color = if is_staged {
            palette::SUCCESS()
        } else if hovered {
            palette::BORDER_STRONG()
        } else {
            palette::BORDER()
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(5),
            palette::CODE_BG(),
            egui::Stroke::new(1.0, stroke_color),
            egui::StrokeKind::Inside,
        );

        // Value text, single line, clipped before the chevron zone.
        let text_color = if is_staged {
            palette::SUCCESS()
        } else if shown.is_null() || matches!(shown, dbcore::Value::Bytes(_)) {
            palette::TEXT_FAINT()
        } else {
            palette::TEXT()
        };
        let font = if kind.monospace_value() && !shown.is_null() {
            egui::TextStyle::Monospace.resolve(ui.style())
        } else {
            egui::TextStyle::Body.resolve(ui.style())
        };
        let display = if kind == K::Bool && !shown.is_null() {
            if crate::edit::as_bool(&shown) {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        } else {
            shown.display()
        };
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &display,
            0.0,
            egui::TextFormat {
                font_id: font,
                color: text_color,
                italics: shown.is_null(),
                ..Default::default()
            },
        );
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let text_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(chev_rect.left() - 2.0, rect.bottom()));
        ui.painter().with_clip_rect(text_clip).galley(
            egui::pos2(
                rect.left() + crate::edit::DETAILS_VALUE_PAD_X,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            text_color,
        );

        // ⌄ glyph (slightly emphasised on hover).
        let chev_color = if chev_resp.hovered() {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let cc = chev_rect.center();
        let r = 3.0;
        let s = egui::Stroke::new(1.3, chev_color);
        ui.painter().line_segment(
            [cc + egui::vec2(-r, -r * 0.5), cc + egui::vec2(0.0, r * 0.5)],
            s,
        );
        ui.painter().line_segment(
            [cc + egui::vec2(0.0, r * 0.5), cc + egui::vec2(r, -r * 0.5)],
            s,
        );
    }

    // Click-to-edit, like a real input. Booleans toggle instead of opening an editor.
    if can_edit {
        let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
        if resp.clicked() {
            if kind == K::Bool {
                edits.toggle_bool(row_idx, c, value);
            } else {
                // Prefill from the staged value (if any) so editing continues from it.
                edits.begin(row_idx, c, &shown, crate::edit::EditOrigin::Details);
            }
        }
    }

    // The ⌄ actions menu: Copy always; mutating actions only when editable.
    egui::Popup::menu(&chev_resp).show(|ui| {
        ui.set_min_width(150.0);
        if ui.button("Copy value").clicked() {
            ui.ctx().copy_text(shown.as_text());
        }
        if can_edit {
            if kind != K::Bool && ui.button("Edit").clicked() {
                edits.begin(row_idx, c, &shown, crate::edit::EditOrigin::Details);
            }
            ui.separator();
            match kind {
                K::Bool => {
                    if ui.button("Set TRUE").clicked() {
                        edits.stage(row_idx, c, dbcore::Value::Bool(true), value);
                    }
                    if ui.button("Set FALSE").clicked() {
                        edits.stage(row_idx, c, dbcore::Value::Bool(false), value);
                    }
                }
                K::Date => {
                    if ui.button("Pick date…").clicked() {
                        *date_pick = Some((row_idx, c));
                    }
                    if ui.button("Today").clicked() {
                        let today = jiff::Zoned::now().date().to_string();
                        edits.stage(row_idx, c, dbcore::Value::Text(today), value);
                    }
                }
                K::DateTime => {
                    if ui.button("Now").clicked() {
                        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        edits.stage(row_idx, c, dbcore::Value::Text(now), value);
                    }
                }
                _ => {}
            }
            if !shown.is_null() && ui.button("Set NULL").clicked() {
                edits.stage(row_idx, c, dbcore::Value::Null, value);
            }
            if is_staged && ui.button("Revert").clicked() {
                // Staging the original value clears the staged edit.
                edits.stage(row_idx, c, value.clone(), value);
            }
        }
    });

    // Inline calendar opened from the menu: a plain widget below the box, so its own
    // popup behaves normally (a calendar nested inside the menu would close with it).
    if *date_pick == Some((row_idx, c)) {
        let mut date = shown
            .display()
            .trim()
            .parse::<jiff::civil::Date>()
            .unwrap_or_else(|_| jiff::Zoned::now().date());
        ui.add_space(2.0);
        let salt = format!("details_date_{c}");
        let picker = ui.add(egui_extras::DatePickerButton::new(&mut date).id_salt(&salt));
        if picker.changed() {
            edits.stage(row_idx, c, dbcore::Value::Text(date.to_string()), value);
            *date_pick = None;
        }
    }
}
