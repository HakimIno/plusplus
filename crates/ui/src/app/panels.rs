use super::{Action, Busy, ConnField, ConnTestState, DbGuiApp, PageNav, QueryTab, TabView};
use crate::filter::{self, FilterEvent};
use crate::grid::results_grid;
use crate::icons;
use crate::style::{self, palette};
use crate::theme::ThemeId;
use crate::title_bar;

/// Group a number's digits with commas (`1234567` → `"1,234,567"`) for the pager.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

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

/// First non-empty line of a SQL string, for one-line list displays.
fn first_line(sql: &str) -> &str {
    sql.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
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
                let connected = self.active().is_some();
                let has_result = self.tab().result.is_some();
                let breadcrumb = self.breadcrumb_text();

                // Side clusters are drawn first and size themselves from their contents;
                // the breadcrumb then takes exactly the space left between them.
                let left_used = title_bar::cluster(
                    ui,
                    bar_rect,
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(chrome_inset.max(6.0));
                        if super::widgets::toolbar_icon_button(ui, icons::plus(), "New connection")
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

                let right_used = title_bar::cluster(
                    ui,
                    bar_rect,
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.add_space(6.0);
                        self.update_title_bar_button(ui, actions);
                        if super::widgets::toolbar_icon_button(ui, icons::settings(), "Settings")
                            .clicked()
                        {
                            actions.push(Action::OpenSettings);
                        }
                        if super::widgets::toolbar_icon_button(ui, icons::code(), "Query history")
                            .clicked()
                        {
                            actions.push(Action::ToggleHistory);
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

                let center = title_bar::center_rect(bar_rect, left_used, right_used);
                title_bar::cluster(
                    ui,
                    center,
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        title_bar::breadcrumb(ui, &breadcrumb, breadcrumb_fill);
                    },
                );
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
                ui.horizontal(|ui| {
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
                                let tab_count = self.tabs.len();
                                let can_close_others = tab_count > 1;
                                let can_close_right = idx + 1 < tab_count;
                                resp.response.context_menu(|ui| {
                                    ui.set_min_width(200.0);
                                    if ui.button("Close Tab").clicked() {
                                        actions.push(Action::CloseTab(idx));
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            can_close_others,
                                            egui::Button::new("Close Other Tabs"),
                                        )
                                        .clicked()
                                    {
                                        actions.push(Action::CloseOtherTabs(idx));
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            can_close_right,
                                            egui::Button::new("Close Tabs to the Right"),
                                        )
                                        .clicked()
                                    {
                                        actions.push(Action::CloseTabsToRight(idx));
                                        ui.close();
                                    }
                                    if ui.button("Close All Tabs").clicked() {
                                        actions.push(Action::CloseAllTabs);
                                        ui.close();
                                    }
                                    if preview {
                                        ui.separator();
                                        if icons::button(ui, icons::save(), "Pin Tab", true).clicked()
                                        {
                                            actions.push(Action::PinTab(idx));
                                            ui.close();
                                        }
                                    }
                                });
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
            });
    }

    fn update_title_bar_state(&self) -> Option<(String, &'static str, bool)> {
        match &self.update {
            crate::update::UpdatePhase::Downloading { offer, progress } => Some((
                if *progress > 0.0 {
                    format!("Updating… {}%", (*progress * 100.0).round() as u32)
                } else {
                    format!("Updating v{}…", offer.version)
                },
                "Downloading the new version",
                true,
            )),
            crate::update::UpdatePhase::Ready { offer, .. } => Some((
                format!("Install v{}", offer.version),
                "Replace the installed app and relaunch",
                false,
            )),
            crate::update::UpdatePhase::Available(offer)
                if self.update_dismissed.as_deref() != Some(offer.version.as_str()) =>
            {
                Some((
                    format!("Update v{}", offer.version),
                    "A new version is available",
                    false,
                ))
            }
            _ => None,
        }
    }

    /// Outline update button in the title bar, rightmost (Settings sits just to its left).
    fn update_title_bar_button(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some((label, tooltip, busy)) = self.update_title_bar_state() else {
            return;
        };

        let resp = super::widgets::update_outline_button(ui, &label, busy).on_hover_text(tooltip);
        if resp.clicked() && !busy {
            actions.push(Action::OpenUpdateDialog);
        }
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
                let version = format!("v{}", crate::update::CURRENT_VERSION);
                ui.allocate_ui_with_layout(
                    egui::vec2((ui.available_width() - 44.0).max(0.0), ui.available_height()),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(4.0);
                        if let Some(err) = &self.error {
                            icons::show_colored(ui, icons::warning(), 13.0, palette::DANGER());
                            ui.label(
                                egui::RichText::new(err).size(11.0).color(palette::DANGER()),
                            );
                        } else {
                            if self.busy != Busy::Idle {
                                ui.add(style::spinner(11.0));
                                ui.add_space(4.0);
                            }
                            icons::show_native(ui, icons::table(), 12.0);
                            ui.label(
                                egui::RichText::new(&self.status_msg)
                                    .size(11.0)
                                    .color(palette::TEXT_WEAK()),
                            );
                            let tab = self.tab();
                            if let Some(res) = &tab.result {
                                if tab.filter.is_active()
                                    && tab.row_order.len() != res.row_count()
                                {
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
                    },
                );
                ui.label(
                    egui::RichText::new(version)
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
                ui.add_space(8.0);
            });
            ui.add_space(3.0);
        });
    }

    /// Server-side pager, right-aligned in the view-mode bar. Shown only for table tabs
    /// whose SQL is a paged simple read (`LIMIT n …` / `TOP n`) — exactly the queries
    /// [`dbcore::with_page_window`] can rewrite. Page flips re-run against the server, so a
    /// million-row table is browsed one page at a time instead of being fetched whole.
    fn pager(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let tab = self.tab();
        if tab.result.is_none()
            || (tab.edits.source.is_none() && tab.edits.pending_source.is_none())
        {
            return;
        }
        let Some(win) = dbcore::parse_page_window(&tab.sql) else {
            return;
        };
        let Some(limit) = win.limit.filter(|&l| l > 0) else {
            return;
        };
        let shown = tab.result.as_ref().map_or(0, |r| r.row_count() as u64);
        let total = tab.total_rows;
        let idle = self.busy == Busy::Idle;
        let at_start = win.offset == 0;
        let has_more = match total {
            Some(t) => win.offset + shown < t,
            // Unknown total: a full page means there's probably another one.
            None => shown == limit,
        };

        // Right-to-left, so the first widget lands at the right edge:
        // … ⏮ ◀ "1–100 of 1,234,567" ▶ ⏭ · size ▾
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            egui::ComboBox::from_id_salt("pager_size")
                .width(70.0)
                .selected_text(egui::RichText::new(format!("{limit} / page")).size(11.0))
                .show_ui(ui, |ui| {
                    for n in [100u64, 500, 1_000, 5_000, 10_000] {
                        if ui
                            .selectable_label(limit == n, group_digits(n))
                            .clicked()
                            && idle
                        {
                            actions.push(Action::SetPageSize(n));
                        }
                    }
                });

            let nav_btn = |ui: &mut egui::Ui, glyph: &str, enabled: bool, hint: &str| {
                ui.add_enabled(
                    enabled && idle,
                    egui::Button::new(egui::RichText::new(glyph).size(11.0)),
                )
                .on_hover_text(hint.to_string())
                .clicked()
            };
            if nav_btn(ui, "⏭", has_more && total.is_some(), "Last page") {
                actions.push(Action::Page(PageNav::Last));
            }
            if nav_btn(ui, "▶", has_more, "Next page") {
                actions.push(Action::Page(PageNav::Next));
            }
            let range = if shown == 0 {
                "0".to_string()
            } else {
                format!(
                    "{}–{}",
                    group_digits(win.offset + 1),
                    group_digits(win.offset + shown)
                )
            };
            let of = match total {
                Some(t) => format!(" of {}", group_digits(t)),
                None => " of ?".to_string(),
            };
            ui.label(
                egui::RichText::new(format!("{range}{of}"))
                    .size(11.0)
                    .color(palette::TEXT_WEAK()),
            );
            if nav_btn(ui, "◀", !at_start, "Previous page") {
                actions.push(Action::Page(PageNav::Prev));
            }
            if nav_btn(ui, "⏮", !at_start, "First page") {
                actions.push(Action::Page(PageNav::First));
            }
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
                                ui.label(
                                    egui::RichText::new(&col.name)
                                        .strong()
                                        .color(palette::TEXT()),
                                );
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
                                    let active_conn = self
                                        .active_connections
                                        .iter()
                                        .find(|a| a.config_id == conn.id);
                                    let live = active_conn.is_some();
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
                                        conn.icon,
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
                                    let databases: Vec<String> = active_conn
                                        .map(|a| a.databases.clone())
                                        .unwrap_or_default();
                                    let current_db = conn.database.clone();
                                    resp.context_menu(|ui| {
                                        ui.set_min_width(180.0);
                                        let connect_label =
                                            if live { "Reconnect" } else { "Connect" };
                                        if icons::button(ui, icons::connect(), connect_label, true)
                                            .clicked()
                                        {
                                            actions.push(Action::Connect(idx));
                                            ui.close();
                                        }
                                        if live && !databases.is_empty() {
                                            let tint = ui.visuals().widgets.inactive.fg_stroke.color;
                                            let db_img = egui::Image::new(icons::database())
                                                .fit_to_exact_size(egui::vec2(icons::SIZE, icons::SIZE))
                                                .tint(tint);
                                            let btn = egui::Button::image_and_text(db_img, "Switch Database")
                                                .right_text("⏵");
                                            #[allow(deprecated)]
                                            egui::menu::menu_custom_button(
                                                ui,
                                                btn,
                                                |ui| {
                                                    ui.set_min_width(160.0);
                                                    egui::ScrollArea::vertical()
                                                        .max_height(220.0)
                                                        .show(ui, |ui| {
                                                            for db in &databases {
                                                                let is_current = *db == current_db;
                                                                let tint = ui.visuals().widgets.inactive.fg_stroke.color;
                                                                let db_img = egui::Image::new(icons::database())
                                                                    .fit_to_exact_size(egui::vec2(14.0, 14.0))
                                                                    .tint(tint);
                                                                let label = if is_current {
                                                                    format!("✓  {db}")
                                                                } else {
                                                                    db.clone()
                                                                };
                                                                let btn = egui::Button::image_and_text(db_img, label)
                                                                    .min_size(egui::vec2(ui.available_width(), 0.0));
                                                                if ui.add_enabled(!is_current, btn).clicked() {
                                                                    actions.push(Action::SwitchDatabase {
                                                                        conn_idx: idx,
                                                                        database: db.clone(),
                                                                    });
                                                                    ui.close();
                                                                }
                                                            }
                                                        });
                                                },
                                            );
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
                                        icons::show_native(ui, icons::database(), 16.0);
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
                ui.horizontal(|ui| {
                    style::section_header(ui, "Schema");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let connected = self.active().is_some();
                        let resp = icons::icon_button(ui, icons::plus(), "New Table");
                        if resp.enabled() && resp.clicked() && connected {
                            actions.push(Action::OpenNewTable);
                        }
                        if !connected {
                            let _ = resp.on_disabled_hover_text("Connect to a database first");
                        }
                    });
                });
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
            icons::show_native(ui, icons::database(), icons::SIZE);
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
                    icons::show_native(ui, icons::table(), 15.0);
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
                    if !table.foreign_keys.is_empty() {
                        ui.add_space(3.0);
                        for fk in &table.foreign_keys {
                            ui.horizontal(|ui| {
                                icons::show_weak(ui, icons::connect(), 13.0);
                                ui.add_space(2.0);
                                let detail = fk.display();
                                let hover = if fk.name.is_empty() {
                                    format!("{detail} · on delete {}", fk.on_delete)
                                } else {
                                    format!(
                                        "{} · {detail} · on delete {}",
                                        fk.name, fk.on_delete
                                    )
                                };
                                style::truncated_label(
                                    ui,
                                    &detail,
                                    Some(&hover),
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
        // The filter applies to data rows; it has no meaning over the Structure view or
        // while the schema editor occupies the central panel.
        if !self.tabs[idx].filter.visible
            || self.tabs[idx].view == TabView::Structure
            || self.tabs[idx].schema_editor.is_some()
        {
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
    pub(super) fn view_mode_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        if self.structure_table(idx).is_none() {
            self.tabs[idx].view = TabView::Data;
            return;
        }
        let table_info = self.structure_table(idx).cloned();
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
                    // While the schema editor owns the central panel, neither data mode is
                    // current; clicking one closes the editor and switches back.
                    let editing = self.tabs[idx].schema_editor.is_some();
                    {
                        let view = &mut self.tabs[idx].view;
                        for (mode, label) in
                            [(TabView::Data, "Data"), (TabView::Structure, "Structure")]
                        {
                            if ui
                                .selectable_label(
                                    !editing && *view == mode,
                                    egui::RichText::new(label).size(11.0),
                                )
                                .clicked()
                            {
                                *view = mode;
                                if editing {
                                    actions.push(Action::CancelSchema);
                                }
                            }
                        }
                    }
                    // "Edit Table" sits with Data/Structure as a third mode: it swaps the
                    // central panel for the schema editor rather than opening a dialog.
                    if let Some(info) = table_info {
                        if ui
                            .selectable_label(editing, egui::RichText::new("Edit Table").size(11.0))
                            .clicked()
                            && !editing
                        {
                            actions.push(Action::OpenEditTable(info));
                        }
                    }
                    // The server-side pager lives on the right, directly under the grid.
                    self.pager(ui, actions);
                });
            });
    }

    pub(super) fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        // The schema editor takes over the central panel (like Data/Structure) while open.
        if self.tabs[idx].schema_editor.is_some() {
            egui::CentralPanel::default().show_inside(root, |ui| {
                self.schema_editor_view(ui, actions);
            });
            return;
        }
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
                    results_grid(ui, result, row_order, sort, *selected_row, edits, editable, tab_id);
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
                // Commit the open editor into the staged set, typing the value against the
                // stored cell; invalid input matches the click-away rule and is discarded.
                let settle_active = |edits: &mut crate::edit::Edits| {
                    let Some((ar, ac)) = edits.active.as_ref().map(|a| (a.row, a.col)) else {
                        return;
                    };
                    match original(ar, ac) {
                        Some(orig) => {
                            if !edits.commit_active(&orig) {
                                edits.cancel_active();
                            }
                        }
                        None => edits.cancel_active(),
                    }
                };
                if resp.commit_edit {
                    settle_active(edits);
                }
                if resp.cancel_edit {
                    edits.cancel_active();
                }
                if let Some((r, c)) = resp.begin_edit {
                    // An editor can still be open on another cell without ever having
                    // reported lost_focus (its cell may have scrolled out of the virtualized
                    // grid, so the widget wasn't rendered) — settle it first instead of
                    // silently dropping the typed value when `begin` replaces it.
                    if edits.active.as_ref().is_some_and(|a| (a.row, a.col) != (r, c)) {
                        settle_active(edits);
                    }
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
                // Double-clicking empty table space appends a new (insert) row, selects it,
                // and opens an editor on the first text-editable column right away.
                if resp.add_row {
                    settle_active(edits);
                    let new_id = edits.add_new_row();
                    *selected_row = Some(row_order.len() + edits.new_rows - 1);
                    let first_col = (0..result.column_count())
                        .find(|&c| edits.col_kind(c) != crate::edit::EditorKind::Bool);
                    if let Some(c) = first_col {
                        edits.begin(new_id, c, &dbcore::Value::Null, crate::edit::EditOrigin::Grid);
                    }
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

    /// Full-page first-run welcome screen. Replaces the entire window; no title bar.
    /// Called from `draw()` with an early return so no other panels render simultaneously.
    pub(super) fn draw_welcome_page(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let ctx = root.ctx().clone();

        // Left: decorative illustration panel.
        egui::Panel::left("welcome_illus")
            .exact_size(320.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette::PANEL())
                    .inner_margin(egui::Margin::same(0)),
            )
            .show_inside(root, |ui| {
                let rect = ui.max_rect();
                let center = rect.center();
                let p = ui.painter();

                // Concentric accent rings (fading outward).
                for (r, alpha) in [(52.0f32, 80u8), (88.0, 50), (130.0, 25)] {
                    let c = palette::ACCENT().linear_multiply(alpha as f32 / 255.0);
                    p.circle_stroke(center, r, egui::Stroke::new(1.0, c));
                }

                // Floating dots orbiting at various angles & distances.
                for &(angle_deg, dist, r, alpha) in &[
                    (20.0f32, 68.0f32, 5.0f32, 0.70f32),
                    (100.0,   95.0,    3.5,     0.45),
                    (190.0,   72.0,    4.5,     0.60),
                    (270.0,  105.0,    5.5,     0.75),
                    (55.0,   120.0,    3.0,     0.35),
                    (155.0,  115.0,    4.0,     0.50),
                ] {
                    let a = angle_deg.to_radians();
                    let pos = center + egui::vec2(a.cos() * dist, a.sin() * dist);
                    p.circle_filled(pos, r, palette::ACCENT().linear_multiply(alpha));
                }

                // Small sparkle crosses.
                for &(angle_deg, dist) in &[(42.0f32, 92.0f32), (215.0, 98.0)] {
                    let a = angle_deg.to_radians();
                    let pos = center + egui::vec2(a.cos() * dist, a.sin() * dist);
                    let stroke = egui::Stroke::new(1.5, palette::ACCENT().linear_multiply(0.45));
                    let s = 5.0_f32;
                    p.line_segment([pos - egui::vec2(s, 0.0), pos + egui::vec2(s, 0.0)], stroke);
                    p.line_segment([pos - egui::vec2(0.0, s), pos + egui::vec2(0.0, s)], stroke);
                }

                // Large database icon at centre.
                let sz = 52.0;
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_center_size(center, egui::vec2(sz, sz))),
                    |ui| { icons::show_colored(ui, icons::database(), sz, palette::ACCENT()); },
                );

                // Small table icon — upper-right orbit.
                let tbl = center + egui::vec2(56.0, -54.0);
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_center_size(tbl, egui::vec2(22.0, 22.0))),
                    |ui| { icons::show_colored(ui, icons::table(), 22.0, palette::ACCENT().linear_multiply(0.7)); },
                );

                // Small key icon — lower-left orbit.
                let key = center + egui::vec2(-56.0, 52.0);
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_center_size(key, egui::vec2(20.0, 20.0))),
                    |ui| { icons::show_colored(ui, icons::key(), 20.0, palette::ACCENT().linear_multiply(0.55)); },
                );
            });

        // Right: text content, theme picker, CTA.
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette::BASE())
                    .inner_margin(egui::Margin::symmetric(52, 0)),
            )
            .show_inside(root, |ui| {
                // Vertically centre the content block.
                let avail_h = ui.available_height();
                ui.add_space((avail_h - 370.0_f32).max(24.0) / 2.0);

                // --- App name ---
                ui.label(
                    egui::RichText::new("plusplus")
                        .size(44.0)
                        .strong()
                        .color(palette::ACCENT()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Your fast, native database client")
                        .size(14.0)
                        .color(palette::TEXT_WEAK()),
                );

                ui.add_space(30.0);

                // --- Feature bullets ---
                for txt in [
                    "Connect to Postgres, MySQL, MSSQL & SQLite",
                    "Browse schemas, tables & columns",
                    "Inline cell editing with safe transactions",
                    "SQL editor with syntax highlighting",
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("·  ").color(palette::ACCENT()));
                        ui.label(egui::RichText::new(txt).color(palette::TEXT_WEAK()));
                    });
                    ui.add_space(3.0);
                }

                ui.add_space(30.0);

                // --- Theme picker ---
                style::section_header(ui, "Choose a theme");
                ui.add_space(10.0);
                let mut chosen = self.theme;
                for id in ThemeId::ALL {
                    ui.radio_value(&mut chosen, id, id.label());
                    ui.add_space(3.0);
                }
                if chosen != self.theme {
                    self.set_theme(&ctx, chosen);
                }

                ui.add_space(30.0);

                // --- CTA ---
                if icons::primary_button(ui, icons::play(), "Get Started", true).clicked() {
                    actions.push(Action::DismissWelcome);
                }
            });
    }

    pub(super) fn update_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.update_dialog_open {
            return;
        }

        let current = crate::update::CURRENT_VERSION;
        let mut open = true;
        let mut close = false;
        let mut dismiss = false;
        let mut download = false;
        let mut install = false;

        let (title, version, notes, progress, ready, failed, downloading) = match &self.update {
            crate::update::UpdatePhase::Available(offer) => (
                "Update available",
                offer.version.clone(),
                offer.notes.clone(),
                None,
                false,
                None,
                false,
            ),
            crate::update::UpdatePhase::Downloading { offer, progress } => (
                "Downloading update",
                offer.version.clone(),
                offer.notes.clone(),
                Some(*progress),
                false,
                None,
                true,
            ),
            crate::update::UpdatePhase::Ready { offer, .. } => (
                "Ready to install",
                offer.version.clone(),
                offer.notes.clone(),
                Some(1.0),
                true,
                None,
                false,
            ),
            crate::update::UpdatePhase::Failed(msg) => (
                "Update failed",
                String::new(),
                String::new(),
                None,
                false,
                Some(msg.clone()),
                false,
            ),
            _ => return,
        };

        style::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                if !version.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("plusplus v{version}"))
                            .strong()
                            .size(16.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("Current version: v{current}"))
                            .color(palette::TEXT_WEAK()),
                    );
                }
                ui.add_space(8.0);

                if let Some(p) = progress {
                    ui.add(egui::ProgressBar::new(p).show_percentage());
                    ui.add_space(8.0);
                }

                if let Some(err) = &failed {
                    ui.colored_label(palette::DANGER(), err);
                    ui.add_space(8.0);
                } else if !notes.trim().is_empty() {
                    style::section_header(ui, "Release notes");
                    egui::ScrollArea::vertical()
                        .id_salt("update_notes_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.label(notes.trim());
                        });
                    ui.add_space(8.0);
                }

                style::dialog_footer(ui, |ui| {
                    if ready {
                        if icons::primary_button(ui, icons::save(), "Install & Restart", true)
                            .clicked()
                        {
                            install = true;
                        }
                    } else if downloading {
                        ui.add_enabled(false, egui::Button::new("Downloading…"));
                    } else if failed.is_some() {
                        if icons::button(ui, icons::play(), "Retry download", true).clicked() {
                            download = true;
                        }
                    } else if icons::primary_button(
                        ui,
                        icons::play(),
                        "Download update",
                        true,
                    )
                    .clicked()
                    {
                        download = true;
                    }

                    if icons::button(ui, icons::close(), "Later", true).clicked() {
                        dismiss = true;
                    }
                    if icons::button(ui, icons::close(), "Close", true).clicked() {
                        close = true;
                    }
                });
            });

        if install {
            actions.push(Action::InstallUpdate);
        }
        if download {
            actions.push(Action::DownloadUpdate);
        }
        if dismiss {
            actions.push(Action::DismissUpdate);
        }
        if !open || close {
            actions.push(Action::CloseUpdateDialog);
        }
    }

    pub(super) fn whats_new_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.show_whats_new {
            return;
        }

        let mut open = true;
        let mut close = false;

        style::dialog_window("What's New")
            .open(&mut open)
            .resizable(false)
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    egui::RichText::new(format!("plusplus v{}", crate::update::CURRENT_VERSION))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(8.0);

                style::section_header(ui, "Release notes");
                egui::ScrollArea::vertical()
                    .id_salt("whats_new_notes_scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.label("• Implement query history feature with local audit log\n• Refactor dialog UI components for improved consistency and layout\n• Improve light-mode readability\n• Added \"What's New\" dialog on update");
                    });
                ui.add_space(8.0);

                style::dialog_footer(ui, |ui| {
                    if icons::primary_button(ui, icons::play(), "Awesome", true).clicked() {
                        close = true;
                    }
                });
            });

        if !open || close {
            actions.push(Action::DismissWhatsNew);
        }
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.settings_open {
            return;
        }

        let mut open = true;
        let mut close = false;
        let mut chosen = self.theme;

        style::dialog_window("Settings")
            .open(&mut open)
            .resizable(false)
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                style::section_header(ui, "Appearance");
                ui.label(egui::RichText::new("Theme").color(palette::TEXT_WEAK()));
                ui.add_space(6.0);

                for id in ThemeId::ALL {
                    ui.radio_value(&mut chosen, id, id.label());
                }

                ui.add_space(10.0);
                style::section_header(ui, "Privacy");
                if ui
                    .checkbox(&mut self.history_enabled, "Record query history")
                    .on_hover_text(
                        "Append every executed statement (with its outcome) to a local \
                         log file. SQL may contain data values — turn this off for \
                         sensitive work.",
                    )
                    .changed()
                {
                    self.persist_settings();
                }

                style::dialog_footer(ui, |ui| {
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

    /// Right-hand query-history panel (the audit log): every executed statement with its
    /// connection, time, duration, and outcome, newest first. Toggled from the title bar.
    pub(super) fn history_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::right("history_panel")
            .resizable(true)
            .default_size(300.0)
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::section_header(ui, "Query History");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::icon_button(ui, icons::close(), "Hide history").clicked() {
                            actions.push(Action::ToggleHistory);
                        }
                        if icons::icon_button(ui, icons::trash(), "Delete the entire history")
                            .clicked()
                        {
                            actions.push(Action::ClearHistory);
                        }
                    });
                });
                ui.add_space(4.0);

                if self.history_cache.is_empty() {
                    ui.label(
                        egui::RichText::new("No queries recorded yet.")
                            .color(palette::TEXT_WEAK()),
                    );
                    return;
                }

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let body_h = ui.text_style_height(&egui::TextStyle::Body);
                let spacing = ui.spacing().item_spacing.y;
                // Each entry stacks three lines plus a separator; keep the estimate in
                // sync with the layout below so `show_rows` scrolls without jitter.
                let row_h = 3.0 * (body_h + spacing) + 8.0;
                let count = self.history_cache.len();
                egui::ScrollArea::vertical()
                    .id_salt("history_scroll")
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, count, |ui, range| {
                        for offset in range {
                            // Newest entries last in the cache; display newest first.
                            let idx = count - 1 - offset;
                            let entry = &self.history_cache[idx];

                            // Line 1: status + connection, actions on the right.
                            ui.horizontal(|ui| {
                                let (status, color) = if entry.ok {
                                    ("ok", egui::Color32::from_rgb(58, 178, 108))
                                } else {
                                    ("err", palette::DANGER())
                                };
                                ui.label(egui::RichText::new(status).strong().color(color));
                                ui.label(egui::RichText::new(&entry.conn_name).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button("Use")
                                            .on_hover_text("Put this SQL into the active tab")
                                            .clicked()
                                        {
                                            actions.push(Action::UseHistorySql(idx));
                                        }
                                        if ui.small_button("Copy").clicked() {
                                            ui.ctx().copy_text(entry.sql.clone());
                                        }
                                    },
                                );
                            });

                            // Line 2: when, how many rows, how long.
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        entry.at.replace('T', " ").trim_end_matches('Z'),
                                    )
                                    .small()
                                    .color(palette::TEXT_WEAK()),
                                );
                                if let Some(rows) = entry.rows {
                                    ui.label(
                                        egui::RichText::new(format!("{rows} rows"))
                                            .small()
                                            .color(palette::TEXT_WEAK()),
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(format!("{:.0} ms", entry.elapsed_ms))
                                        .small()
                                        .color(palette::TEXT_WEAK()),
                                );
                            });

                            // Line 3: one truncated line of SQL (or the error); hover for all.
                            let detail = match &entry.error {
                                Some(e) => format!("{} — {e}", first_line(&entry.sql)),
                                None => first_line(&entry.sql).to_string(),
                            };
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(detail).font(font.clone()).color(
                                        if entry.ok {
                                            palette::TEXT_WEAK()
                                        } else {
                                            palette::DANGER()
                                        },
                                    ),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&entry.sql);
                            ui.separator();
                        }
                    });
            });
    }

    /// Modal showing the SQL that will be executed, with Commit and Cancel buttons.
    /// Opened by Cmd+S; the user reviews the statements before anything is sent to the DB.
    pub(super) fn commit_preview_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(stmts) = self.commit_pending.clone() else {
            return;
        };

        let title = format!("Review {} Change(s)", stmts.len());
        let mut open = true;
        style::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 440.0])
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "These statements will run as a single transaction. \
                         If any fails, all changes are rolled back.",
                    )
                    .color(palette::TEXT_WEAK()),
                );
                ui.add_space(8.0);

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                egui::ScrollArea::vertical()
                    .id_salt("commit_preview_scroll")
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, stmt) in stmts.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);
                            }
                            let job = crate::highlight::highlight_sql(stmt, font.clone());
                            ui.label(job);
                        }
                    });

                style::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if icons::primary_button(ui, icons::save(), "Commit", can_act)
                        .on_hover_text("Execute all statements in a single transaction")
                        .clicked()
                    {
                        actions.push(Action::ConfirmEdits);
                    }
                    if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelEdits);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelEdits);
        }
    }

    /// Modal listing the destructive statements about to hit a production connection,
    /// with Run and Cancel buttons. Opened by Run when the tab's connection is marked
    /// production and the batch contains UPDATE/DELETE/DROP/TRUNCATE/ALTER.
    pub(super) fn danger_confirm_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(stmts) = self.danger_pending.clone() else {
            return;
        };

        let title = format!("Production: {} Destructive Statement(s)", stmts.len());
        let mut open = true;
        style::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 440.0])
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "This connection is marked as production. \
                         Review the statements below before running them.",
                    )
                    .color(palette::DANGER()),
                );
                ui.add_space(8.0);

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                egui::ScrollArea::vertical()
                    .id_salt("danger_confirm_scroll")
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, stmt) in stmts.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);
                            }
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(stmt.kind.label())
                                        .strong()
                                        .color(palette::DANGER()),
                                );
                                if stmt.missing_where {
                                    ui.label(
                                        egui::RichText::new("no WHERE — affects every row")
                                            .strong()
                                            .color(palette::DANGER()),
                                    );
                                }
                            });
                            let job = crate::highlight::highlight_sql(&stmt.sql, font.clone());
                            ui.label(job);
                        }
                    });

                style::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if icons::primary_button(ui, icons::connect(), "Run", can_act)
                        .on_hover_text("Execute against the production connection")
                        .clicked()
                    {
                        actions.push(Action::ConfirmDangerQuery);
                    }
                    if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDangerQuery);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelDangerQuery);
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
        style::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(style::dialog_frame(ctx))
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

                        ui.label("Icon");
                        ui.horizontal(|ui| {
                            for icon in dbcore::ConnectionIcon::ALL {
                                let selected = editor.config.icon == icon;
                                let resp =
                                    icons::connection_icon_picker_button(ui, icon, selected, 32.0);
                                if resp.clicked() {
                                    editor.config.icon = icon;
                                    form_changed = true;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Type");
                        let previous_kind = editor.config.kind;
                        icons::db_kind_combo(
                            ui,
                            &mut editor.config.kind,
                            "kind",
                            field_w,
                        );
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

                        ui.label("Production");
                        form_changed |= ui
                            .checkbox(
                                &mut editor.config.production,
                                "Confirm destructive queries",
                            )
                            .on_hover_text(
                                "UPDATE, DELETE, DROP, TRUNCATE, and ALTER must be \
                                 confirmed in a dialog before they run",
                            )
                            .changed();
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

                            ui.label("SSL mode");
                            let previous_ssl = editor.config.ssl_mode;
                            egui::ComboBox::from_id_salt("ssl_mode")
                                .selected_text(editor.config.ssl_mode.label())
                                .show_ui(ui, |ui| {
                                    for mode in dbcore::SslMode::ALL {
                                        ui.selectable_value(
                                            &mut editor.config.ssl_mode,
                                            mode,
                                            mode.label(),
                                        );
                                    }
                                });
                            form_changed |= editor.config.ssl_mode != previous_ssl;
                            ui.end_row();

                            if editor.config.ssl_mode.verifies_certificate() {
                                ui.label("CA certificate");
                                ui.horizontal(|ui| {
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.ssl_ca_cert,
                                        "System trust store",
                                        field_w,
                                        None,
                                    )
                                    .changed();
                                    if ui.button("Browse…").clicked() {
                                        actions.push(Action::BrowseSslCaCert);
                                    }
                                });
                                ui.end_row();
                            }

                            if editor.config.kind.supports_client_cert()
                                && editor.config.ssl_mode != dbcore::SslMode::Disable
                            {
                                ui.label("Client certificate");
                                ui.horizontal(|ui| {
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.ssl_client_cert,
                                        "None",
                                        field_w,
                                        None,
                                    )
                                    .changed();
                                    if ui.button("Browse…").clicked() {
                                        actions.push(Action::BrowseSslClientCert);
                                    }
                                });
                                ui.end_row();

                                ui.label("Client key");
                                ui.horizontal(|ui| {
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.ssl_client_key,
                                        "None",
                                        field_w,
                                        None,
                                    )
                                    .changed();
                                    if ui.button("Browse…").clicked() {
                                        actions.push(Action::BrowseSslClientKey);
                                    }
                                });
                                ui.end_row();
                            }

                            ui.label("SSH tunnel");
                            form_changed |= ui
                                .checkbox(
                                    &mut editor.config.ssh_enabled,
                                    "Connect through a bastion host",
                                )
                                .on_hover_text(
                                    "Host and port above are then resolved from the \
                                     bastion, not from this machine",
                                )
                                .changed();
                            ui.end_row();

                            if editor.config.ssh_enabled {
                                ui.label("SSH host");
                                form_changed |= status_text_input(
                                    ui,
                                    &mut editor.config.ssh_host,
                                    "bastion.example.com",
                                    field_w,
                                    None,
                                )
                                .changed();
                                ui.end_row();

                                ui.label("SSH port");
                                form_changed |= ui
                                    .add_sized(
                                        egui::vec2(80.0, style::CONTROL_H),
                                        egui::DragValue::new(&mut editor.config.ssh_port),
                                    )
                                    .changed();
                                ui.end_row();

                                ui.label("SSH user");
                                form_changed |= status_text_input(
                                    ui,
                                    &mut editor.config.ssh_user,
                                    "",
                                    field_w,
                                    None,
                                )
                                .changed();
                                ui.end_row();

                                ui.label("SSH key");
                                ui.horizontal(|ui| {
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.ssh_key_path,
                                        "None — use password",
                                        field_w,
                                        None,
                                    )
                                    .changed();
                                    if ui.button("Browse…").clicked() {
                                        actions.push(Action::BrowseSshKey);
                                    }
                                });
                                ui.end_row();

                                ui.label(if editor.config.ssh_key_path.trim().is_empty() {
                                    "SSH password"
                                } else {
                                    "Key passphrase"
                                });
                                form_changed |= ui
                                    .add_sized(
                                        egui::vec2(field_w, style::CONTROL_H),
                                        egui::TextEdit::singleline(&mut editor.ssh_password)
                                            .password(true)
                                            .vertical_align(egui::Align::Center)
                                            .margin(egui::Margin::symmetric(6, 0)),
                                    )
                                    .changed();
                                ui.end_row();
                            }
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
                            ui.add(style::spinner(style::CONTROL_H));
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
                style::dialog_footer(ui, |ui| {
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

    // ─── Schema Editor dialog ─────────────────────────────────────────────────

    /// The schema editor (Create/Edit Table), rendered inline in the central panel —
    /// it takes the grid's place like the Data/Structure views rather than floating as
    /// a dialog. Only the DDL preview remains a modal (it's a confirm step).
    fn schema_editor_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        use crate::schema::{SchemaEditorMode, SchemaTab};
        let idx = self.active_query_tab;
        let Some(editor) = self.tabs[idx].schema_editor.as_mut() else {
            return;
        };

        let title = match editor.mode {
            SchemaEditorMode::NewTable => "Create Table".to_string(),
            SchemaEditorMode::EditTable => {
                format!("Edit Table — {}", editor.table_name)
            }
        };

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            style::section_header(ui, &title);
            // Action buttons on the right of the header, where the eye lands first.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::primary_button(ui, icons::code(), "Preview SQL", true).clicked() {
                    actions.push(Action::GenerateSchema);
                }
                ui.add_space(6.0);
                if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                    actions.push(Action::CancelSchema);
                }
            });
        });
        ui.add_space(6.0);

        // Table name (only editable in NewTable mode; read-only in EditTable).
        ui.horizontal(|ui| {
            let pad = egui::Margin::symmetric(10, 5);
            ui.label("Table name:");
            let te = egui::TextEdit::singleline(&mut editor.table_name)
                .hint_text("my_table")
                .desired_width(200.0)
                .vertical_align(egui::Align::Center)
                .margin(pad);
            ui.add_enabled(editor.mode == SchemaEditorMode::NewTable, te);
            if !editor.schema_name.is_empty() || editor.mode == SchemaEditorMode::NewTable {
                ui.label("Schema:");
                ui.add(
                    egui::TextEdit::singleline(&mut editor.schema_name)
                        .hint_text("public")
                        .desired_width(120.0)
                        .vertical_align(egui::Align::Center)
                        .margin(pad),
                );
            }
        });
        ui.add_space(6.0);

        // Tab selector: Columns | Indexes | Foreign Keys
        ui.horizontal(|ui| {
            for (tab, label) in [
                (SchemaTab::Columns, "Columns"),
                (SchemaTab::Indexes, "Indexes"),
                (SchemaTab::ForeignKeys, "Foreign Keys"),
            ] {
                if ui
                    .selectable_label(
                        editor.active_tab == tab,
                        egui::RichText::new(label).size(12.0),
                    )
                    .clicked()
                {
                    editor.active_tab = tab;
                }
            }
        });
        ui.separator();
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("schema_editor_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match editor.active_tab {
                SchemaTab::Columns => {
                    schema_columns_tab(ui, &mut editor.columns, editor.mode, editor.db_kind);
                }
                SchemaTab::Indexes => {
                    schema_indexes_tab(ui, &mut editor.indexes);
                }
                SchemaTab::ForeignKeys => {
                    schema_fk_tab(ui, &mut editor.fks);
                }
            });
    }

    // ─── Schema DDL preview dialog ────────────────────────────────────────────

    pub(super) fn schema_preview_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(stmts) = self.schema_pending.clone() else {
            return;
        };

        let title = format!("Preview Migration — {} Statement(s)", stmts.len());
        let mut open = true;
        style::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([660.0, 460.0])
            .frame(style::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Review the generated DDL before applying. \
                         All statements run as a single transaction.",
                    )
                    .color(palette::TEXT_WEAK()),
                );
                ui.add_space(8.0);

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                egui::ScrollArea::vertical()
                    .id_salt("schema_ddl_scroll")
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, stmt) in stmts.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);
                            }
                            let job = crate::highlight::highlight_sql(stmt, font.clone());
                            ui.label(job);
                        }
                    });

                style::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if icons::primary_button(ui, icons::save(), "Apply Migration", can_act)
                        .on_hover_text("Execute all DDL statements in a single transaction")
                        .clicked()
                    {
                        actions.push(Action::ApplySchema);
                    }
                    if icons::button(ui, icons::close(), "Back", true).clicked() {
                        actions.push(Action::CancelSchema);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelSchema);
        }
    }
}

/// The Structure view of a table tab: its introspected columns, indexes, and foreign keys
/// as read-only grids, styled after the results grid (TablePlus's "Structure" mode).
fn structure_view(ui: &mut egui::Ui, info: &dbcore::TableInfo) {
    use egui_extras::{Column, TableBuilder};

    let row_height = egui::TextStyle::Monospace.resolve(ui.style()).size + 8.0;
    let header = |ui: &mut egui::Ui, title: &str| {
        style::paint_table_header_cell(ui);
        ui.add(
            egui::Label::new(
                egui::RichText::new(title)
                    .strong()
                    .color(palette::TEXT()),
            )
            .selectable(false),
        );
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
                        style::paint_table_header_cell(ui);
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("#")
                                .color(palette::TEXT_FAINT())
                                .monospace(),
                        );
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
                            style::paint_table_header_cell(ui);
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("#")
                                    .color(palette::TEXT_FAINT())
                                    .monospace(),
                            );
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

            if !info.foreign_keys.is_empty() {
                ui.add_space(12.0);
                style::section_header(ui, "Foreign Keys");
                ui.add_space(2.0);
                TableBuilder::new(ui)
                    .id_salt("structure_fks")
                    .striped(true)
                    .resizable(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .auto_shrink([false, true])
                    .column(Column::exact(30.0))
                    .column(Column::initial(220.0).at_least(60.0).clip(true))
                    .column(Column::initial(140.0).at_least(60.0).clip(true))
                    .column(Column::initial(220.0).at_least(60.0).clip(true))
                    .column(Column::initial(100.0).at_least(60.0).clip(true))
                    .column(Column::remainder().at_least(60.0).clip(true))
                    .header(24.0, |mut h| {
                        h.col(|ui| {
                            style::paint_table_header_cell(ui);
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("#")
                                    .color(palette::TEXT_FAINT())
                                    .monospace(),
                            );
                        });
                        for title in
                            ["constraint_name", "columns", "references", "on_delete", "on_update"]
                        {
                            h.col(|ui| header(ui, title));
                        }
                    })
                    .body(|mut body| {
                        for (i, fk) in info.foreign_keys.iter().enumerate() {
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
                                    icons::show_weak(ui, icons::connect(), 13.0);
                                    ui.add_space(2.0);
                                    if fk.name.is_empty() {
                                        ui.colored_label(palette::TEXT_FAINT(), "(unnamed)");
                                    } else {
                                        ui.label(&fk.name);
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(fk.columns.join(", "));
                                });
                                row.col(|ui| {
                                    // Qualify the target with its schema only when it lives
                                    // outside this table's own schema.
                                    let target = match (&fk.ref_schema, &info.schema) {
                                        (Some(rs), Some(s)) if rs != s => {
                                            format!("{rs}.{}", fk.ref_table)
                                        }
                                        _ => fk.ref_table.clone(),
                                    };
                                    ui.label(format!(
                                        "{target} ({})",
                                        fk.ref_columns.join(", ")
                                    ));
                                });
                                row.col(|ui| {
                                    ui.label(&fk.on_delete);
                                });
                                row.col(|ui| {
                                    ui.label(&fk.on_update);
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

// ─── Schema editor tab helpers ────────────────────────────────────────────────

/// Common column types per database, offered in the Type dropdown. The current value is
/// always shown even if it isn't in this list (e.g. an exotic type on an existing column).
fn db_type_options(kind: dbcore::DbKind) -> &'static [&'static str] {
    use dbcore::DbKind;
    match kind {
        DbKind::Postgres => &[
            "TEXT", "VARCHAR(255)", "INTEGER", "BIGINT", "SERIAL", "BIGSERIAL",
            "NUMERIC", "REAL", "DOUBLE PRECISION", "BOOLEAN", "DATE", "TIME",
            "TIMESTAMP", "TIMESTAMPTZ", "UUID", "JSONB", "BYTEA",
        ],
        DbKind::MySql | DbKind::MariaDb => &[
            "VARCHAR(255)", "TEXT", "INT", "BIGINT", "TINYINT", "DECIMAL(10,2)",
            "FLOAT", "DOUBLE", "BOOLEAN", "DATE", "DATETIME", "TIMESTAMP", "TIME",
            "JSON", "BLOB",
        ],
        DbKind::SqlServer => &[
            "NVARCHAR(255)", "NVARCHAR(MAX)", "INT", "BIGINT", "BIT",
            "DECIMAL(18,2)", "FLOAT", "REAL", "DATE", "DATETIME2", "TIME",
            "UNIQUEIDENTIFIER", "VARBINARY(MAX)",
        ],
        DbKind::Sqlite => &[
            "TEXT", "INTEGER", "REAL", "NUMERIC", "BLOB", "BOOLEAN", "DATE",
            "DATETIME",
        ],
    }
}

fn schema_columns_tab(
    ui: &mut egui::Ui,
    columns: &mut Vec<crate::schema::ColumnDraft>,
    mode: crate::schema::SchemaEditorMode,
    db_kind: dbcore::DbKind,
) {
    use crate::schema::SchemaEditorMode;

    let mut to_remove: Option<usize> = None;
    // Rows sit flush against each other — the frames' own inner margin is enough.
    ui.spacing_mut().item_spacing.y = 0.0;

    for (i, col) in columns.iter_mut().enumerate() {
        let row_color = if col.drop {
            Some(palette::DANGER().linear_multiply(0.12))
        } else if col.is_existing {
            None
        } else {
            Some(palette::ACCENT().linear_multiply(0.10))
        };

        let frame = egui::Frame::new()
            .fill(row_color.unwrap_or(egui::Color32::TRANSPARENT))
            .inner_margin(egui::Margin::symmetric(4, 3));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                // A singleline TextEdit's height is font height + its own vertical margin —
                // this margin is the only reliable height knob, so pad vertically here.
                let pad = egui::Margin::symmetric(10, 5);
                // Name
                ui.add_enabled(
                    !col.drop,
                    egui::TextEdit::singleline(&mut col.name)
                        .hint_text("column_name")
                        .desired_width(140.0)
                        .vertical_align(egui::Align::Center)
                        .margin(pad),
                );
                // Type — a dropdown of common types for this database. The combo is sized
                // up to match the padded text inputs beside it.
                ui.add_enabled_ui(!col.drop, |ui| {
                    ui.spacing_mut().interact_size.y = 27.0;
                    egui::ComboBox::from_id_salt(("schema_col_type", i))
                        .selected_text(if col.data_type.is_empty() { "TEXT" } else { &col.data_type })
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for ty in db_type_options(db_kind) {
                                ui.selectable_value(&mut col.data_type, ty.to_string(), *ty);
                            }
                        });
                });
                ui.add_space(2.0);
                // Nullable
                style::accent_checkbox(ui, !col.drop, &mut col.nullable, Some("NULL"));
                ui.add_space(2.0);
                // PK
                style::accent_checkbox(ui, !col.drop, &mut col.primary_key, Some("PK"));
                ui.add_space(2.0);
                // Default
                ui.add_enabled(
                    !col.drop,
                    egui::TextEdit::singleline(&mut col.default)
                        .hint_text("default…")
                        .desired_width(90.0)
                        .vertical_align(egui::Align::Center)
                        .margin(pad),
                );

                // Drop / restore button
                if col.is_existing {
                    let (label, hover) = if col.drop {
                        ("Restore", "Keep this column")
                    } else {
                        ("Drop", "Mark column for deletion")
                    };
                    if ui.small_button(label).on_hover_text(hover).clicked() {
                        col.drop = !col.drop;
                    }
                } else if mode == SchemaEditorMode::EditTable {
                    if ui.small_button("✕").on_hover_text("Remove new column").clicked() {
                        to_remove = Some(i);
                    }
                } else {
                    if i > 0 && ui.small_button("✕").on_hover_text("Remove column").clicked() {
                        to_remove = Some(i);
                    }
                }
            });
        });
    }

    if let Some(i) = to_remove {
        columns.remove(i);
    }

    ui.add_space(4.0);
    if ui.small_button("+ Add Column").clicked() {
        columns.push(crate::schema::ColumnDraft::new_empty());
    }
}

fn schema_indexes_tab(ui: &mut egui::Ui, indexes: &mut Vec<crate::schema::IndexDraft>) {
    let mut to_remove: Option<usize> = None;
    ui.spacing_mut().item_spacing.y = 0.0;

    for (i, idx) in indexes.iter_mut().enumerate() {
        let row_color = if idx.drop {
            Some(palette::DANGER().linear_multiply(0.12))
        } else if !idx.is_existing {
            Some(palette::ACCENT().linear_multiply(0.10))
        } else {
            None
        };

        let frame = egui::Frame::new()
            .fill(row_color.unwrap_or(egui::Color32::TRANSPARENT))
            .inner_margin(egui::Margin::symmetric(4, 3));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let pad = egui::Margin::symmetric(10, 5);
                ui.add_enabled(
                    !idx.drop,
                    egui::TextEdit::singleline(&mut idx.name)
                        .hint_text("index_name")
                        .desired_width(150.0)
                        .vertical_align(egui::Align::Center)
                        .margin(pad),
                );
                ui.add_enabled(
                    !idx.drop,
                    egui::TextEdit::singleline(&mut idx.columns_raw)
                        .hint_text("col1, col2")
                        .desired_width(160.0)
                        .vertical_align(egui::Align::Center)
                        .margin(pad),
                );
                ui.add_space(2.0);
                style::accent_checkbox(ui, !idx.drop, &mut idx.unique, Some("Unique"));

                if idx.is_existing {
                    let (label, hover) = if idx.drop {
                        ("Restore", "Keep this index")
                    } else {
                        ("Drop", "Mark index for removal")
                    };
                    if ui.small_button(label).on_hover_text(hover).clicked() {
                        idx.drop = !idx.drop;
                    }
                } else if ui.small_button("✕").on_hover_text("Remove index").clicked() {
                    to_remove = Some(i);
                }
            });
        });
    }

    if let Some(i) = to_remove {
        indexes.remove(i);
    }

    ui.add_space(4.0);
    if ui.small_button("+ Add Index").clicked() {
        indexes.push(crate::schema::IndexDraft::new_empty());
    }
}

fn schema_fk_tab(ui: &mut egui::Ui, fks: &mut Vec<crate::schema::FkDraft>) {
    use dbcore::FkAction;

    let mut to_remove: Option<usize> = None;

    for (i, fk) in fks.iter_mut().enumerate() {
        let row_color = if fk.drop {
            Some(palette::DANGER().linear_multiply(0.12))
        } else if !fk.is_existing {
            Some(palette::ACCENT().linear_multiply(0.10))
        } else {
            None
        };

        let frame = if let Some(c) = row_color {
            egui::Frame::new().fill(c).inner_margin(egui::Margin::symmetric(4, 2))
        } else {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 2))
        };

        frame.show(ui, |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Constraint:");
                    ui.add_enabled(
                        !fk.drop,
                        egui::TextEdit::singleline(&mut fk.constraint_name)
                            .hint_text("fk_name (optional)")
                            .desired_width(160.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Columns:");
                    ui.add_enabled(
                        !fk.drop,
                        egui::TextEdit::singleline(&mut fk.columns_raw)
                            .hint_text("col1, col2")
                            .desired_width(130.0),
                    );
                    ui.label("→");
                    ui.add_enabled(
                        !fk.drop,
                        egui::TextEdit::singleline(&mut fk.ref_table)
                            .hint_text("ref_table")
                            .desired_width(110.0),
                    );
                    ui.label("(");
                    ui.add_enabled(
                        !fk.drop,
                        egui::TextEdit::singleline(&mut fk.ref_columns_raw)
                            .hint_text("ref_col")
                            .desired_width(90.0),
                    );
                    ui.label(")");
                });
                ui.horizontal(|ui| {
                    ui.label("On Delete:");
                    egui::ComboBox::from_id_salt(format!("fk_action_{i}"))
                        .selected_text(fk.on_delete.label())
                        .show_ui(ui, |ui| {
                            for action in FkAction::ALL {
                                ui.selectable_value(&mut fk.on_delete, *action, action.label());
                            }
                        });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if fk.is_existing {
                            let (label, hover) = if fk.drop {
                                ("Restore", "Keep this FK")
                            } else {
                                ("Drop", "Remove FK constraint")
                            };
                            if ui.small_button(label).on_hover_text(hover).clicked() {
                                fk.drop = !fk.drop;
                            }
                        } else if ui.small_button("✕").on_hover_text("Remove FK").clicked() {
                            to_remove = Some(i);
                        }
                    });
                });
                ui.add_space(2.0);
            });
        });
        ui.separator();
    }

    if let Some(i) = to_remove {
        fks.remove(i);
    }

    ui.add_space(4.0);
    if ui.small_button("+ Add Foreign Key").clicked() {
        fks.push(crate::schema::FkDraft::new_empty());
    }
}
