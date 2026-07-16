use super::{
    query_editor_title, result_status, schema_table_key, Action, ActiveConnection, Busy, ConnField,
    ConnTestState, DbGuiApp, PageNav, QueryEditorPlacement, QueryTab, SchemaTableDrag, TabView,
};
use crate::components;
use crate::filter::{self, FilterEvent};
use crate::grid::results_grid;
use crate::icons;
use crate::style::{self, palette};
use crate::title_bar;

/// The ER diagram is hidden for now (not ready to ship); flip to `true` to bring
/// back the toolbar button. The feature code itself is kept intact.
const ERD_ENABLED: bool = false;

/// Byte offset of the `char_idx`-th character in `s` (its length when out of range), for
/// turning the editor's char-based caret indices into `str` slice bounds.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Pure core of the Cmd/Ctrl+/ comment toggle. Given the buffer and a sorted **char** range,
/// returns the byte range to replace and its replacement — or `None` when there's nothing to
/// do (an all-blank selection). VS Code semantics: the selection is grown to whole lines, and
/// if every non-blank line it touches already starts (after its indent) with `--`, the markers
/// are stripped; otherwise a `-- ` is inserted on each non-blank line at the shallowest indent
/// so the markers line up. Only the touched slice is scanned and rebuilt, in a single pass.
pub(super) fn toggle_comment_edit(
    text: &str,
    sel: std::ops::Range<usize>,
) -> Option<(std::ops::Range<usize>, String)> {
    let bytes = text.as_bytes();
    let sel_start = char_to_byte(text, sel.start);
    let sel_end = char_to_byte(text, sel.end);

    // Grow the range to whole lines: back up to the char after the previous newline, and
    // forward to the newline ending the last touched line. A non-empty selection ending exactly
    // at a line start doesn't really reach that line, so drop it (matching VS Code).
    let first_line_start = bytes[..sel_start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let mut region_end = sel_end;
    if region_end > sel_start && region_end > 0 && bytes[region_end - 1] == b'\n' {
        region_end -= 1;
    }
    let last_line_end = bytes[region_end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |i| region_end + i);

    let region = &text[first_line_start..last_line_end];
    let lines: Vec<&str> = region.split('\n').collect();

    let is_blank = |l: &str| l.trim().is_empty();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let commented = |l: &str| l.trim_start().starts_with("--");

    // Nothing meaningful to comment on an all-blank selection.
    if lines.iter().all(|l| is_blank(l)) {
        return None;
    }
    // Uncomment only when every non-blank line already carries a marker; a single bare line
    // means the toggle adds markers instead.
    let uncomment = lines.iter().filter(|l| !is_blank(l)).all(|l| commented(l));
    // Insert every marker at the shallowest indent so they line up under the code.
    let col = lines
        .iter()
        .filter(|l| !is_blank(l))
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);

    let mut out = String::with_capacity(region.len() + lines.len() * 3);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if uncomment {
            let ind = indent(line);
            match line[ind..].strip_prefix("--") {
                // Drop the marker and, if present, the single space we insert after it.
                Some(rest) => {
                    out.push_str(&line[..ind]);
                    out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                None => out.push_str(line),
            }
        } else if is_blank(line) {
            out.push_str(line);
        } else {
            out.push_str(&line[..col]);
            out.push_str("-- ");
            out.push_str(&line[col..]);
        }
    }

    Some((first_line_start..last_line_end, out))
}

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

/// Query failures belong in the result surface: this keeps the database's precise message
/// visible while the user fixes the SQL instead of squeezing it into the one-line status bar.
fn query_error_state(ui: &mut egui::Ui, error: &str) {
    let available_width = ui.available_width();
    ui.add_space((ui.available_height() * 0.18).max(24.0));
    ui.vertical_centered(|ui| {
        icons::show_colored(ui, icons::warning(), 34.0, palette::DANGER());
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("Query failed")
                .size(15.0)
                .strong()
                .color(palette::DANGER()),
        );
        ui.add_space(10.0);
    });

    let box_width = (available_width * 0.72)
        .clamp(280.0, 760.0)
        .min(available_width);
    let left_space = ((available_width - box_width) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(left_space);
        egui::Frame::new()
            .fill(palette::CODE_BG())
            .stroke(egui::Stroke::new(1.0, palette::DANGER()))
            .corner_radius(egui::CornerRadius::same(style::radius::SM))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width((box_width - 24.0).max(0.0));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(error)
                            .monospace()
                            .color(palette::TEXT()),
                    )
                    .wrap()
                    .selectable(true),
                );
            });
    });
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Fix the SQL and run again  ·  Cmd/Ctrl+Enter")
                .color(palette::TEXT_FAINT()),
        );
    });
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
    with_field_status(ui, status, |ui| {
        components::text_input(ui, text, hint, width)
    })
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
                // The whole bar is a drag surface (move window, double-click to maximize) —
                // the OS-native expectation on Windows/Linux where we draw our own chrome.
                // Registered before the clusters so the buttons drawn on top of it still
                // win hit-testing (same pattern as egui's custom_window_frame example).
                let bar_resp = ui.interact(
                    bar_rect,
                    ui.id().with("title_bar_drag"),
                    egui::Sense::click_and_drag(),
                );
                title_bar::handle_chrome_response(ui, &bar_resp);
                let connected = self.active().is_some();
                let has_result = self
                    .tabs
                    .get(self.active_query_tab)
                    .is_some_and(|tab| tab.result.is_some());
                let breadcrumb = self.breadcrumb_text();

                // Side clusters are drawn first and size themselves from their contents;
                // the breadcrumb then takes exactly the space left between them.
                let left_used = title_bar::cluster(
                    ui,
                    bar_rect,
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(chrome_inset.max(6.0));
                        if components::toolbar_icon_button(ui, icons::plus(), "New connection")
                            .clicked()
                        {
                            actions.push(Action::NewConnection);
                        }
                        if components::toolbar_icon_button(ui, icons::disconnect(), "Disconnect")
                            .clicked()
                            && connected
                        {
                            actions.push(Action::Disconnect);
                        }
                        if has_result {
                            components::toolbar_sep(ui);
                            if components::toolbar_icon_button(
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
                        #[cfg(not(target_os = "macos"))]
                        {
                            title_bar::window_controls(ui);
                            title_bar::group_separator(ui);
                        }
                        #[cfg(target_os = "macos")]
                        ui.add_space(6.0);
                        self.update_title_bar_button(ui, actions);
                        if components::toolbar_icon_button(ui, icons::settings(), "Settings")
                            .clicked()
                        {
                            actions.push(Action::OpenSettings);
                        }
                        #[cfg(not(target_os = "macos"))]
                        title_bar::group_separator(ui);
                        if components::toolbar_icon_button(ui, icons::code(), "Query history")
                            .clicked()
                        {
                            actions.push(Action::ToggleHistory);
                        }
                        if ERD_ENABLED
                            && components::toolbar_icon_button(ui, icons::diagram(), "ER diagram")
                                .clicked()
                        {
                            actions.push(Action::ToggleErd);
                        }
                        #[cfg(not(target_os = "macos"))]
                        title_bar::group_separator(ui);
                        if components::layout_toggle(
                            ui,
                            self.show_details_panel,
                            components::LayoutSide::Details,
                            "Details panel",
                        )
                        .clicked()
                        {
                            self.show_details_panel = !self.show_details_panel;
                        }
                        if components::layout_toggle(
                            ui,
                            self.show_schema_panel,
                            components::LayoutSide::Schema,
                            "Schema panel",
                        )
                        .clicked()
                        {
                            self.show_schema_panel = !self.show_schema_panel;
                        }
                        if components::layout_toggle(
                            ui,
                            self.show_query_console,
                            components::LayoutSide::Query,
                            "Query console",
                        )
                        .clicked()
                        {
                            self.show_query_console = !self.show_query_console;
                        }
                        if components::layout_toggle(
                            ui,
                            self.show_connection_tabs,
                            components::LayoutSide::Connections,
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
                                ui.spacing_mut().item_spacing.x = 0.0;
                                // Rects collected per frame so the drag handler below can map
                                // the pointer to an insertion slot.
                                let mut rects = Vec::with_capacity(self.tabs.len());
                                let pointer_x = ui.ctx().pointer_interact_pos().map(|p| p.x);
                                for idx in 0..self.tabs.len() {
                                    let selected = idx == self.active_query_tab;
                                    let label = self.tab_label(idx);
                                    let kind = self.tab_kind(idx);
                                    let db_kind = (kind == crate::components::QueryTabKind::Query)
                                        .then(|| self.tab_db_kind(idx))
                                        .flatten();
                                    let preview = self.tabs[idx].preview;
                                    // While this tab is dragged, its chip floats with its
                                    // left edge tracking the pointer (minus the grab offset).
                                    let drag_float_x = match (self.tab_drag, pointer_x) {
                                        (Some(drag), Some(px)) if drag.id == self.tabs[idx].id => {
                                            Some(px - drag.grab_x)
                                        }
                                        _ => None,
                                    };
                                    let resp = components::query_tab_item(
                                        ui,
                                        &label,
                                        kind,
                                        db_kind,
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
                                            if components::button(
                                                ui,
                                                icons::save(),
                                                "Pin Tab",
                                                true,
                                            )
                                            .clicked()
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
                                if components::toolbar_icon_button(
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

        let resp = components::update_outline_button(ui, &label, busy).on_hover_text(tooltip);
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
    pub(super) fn status_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::bottom("status_bar").show_inside(root, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let version = format!("v{}", crate::update::CURRENT_VERSION);
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        (ui.available_width() - 44.0).max(0.0),
                        ui.available_height(),
                    ),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(4.0);
                        if let Some(err) = &self.error {
                            icons::show_colored(ui, icons::warning(), 13.0, palette::DANGER());
                            ui.label(egui::RichText::new(err).size(11.0).color(palette::DANGER()));
                        } else {
                            if self.busy == Busy::Querying {
                                ui.add(components::spinner(11.0));
                                ui.add_space(4.0);
                                if ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new("Cancel")
                                                .size(11.0)
                                                .color(palette::DANGER()),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text("Abort the running query")
                                    .clicked()
                                {
                                    actions.push(Action::CancelQuery);
                                }
                                ui.add_space(4.0);
                            }
                            icons::show_native(ui, icons::table(), 12.0);
                            ui.label(
                                egui::RichText::new(&self.status_msg)
                                    .size(11.0)
                                    .color(palette::TEXT_WEAK()),
                            );
                            if let Some(tab) = self.tabs.get(self.active_query_tab) {
                                if let Some(res) = &tab.result {
                                    if tab.filter.is_active()
                                        && tab.row_order.len() != res.row_count()
                                    {
                                        ui.colored_label(palette::TEXT_FAINT(), "·");
                                        icons::show_colored(
                                            ui,
                                            icons::filter(),
                                            13.0,
                                            palette::ACCENT(),
                                        );
                                        ui.colored_label(
                                            palette::ACCENT(),
                                            format!(
                                                "{} of {} rows",
                                                tab.row_order.len(),
                                                res.row_count()
                                            ),
                                        );
                                    }
                                }
                                if tab.result.is_some() && !tab.selection.is_empty() {
                                    ui.colored_label(palette::TEXT_FAINT(), "·");
                                    let n = tab.selection.len();
                                    let label = if n > 1 {
                                        format!("{n} rows selected")
                                    } else if let Some(lead) = tab.selection.lead() {
                                        format!("row {}", lead + 1)
                                    } else {
                                        String::new()
                                    };
                                    ui.colored_label(palette::TEXT_WEAK(), label);
                                }
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
                        if ui.selectable_label(limit == n, group_digits(n)).clicked() && idle {
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

    fn query_workspace_bar(
        &mut self,
        ui: &mut egui::Ui,
        kind: crate::components::QueryTabKind,
        actions: &mut Vec<Action>,
    ) {
        let dialect_label = self.active().map(|a| a.db.kind().label()).unwrap_or("SQL");
        let has_sql = !self.tab().sql.trim().is_empty();
        let supports_saved_queries = kind == crate::components::QueryTabKind::Query;
        let showing_saved_queries = supports_saved_queries && self.show_saved_queries;
        let bar_h = 36.0;
        let row_h = 28.0;
        let (bar_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), bar_h),
            egui::Sense::hover(),
        );
        let row_rect =
            egui::Rect::from_center_size(bar_rect.center(), egui::vec2(bar_rect.width(), row_h));
        ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if supports_saved_queries {
                    let saved_label = match self.favorites_cache.len() {
                        0 => "Saved".to_string(),
                        count => format!("Saved ({count})"),
                    };
                    let selected = usize::from(showing_saved_queries);
                    let choice = components::segmented_sized(
                        ui,
                        &[
                            (icons::code(), query_editor_title(kind)),
                            (icons::star_filled(), saved_label.as_str()),
                        ],
                        selected,
                        210.0,
                        false,
                    );
                    if choice != selected {
                        actions.push(Action::ToggleFavoritesTab);
                    }
                } else {
                    components::segmented_sized(
                        ui,
                        &[(icons::code(), query_editor_title(kind))],
                        0,
                        108.0,
                        false,
                    );
                }
                ui.label(
                    egui::RichText::new(format!("{dialect_label} workspace"))
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
                let dot = if self.active().is_some() {
                    palette::SUCCESS()
                } else {
                    palette::TEXT_FAINT()
                };
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(8.0, row_h), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.0, dot);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !showing_saved_queries {
                        if self.busy == Busy::Querying {
                            let resp = ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Cancel")
                                            .color(palette::ON_ACCENT())
                                            .strong(),
                                    )
                                    .fill(palette::DANGER()),
                                )
                                .on_hover_text("Abort the running query");
                            if resp.clicked() {
                                actions.push(Action::CancelQuery);
                            }
                        } else {
                            let can_run = self.active().is_some() && self.busy == Busy::Idle;
                            if components::primary_button(ui, icons::play(), "Run", can_run)
                                .on_hover_text("Cmd/Ctrl+Enter")
                                .clicked()
                            {
                                actions.push(Action::RunQuery);
                            }
                        }
                        let resp = components::beautify_button(
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
                        if components::button(ui, icons::save(), "Save query", has_sql)
                            .on_hover_text("Save the current editor query")
                            .clicked()
                        {
                            actions.push(Action::SaveCurrentAsFavorite);
                        }
                    }
                });
            });
        });
    }

    /// SQL editor with syntax highlighting and a Run button. Query/definition tabs dock it
    /// above their output; table/view tabs keep it below their data grid.
    pub(super) fn query_console(
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
        let panel_id = egui::Id::new(("query_console", tab_id, placement));
        let footer_id = egui::Id::new(("query_footer", tab_id, placement));
        let footer = |app: &mut Self, root: &mut egui::Ui, actions: &mut Vec<Action>| {
            let panel = match placement {
                QueryEditorPlacement::Top => egui::Panel::top(footer_id),
                QueryEditorPlacement::Bottom => egui::Panel::bottom(footer_id),
            };
            panel
                .exact_size(36.0)
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 0)))
                .show_inside(root, |ui| app.query_workspace_bar(ui, kind, actions));
        };

        let panel = match placement {
            QueryEditorPlacement::Top => egui::Panel::top(panel_id),
            QueryEditorPlacement::Bottom => egui::Panel::bottom(panel_id),
        };
        let response = panel
            .resizable(true)
            .default_size(default_size)
            .min_size(min_size)
            .max_size(max_size)
            .frame(egui::Frame::new().inner_margin(egui::Margin::ZERO))
            .show_inside(root, |ui| {
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = crate::highlight::highlight_sql(buf.as_str(), font.clone());
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };

                // Autocomplete: while the popup is open, steal its navigation keys before the
                // editor renders so arrows/Enter/Tab drive the suggestion list instead of the
                // text cursor. Ctrl+Space force-opens it (also when the prefix is empty).
                let mut nav = crate::autocomplete::NavKeys::default();
                if self.autocomplete.open {
                    ui.input_mut(|i| {
                        nav.down |= i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                        nav.up |= i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                        nav.accept |= i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                        nav.accept |= i.consume_key(egui::Modifiers::NONE, egui::Key::Tab);
                        nav.dismiss |= i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                    });
                }
                let force =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));

                // Cmd/Ctrl+/ toggles line comments over the selection, like VS Code. Consumed
                // here — before the editor — so the keystroke never lands as a literal '/'; the
                // edit itself is applied after the render, once we have the live cursor range.
                let toggle_comment =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash));

                // Ghost text (fish-shell autosuggestion): when the popup is closed and a
                // suggestion was trailing the caret last frame, Tab accepts it. Stolen here,
                // before the editor, so the keystroke drives the suggestion, not a literal tab.
                let accept_ghost = !self.autocomplete.open
                    && self.ghost_suggestion.is_some()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));

                // Fill the panel's height instead of shrinking to the text: otherwise a long
                // query would grow the scroll area and push the whole panel taller, fighting
                // the size the user dragged it to. With `auto_shrink` off the editor keeps the
                // panel's height and scrolls its content internally.
                egui::Frame::new()
                    .fill(palette::CODE_BG())
                    .inner_margin(egui::Margin::ZERO)
                    .show(ui, |ui| {
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
                                let avail = ui.available_height();
                                let rows = (avail / row_height).floor().max(5.0) as usize;
                                let line_count =
                                    self.tabs[self.active_query_tab].sql.lines().count().max(1);
                                let digits = line_count.to_string().len();
                                let digit_width = ui.fonts_mut(|fonts| {
                                    fonts.glyph_width(
                                        &egui::TextStyle::Monospace.resolve(ui.style()),
                                        '0',
                                    )
                                });
                                let gutter_width = digits as f32 * digit_width + 14.0;
                                // `.show()` (not `ui.add`) exposes the galley + cursor so the popup can
                                // anchor under the caret and we can move the caret after an insertion.
                                let output = ui
                                    .horizontal_top(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        let gutter_height =
                                            rows.max(line_count) as f32 * row_height;
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
                                        if ui.is_rect_visible(gutter_rect) {
                                            ui.painter().vline(
                                                gutter_rect.right(),
                                                gutter_rect.y_range(),
                                                egui::Stroke::new(1.0, palette::BORDER()),
                                            );
                                            for line in 1..=line_count {
                                                ui.painter().text(
                                                    egui::pos2(
                                                        gutter_rect.right() - 7.0,
                                                        gutter_rect.top()
                                                            + (line - 1) as f32 * row_height,
                                                    ),
                                                    egui::Align2::RIGHT_TOP,
                                                    line,
                                                    font.clone(),
                                                    palette::TEXT_FAINT(),
                                                );
                                            }
                                        }

                                        egui::TextEdit::multiline(
                                            &mut self.tabs[self.active_query_tab].sql,
                                        )
                                        .code_editor()
                                        .frame(egui::Frame::NONE)
                                        .margin(egui::Margin::ZERO)
                                        .desired_rows(rows)
                                        .desired_width(f32::INFINITY)
                                        .layouter(&mut layouter)
                                        .hint_text("SELECT ...")
                                        .show(ui)
                                    })
                                    .inner;

                                let resp = &output.response.response;
                                let editor_id = resp.id;
                                let focused = resp.has_focus();
                                let text_changed = resp.changed();
                                if text_changed {
                                    // Editing the SQL means the rows currently on screen may no longer
                                    // map back to one table, so they turn read-only; the next Run
                                    // re-derives editability from the new SQL (`derive_edit_source`).
                                    // A previewed tab becomes permanent (just like other editors).
                                    let tab = self.tab_mut();
                                    tab.edits.source = None;
                                    tab.preview = false;
                                    self.workspace_dirty = true;
                                }

                                // Caret position (char index + on-screen rect) drives the popup.
                                let cursor = output.cursor_range.map(|r| r.primary);
                                let cursor_char = cursor.map(|c| c.index);
                                let cursor_rect = cursor.map(|c| {
                                    output
                                        .galley
                                        .pos_from_cursor(c)
                                        .translate(output.galley_pos.to_vec2())
                                });

                                let ctx = ui.ctx().clone();

                                // Comment toggle rewrites the buffer and repositions the caret;
                                // skip the suggestion machinery this frame so it doesn't run on a
                                // stale caret. Needs the editor's live selection, so it only fires
                                // when the editor reported one (i.e. it is focused).
                                let toggled = toggle_comment
                                    && output.cursor_range.is_some_and(|range| {
                                        self.toggle_line_comment(&ctx, editor_id, range);
                                        true
                                    });

                                if !toggled {
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
                                }
                            });
                    });
            });

        // Reserve the footer after the editor on the same docking side. For top-docked Query
        // tabs this puts it below the SQL; for bottom-docked Table/View tabs it puts it above.
        footer(self, root, actions);

        let rendered_size = response.response.rect.height();
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

    /// Drive the editor's inline ghost-text suggestion for one frame: recompute it from the
    /// caret, paint the greyed remainder, and apply a pending Tab acceptance. Suppressed
    /// while the autocomplete popup is open (they share the Tab key).
    #[allow(clippy::too_many_arguments)]
    fn update_ghost(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        editor_id: egui::Id,
        focused: bool,
        cursor_char: Option<usize>,
        cursor_rect: Option<egui::Rect>,
        font: &egui::FontId,
        accept: bool,
    ) {
        // The popup owns the caret area and the Tab key while it's up; no ghost then.
        let (Some(cursor_char), Some(cursor_rect)) = (cursor_char, cursor_rect) else {
            self.ghost_suggestion = None;
            self.ghost_key = None;
            return;
        };
        if !focused || self.autocomplete.open {
            self.ghost_suggestion = None;
            self.ghost_key = None;
            return;
        }

        // Recompute only when the text or caret moved since the cached suggestion. While the
        // focused editor merely repaints — e.g. the result grid is scrolling — reuse the cached
        // value instead of re-scanning history and the schema every frame.
        let key = (
            self.tabs[self.active_query_tab].sql.chars().count(),
            cursor_char,
        );
        if self.ghost_key != Some(key) {
            self.ghost_suggestion = {
                let (schema, kind) = match self.active() {
                    Some(c) => (Some(&c.schema), Some(c.db.kind())),
                    None => (None, None),
                };
                // Only complete from history this tab's own connection produced — never from
                // another database's queries (whose tables, and SQL dialect, won't match).
                let conn_id = self.tabs[self.active_query_tab].conn_id.as_deref();
                let pool: Vec<&str> = match conn_id {
                    Some(id) => self
                        .suggest_pool
                        .iter()
                        .filter(|q| q.conn_id == id)
                        .map(|q| q.sql.as_str())
                        .collect(),
                    None => Vec::new(),
                };
                let sql = &self.tabs[self.active_query_tab].sql;
                crate::ghost::suggest(sql, cursor_char, &pool, schema, kind)
            };
            self.ghost_key = Some(key);
        }

        let Some(remainder) = self.ghost_suggestion.clone() else {
            return;
        };

        if accept {
            self.accept_ghost(ctx, editor_id, &remainder);
            self.ghost_suggestion = None;
            self.ghost_key = None;
            return;
        }

        // Paint the remainder in a faint colour, flush against the caret. A multi-line
        // remainder flows left-aligned from the caret's x — fine for the common single-line
        // case, which is what history completions almost always are.
        ui.painter().text(
            egui::pos2(cursor_rect.left(), cursor_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &remainder,
            font.clone(),
            palette::TEXT_FAINT(),
        );
        self.ghost_suggestion = Some(remainder);
    }

    /// Append an accepted ghost suggestion at the caret (the end of the buffer) and move
    /// the caret past it.
    fn accept_ghost(&mut self, ctx: &egui::Context, editor_id: egui::Id, remainder: &str) {
        let tab = &mut self.tabs[self.active_query_tab];
        tab.sql.push_str(remainder);
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;

        let new_cursor = self.tabs[self.active_query_tab].sql.chars().count();
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(new_cursor),
                )));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
    }

    /// Toggle `-- ` line comments over the selected lines (Cmd/Ctrl+/), VS Code style, then
    /// restore a caret/selection over the same text. The buffer rewrite itself lives in the
    /// pure [`toggle_comment_edit`] so it can be unit-tested without an egui context.
    fn toggle_line_comment(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        range: egui::text::CCursorRange,
    ) {
        let idx = self.active_query_tab;
        let chars = range.as_sorted_char_range();
        let Some((byte_range, out)) = toggle_comment_edit(&self.tabs[idx].sql, chars.clone())
        else {
            return;
        };

        // Restore a caret/selection over the same text. For a bare caret, keep its distance
        // from the end of its line (markers land near the start, so this tracks the caret
        // through the shift); for a real selection, re-cover the rewritten lines.
        let sql = &self.tabs[idx].sql;
        let first_line_start_char = sql[..byte_range.start].chars().count();
        let new_range = if chars.start == chars.end {
            let old_chars = sql[byte_range.clone()].chars().count();
            let tail = old_chars.saturating_sub(chars.start - first_line_start_char);
            let offset = out.chars().count().saturating_sub(tail);
            egui::text::CCursorRange::one(egui::text::CCursor::new(first_line_start_char + offset))
        } else {
            egui::text::CCursorRange::two(
                egui::text::CCursor::new(first_line_start_char),
                egui::text::CCursor::new(first_line_start_char + out.chars().count()),
            )
        };

        let tab = &mut self.tabs[idx];
        tab.sql.replace_range(byte_range, &out);
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;

        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state.cursor.set_char_range(Some(new_range));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
    }

    /// Drive the SQL editor's autocomplete popup for one frame: recompute suggestions from
    /// the caret position, apply navigation/accept, and draw the popup. Called from
    /// [`Self::query_console`] right after the editor renders.
    #[allow(clippy::too_many_arguments)]
    fn update_autocomplete(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        focused: bool,
        text_changed: bool,
        force: bool,
        cursor_char: Option<usize>,
        cursor_rect: Option<egui::Rect>,
        nav: crate::autocomplete::NavKeys,
    ) {
        // Esc dismisses without touching the text; the editor never saw the keystroke.
        if nav.dismiss {
            self.autocomplete.open = false;
            return;
        }

        // While the editor is focused it reports a live caret; cache it so a click on the
        // popup — which strips the editor's focus that same frame, nulling the caret — can
        // still recompute and resolve the insertion point.
        if let (Some(cc), Some(cr)) = (cursor_char, cursor_rect) {
            self.autocomplete.caret_char = cc;
            self.autocomplete.anchor = cr;
        }

        // Open while actively typing (or on a forced trigger); a caret that merely sits in a
        // word — e.g. after a click — shouldn't pop the menu back up on its own.
        let typing = text_changed || force;
        // Only scan the schema when there's a reason to: the user is typing/forcing, or the
        // popup is already open and following the prefix. Recomputing every focused frame meant
        // re-scanning the whole schema even while idle — e.g. when the grid scrolls and the
        // still-focused editor keeps repainting — which made scrolling janky on big schemas.
        if focused && (typing || self.autocomplete.open) {
            // Recompute against the live text. Borrow the connection's schema and the tab's
            // SQL immutably together, then hand ownership back so the borrows end.
            let completion = {
                let (schema, kind) = match self.active() {
                    Some(c) => (Some(&c.schema), Some(c.db.kind())),
                    None => (None, None),
                };
                let sql = &self.tabs[self.active_query_tab].sql;
                crate::autocomplete::complete(
                    sql,
                    self.autocomplete.caret_char,
                    schema,
                    kind,
                    force,
                )
            };

            match completion {
                Some(c) => {
                    self.autocomplete.items = c.items;
                    self.autocomplete.replace_start = c.replace_start;
                    self.autocomplete.open = true;
                    self.autocomplete.selected = self
                        .autocomplete
                        .selected
                        .min(self.autocomplete.items.len() - 1);
                }
                None => {
                    self.autocomplete.open = false;
                }
            }
        }
        // When not focused we leave `open`/`items` as they were: the popup keeps showing so a
        // click in progress can land on a row. The click-outside check below closes it.

        if !self.autocomplete.open {
            return;
        }

        // Apply list navigation consumed before the editor rendered.
        let len = self.autocomplete.items.len();
        if nav.down {
            self.autocomplete.selected = (self.autocomplete.selected + 1) % len;
        }
        if nav.up {
            self.autocomplete.selected = (self.autocomplete.selected + len - 1) % len;
        }

        let anchor = self.autocomplete.anchor;
        let (event, popup_rect) =
            crate::autocomplete::show_popup(ctx, &self.autocomplete, anchor, nav.up || nav.down);

        let accept = if nav.accept {
            Some(self.autocomplete.selected)
        } else if let crate::autocomplete::Event::Accept(i) = event {
            Some(i)
        } else {
            None
        };
        if let Some(idx) = accept {
            self.accept_completion(ctx, editor_id, idx, self.autocomplete.caret_char);
            return;
        }

        // The editor lost focus without a row being chosen. Keep the popup only while the
        // pointer is pressing inside it (the press half of a click on a row, before the
        // release that fires `clicked()`); any other focus loss — a click elsewhere, Tab
        // away — dismisses it.
        if !focused {
            let keep = ctx.input(|i| {
                (i.pointer.any_down() || i.pointer.any_pressed())
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| popup_rect.contains(p))
            });
            if !keep {
                self.autocomplete.open = false;
            }
        }
    }

    /// Insert suggestion `idx` over the prefix at the caret, then move the caret past it.
    fn accept_completion(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        idx: usize,
        cursor_char: usize,
    ) {
        let Some(suggestion) = self.autocomplete.items.get(idx).map(|s| s.insert.clone()) else {
            return;
        };
        let start = self.autocomplete.replace_start;
        let tab = &mut self.tabs[self.active_query_tab];
        let byte_start = char_to_byte(&tab.sql, start);
        let byte_cursor = char_to_byte(&tab.sql, cursor_char);
        // Guard against indices that went stale if the text shifted under us this frame.
        if byte_start > byte_cursor || byte_cursor > tab.sql.len() {
            self.autocomplete.open = false;
            return;
        }
        tab.sql.replace_range(byte_start..byte_cursor, &suggestion);
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;

        // Move the editor's caret to just after the inserted text.
        let new_cursor = start + suggestion.chars().count();
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(new_cursor),
                )));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
        self.autocomplete.open = false;
    }

    /// Right-hand Details panel: the selected row's columns and values.
    pub(super) fn right_panel(&mut self, root: &mut egui::Ui) {
        // The details panel only makes sense for a selected row; with nothing selected we
        // hide it entirely so the grid gets the full width (rather than showing an empty
        // placeholder panel).
        let idx = self.active_query_tab;
        let tab = &mut self.tabs[idx];
        // The selected row belongs to the data grid, which every other result surface hides.
        if tab.view != TabView::Data {
            return;
        }
        let row_idx = match (tab.result.as_ref(), tab.selection.lead()) {
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
                components::section_header(ui, "Details");
                // Live field filter, TablePlus-style: typing narrows the stacked fields
                // below by column name. Icon sits inside the field via `icon_text_input`.
                components::icon_text_input(
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
                                    |ui| {
                                        components::type_badge(ui, &col.type_name, kind_color(kind))
                                    },
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
                                    let resp = components::connection_tab_item(
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
                                        if components::button(
                                            ui,
                                            icons::connect(),
                                            connect_label,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            actions.push(Action::Connect(idx));
                                            ui.close();
                                        }
                                        if live && !databases.is_empty() {
                                            components::menu_button(
                                                ui,
                                                icons::database(),
                                                "Switch Database",
                                                |ui| {
                                                    ui.set_min_width(160.0);
                                                    egui::ScrollArea::vertical()
                                                        .max_height(220.0)
                                                        .show(ui, |ui| {
                                                            for db in &databases {
                                                                let is_current = *db == current_db;
                                                                let tint = ui
                                                                    .visuals()
                                                                    .widgets
                                                                    .inactive
                                                                    .fg_stroke
                                                                    .color;
                                                                let db_img = egui::Image::new(
                                                                    icons::database(),
                                                                )
                                                                .fit_to_exact_size(egui::vec2(
                                                                    14.0, 14.0,
                                                                ))
                                                                .tint(tint);
                                                                let label = if is_current {
                                                                    format!("✓  {db}")
                                                                } else {
                                                                    db.clone()
                                                                };
                                                                let btn =
                                                                    egui::Button::image_and_text(
                                                                        db_img, label,
                                                                    )
                                                                    .min_size(egui::vec2(
                                                                        ui.available_width(),
                                                                        0.0,
                                                                    ));
                                                                if ui
                                                                    .add_enabled(!is_current, btn)
                                                                    .clicked()
                                                                {
                                                                    actions.push(
                                                                        Action::SwitchDatabase {
                                                                            conn_idx: idx,
                                                                            database: db.clone(),
                                                                        },
                                                                    );
                                                                    ui.close();
                                                                }
                                                            }
                                                        });
                                                },
                                            );
                                        }
                                        if components::button(ui, icons::edit(), "Edit…", true)
                                            .clicked()
                                        {
                                            actions.push(Action::EditConnection(idx));
                                            ui.close();
                                        }
                                        if live
                                            && components::button(
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
                                        if components::button(ui, icons::trash(), "Delete", true)
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
                    components::section_header(ui, "Schema");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let connected = self.active().is_some();
                        // SQLite has no stored functions or procedures.
                        let supports_routines = self
                            .active()
                            .is_some_and(|a| a.db.kind() != dbcore::DbKind::Sqlite);
                        let menu = ui.add_enabled_ui(connected, |ui| {
                            let plus = egui::Image::new(icons::plus())
                                .fit_to_exact_size(egui::vec2(icons::SIZE, icons::SIZE))
                                .tint(ui.visuals().widgets.inactive.fg_stroke.color);
                            ui.menu_button(plus, |ui| {
                                ui.set_min_width(150.0);
                                if components::button(ui, icons::table(), "New Table…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenNewTable);
                                    ui.close();
                                }
                                if components::button(ui, icons::view(), "New View…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenNewView);
                                    ui.close();
                                }
                                if components::button(ui, icons::play(), "New Trigger…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenNewTrigger);
                                    ui.close();
                                }
                                if supports_routines {
                                    ui.separator();
                                    if components::button(ui, icons::code(), "New Function…", true)
                                        .clicked()
                                    {
                                        actions.push(Action::OpenNewRoutine(
                                            dbcore::RoutineKind::Function,
                                        ));
                                        ui.close();
                                    }
                                    if components::button(ui, icons::code(), "New Procedure…", true)
                                        .clicked()
                                    {
                                        actions.push(Action::OpenNewRoutine(
                                            dbcore::RoutineKind::Procedure,
                                        ));
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Create a new object");
                        });
                        if !connected {
                            let _ = menu
                                .response
                                .on_disabled_hover_text("Connect to a database first");
                        }
                    });
                });
                components::icon_text_input(
                    ui,
                    &mut self.schema_filter,
                    "filter tables…",
                    icons::filter(),
                    ui.available_width(),
                );
                ui.add_space(4.0);

                if self.active().is_some() {
                    egui::ScrollArea::vertical()
                        .id_salt("schema_scroll")
                        .show(ui, |ui| {
                            // Keep tree content within the panel — long names must not widen it.
                            ui.set_width(ui.available_width());
                            self.schema_tree(ui, actions);
                        });
                } else {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        let avail = ui.available_height();
                        ui.add_space(avail * 0.4);
                        if self.busy == Busy::Connecting {
                            ui.add(components::spinner(32.0));
                            ui.add_space(16.0);
                            ui.label(
                                egui::RichText::new("Connecting...")
                                    .color(palette::TEXT_WEAK())
                                    .size(14.0),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Connect to a database to browse its schema.")
                                    .color(palette::TEXT_FAINT()),
                            );
                        }
                    });
                }
            });
    }

    fn schema_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            return;
        };

        ui.horizontal(|ui| {
            icons::show_native(ui, icons::database(), icons::SIZE);
            components::truncated_label(
                ui,
                &active.schema.database_name,
                None,
                false,
                egui::Sense::hover(),
            );
        });
        ui.add_space(2.0);

        let conn_id = active.config_id.as_str();
        let filter = self.schema_filter.to_lowercase();
        let visible =
            |t: &dbcore::TableInfo| filter.is_empty() || t.name.to_lowercase().contains(&filter);

        // One continuous list: pinned tables always sort to the top, while the saved custom
        // order controls positions within the pinned and unpinned groups.
        let custom_order = self.schema_table_order.get(conn_id);
        let mut tables: Vec<&dbcore::TableInfo> = active
            .schema
            .tables
            .iter()
            .filter(|table| visible(table))
            .collect();
        tables.sort_by_key(|table| {
            let is_pinned = dbcore::bookmarks::is_pinned(
                &self.bookmarks,
                conn_id,
                table.schema.as_deref(),
                &table.name,
            );
            let key = schema_table_key(table.schema.as_deref(), &table.name);
            let custom_rank = custom_order
                .and_then(|order| order.iter().position(|item| item == &key))
                .unwrap_or(usize::MAX);
            let bookmark_rank = self
                .bookmarks
                .iter()
                .position(|bookmark| {
                    bookmark.matches(conn_id, table.schema.as_deref(), &table.name)
                })
                .unwrap_or(usize::MAX);
            (!is_pinned, custom_rank, bookmark_rank)
        });

        for table in tables {
            let is_pinned = dbcore::bookmarks::is_pinned(
                &self.bookmarks,
                conn_id,
                table.schema.as_deref(),
                &table.name,
            );
            self.schema_table_row(ui, active, table, is_pinned, "tbl", actions);
        }

        // Views, functions, procedures, and triggers follow the tables.
        self.schema_object_tree(ui, actions);
    }

    /// One table entry in the schema explorer: a modern full-width row — a rounded
    /// selection/hover pill, an accent table icon, the name, and a pin (star) toggle — with
    /// the table's columns / indexes / foreign keys as a collapsible body. Used by both the
    /// "Pinned" group and the main table list; `id_salt` keeps their expand state independent.
    fn schema_table_row(
        &self,
        ui: &mut egui::Ui,
        active: &ActiveConnection,
        table: &dbcore::TableInfo,
        pinned: bool,
        id_salt: &str,
        actions: &mut Vec<Action>,
    ) {
        use egui::collapsing_header::CollapsingState;
        const ROW_H: f32 = 26.0;

        // Selected = this table is what the active tab is currently showing.
        let selected = self.tab().edits.source.as_ref().is_some_and(|s| {
            s.schema.as_deref() == table.schema.as_deref() && s.table == table.name
        });

        let id = ui.make_persistent_id((id_salt, table.schema.as_deref(), table.name.as_str()));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);

        let full_w = ui.available_width();
        let (row_rect, row_resp) =
            ui.allocate_exact_size(egui::vec2(full_w, ROW_H), egui::Sense::click_and_drag());
        // Keep the row's height in the outer scroll area, but avoid building its icons,
        // interactions, and menus when a collapsed table is outside the viewport. Large schemas
        // commonly contain thousands of tables, so this removes most per-frame widget work.
        if !ui.is_rect_visible(row_rect) && state.openness(ui.ctx()) <= 0.0 {
            return;
        }
        let row_resp =
            row_resp.on_hover_text("Click to preview · drag to reorder · double-click to open");
        let payload = SchemaTableDrag {
            conn_id: active.config_id.clone(),
            schema: table.schema.clone(),
            table: table.name.clone(),
            pinned,
        };
        row_resp.dnd_set_drag_payload(payload);
        if let Some(source) = row_resp.dnd_hover_payload::<SchemaTableDrag>() {
            let same_table = source.conn_id == active.config_id
                && source.schema == table.schema
                && source.table == table.name;
            let compatible =
                source.conn_id == active.config_id && source.pinned == pinned && !same_table;
            if compatible && ui.is_rect_visible(row_rect) {
                let after = ui
                    .ctx()
                    .pointer_interact_pos()
                    .is_some_and(|pointer| pointer.y > row_rect.center().y);
                let y = if after {
                    row_rect.bottom()
                } else {
                    row_rect.top()
                };
                ui.painter().hline(
                    row_rect.x_range(),
                    y,
                    egui::Stroke::new(2.0, palette::ACCENT()),
                );
                if let Some(source) = row_resp.dnd_release_payload::<SchemaTableDrag>() {
                    actions.push(Action::MoveSchemaTable {
                        conn_id: active.config_id.clone(),
                        source_schema: source.schema.clone(),
                        source_table: source.table.clone(),
                        target_schema: table.schema.clone(),
                        target_table: table.name.clone(),
                        after,
                    });
                }
            }
        }

        // Pill background: a soft accent-tinted selection fill when selected (no border), a
        // plain raised fill on hover.
        if ui.is_rect_visible(row_rect) {
            let r = egui::CornerRadius::same(7);
            if selected {
                ui.painter().rect_filled(row_rect, r, palette::SELECTION());
            } else if row_resp.hovered() {
                ui.painter()
                    .rect_filled(row_rect, r, palette::SURFACE_HOVER());
            }
        }

        // Row content, painted on top of the pill. The chevron and the star are their own
        // interactive widgets layered above `row_resp`, so they capture their own clicks while
        // the rest of the row drives preview/open.
        let mut toggle_open = false;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 0.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    // Disclosure chevron — rotates from ▸ to ▾ as the body opens.
                    let (chev_rect, chev_resp) =
                        ui.allocate_exact_size(egui::vec2(12.0, ROW_H), egui::Sense::click());
                    paint_chevron(
                        ui.painter(),
                        chev_rect.center(),
                        state.openness(ui.ctx()),
                        palette::TEXT_FAINT(),
                    );
                    if chev_resp.clicked() {
                        toggle_open = true;
                    }

                    // Accent table icon (neutral on the selected pill for contrast).
                    let icon_color = if selected {
                        palette::TEXT()
                    } else {
                        palette::ACCENT()
                    };
                    let (icon_rect, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, ROW_H), egui::Sense::hover());
                    egui::Image::new(icons::table()).tint(icon_color).paint_at(
                        ui,
                        egui::Rect::from_center_size(icon_rect.center(), egui::vec2(15.0, 15.0)),
                    );

                    // Split the strip left of the row edge: the pin sits flush right, the name
                    // fills everything to its left (left-aligned right after the icon). Computing
                    // the rects directly keeps the star pinned to the edge regardless of name
                    // length, and reserves its slot so hovering never reflows the row.
                    let rest = ui.available_rect_before_wrap();
                    ui.allocate_rect(rest, egui::Sense::hover());
                    let star_rect = egui::Rect::from_min_size(
                        egui::pos2(rest.right() - 20.0, rest.top()),
                        egui::vec2(20.0, ROW_H),
                    );
                    let label_rect = egui::Rect::from_min_max(
                        rest.min,
                        egui::pos2(star_rect.left() - 4.0, rest.bottom()),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(label_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&table.name).color(palette::TEXT()),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        },
                    );

                    // Pin (star) toggle: always shown when pinned, otherwise only on hover.
                    let star_resp = ui.interact(
                        star_rect,
                        ui.make_persistent_id((
                            id_salt,
                            "star",
                            table.schema.as_deref(),
                            table.name.as_str(),
                        )),
                        egui::Sense::click(),
                    );
                    if (pinned || selected || row_resp.hovered()) && ui.is_rect_visible(star_rect) {
                        let color = if pinned {
                            palette::ACCENT()
                        } else if star_resp.hovered() {
                            palette::TEXT()
                        } else {
                            palette::TEXT_FAINT()
                        };
                        // Solid star once pinned so the "on" state reads at a glance; a hollow
                        // outline for the hover-to-pin affordance.
                        let star = if pinned {
                            icons::star_filled()
                        } else {
                            icons::star()
                        };
                        egui::Image::new(star).tint(color).paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                star_rect.center(),
                                egui::vec2(14.0, 14.0),
                            ),
                        );
                    }
                    if star_resp.clicked() {
                        actions.push(Action::ToggleBookmark {
                            schema: table.schema.clone(),
                            table: table.name.clone(),
                        });
                    }
                    let _ = star_resp.on_hover_text(if pinned { "Unpin" } else { "Pin to top" });
                });
            },
        );

        // Right-click anywhere on the row opens the full table actions menu.
        row_resp.context_menu(|ui| table_actions_menu(ui, table, pinned, actions));

        // Single-click previews (reuses the italic preview tab); double-click pins a tab.
        let open_pin = row_resp.double_clicked();
        if row_resp.clicked() || open_pin {
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
                sql: active
                    .db
                    .kind()
                    .preview_query(&table.qualified(active.db.kind()), 100),
                source,
                pin: open_pin,
                kind: crate::components::QueryTabKind::Table,
            });
        }

        if toggle_open {
            state.toggle(ui);
        }
        // Children (columns / indexes / FKs) as a proper tree: indented well past the parent
        // and connected by a subtle vertical guide line, so the nesting reads at a glance.
        const CHILD_INDENT: f32 = 28.0;
        let guide_x = row_rect.left() + 17.0;
        let body = state.show_body_unindented(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.add_space(CHILD_INDENT);
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    schema_table_body(ui, table);
                    ui.add_space(1.0);
                });
            });
        });
        if let Some(inner) = body {
            let rect = inner.response.rect;
            ui.painter().vline(
                guide_x,
                egui::Rangef::new(rect.top(), (rect.bottom() - 5.0).max(rect.top())),
                egui::Stroke::new(1.0, palette::BORDER()),
            );
        }
    }

    /// Render the non-table schema objects — views, functions, procedures, triggers — as
    /// collapsible groups beneath the tables. Each group appears only when it has objects
    /// matching the sidebar filter, and is collapsed by default to keep the tree compact.
    fn schema_object_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            return;
        };
        let kind = active.db.kind();
        let filter = self.schema_filter.to_lowercase();
        let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);

        // ── Views: collapsible like tables (columns as children), click to preview rows. ──
        let views: Vec<&dbcore::ViewInfo> = active
            .schema
            .views
            .iter()
            .filter(|v| matches(&v.name))
            .collect();
        if !views.is_empty() {
            object_group(ui, "views_group", "Views", views.len(), |ui| {
                for view in views {
                    let id =
                        ui.make_persistent_id(("view", view.schema.as_deref(), view.name.as_str()));
                    let (_t, header, _b) =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            id,
                            false,
                        )
                        .show_header(ui, |ui| {
                            let kind = crate::components::QueryTabKind::View;
                            icons::show_colored(ui, kind.icon(), 15.0, kind.color());
                            ui.add_space(2.0);
                            let label = if view.materialized {
                                format!("{} · materialized", view.name)
                            } else {
                                view.name.clone()
                            };
                            components::truncated_label(
                                ui,
                                &label,
                                None,
                                false,
                                egui::Sense::click(),
                            )
                        })
                        .body(|ui| {
                            for col in &view.columns {
                                ui.horizontal(|ui| {
                                    icons::show_weak(ui, icons::column(), 13.0);
                                    ui.add_space(2.0);
                                    components::truncated_label(
                                        ui,
                                        &col.name,
                                        None,
                                        false,
                                        egui::Sense::hover(),
                                    );
                                    components::truncated_label(
                                        ui,
                                        &col.data_type,
                                        Some(&col.data_type),
                                        true,
                                        egui::Sense::hover(),
                                    );
                                });
                            }
                        });
                    let resp = header
                        .inner
                        .on_hover_text("Click to preview rows · right-click for actions");
                    resp.context_menu(|ui| {
                        ui.set_min_width(170.0);
                        if components::button(ui, icons::edit(), "Edit View…", true).clicked() {
                            actions.push(Action::OpenEditView(view.clone()));
                            ui.close();
                        }
                        if components::button(ui, icons::trash(), "Drop View…", true)
                            .on_hover_text("Delete this view")
                            .clicked()
                        {
                            actions.push(Action::DropView(view.clone()));
                            ui.close();
                        }
                    });
                    if resp.clicked() || resp.double_clicked() {
                        let source = crate::edit::EditSource {
                            schema: view.schema.clone(),
                            table: view.name.clone(),
                            // Views have no primary key, so the preview grid stays read-only.
                            pk_cols: Vec::new(),
                        };
                        actions.push(Action::OpenTable {
                            sql: kind.preview_query(&view.qualified(kind), 100),
                            source,
                            pin: resp.double_clicked(),
                            kind: crate::components::QueryTabKind::View,
                        });
                    }
                }
            });
        }

        // ── Functions & Procedures: leaf rows; click opens the definition for reading. ──
        for (rk, key, title) in [
            (dbcore::RoutineKind::Function, "fn_group", "Functions"),
            (dbcore::RoutineKind::Procedure, "proc_group", "Procedures"),
        ] {
            let routines: Vec<&dbcore::RoutineInfo> = active
                .schema
                .routines
                .iter()
                .filter(|r| r.kind == rk && matches(&r.name))
                .collect();
            if routines.is_empty() {
                continue;
            }
            object_group(ui, key, title, routines.len(), |ui| {
                virtualized_object_rows(ui, &routines, |ui, index, r| {
                    let signature = r.signature();
                    let tab_kind = match r.kind {
                        dbcore::RoutineKind::Function => crate::components::QueryTabKind::Function,
                        dbcore::RoutineKind::Procedure => {
                            crate::components::QueryTabKind::Procedure
                        }
                    };
                    ui.push_id(
                        (
                            "routine",
                            index,
                            r.schema.as_deref(),
                            r.name.as_str(),
                            signature.as_str(),
                        ),
                        |ui| {
                            let row = object_leaf_row(
                                ui,
                                tab_kind.icon(),
                                tab_kind.color(),
                                &r.name,
                                &signature,
                            );
                            row.context_menu(|ui| {
                                ui.set_min_width(170.0);
                                if components::button(ui, icons::edit(), "Edit…", true).clicked()
                                {
                                    actions.push(Action::OpenEditRoutine((*r).clone()));
                                    ui.close();
                                }
                                if components::button(ui, icons::trash(), "Drop…", true).clicked()
                                {
                                    actions.push(Action::DropRoutine((*r).clone()));
                                    ui.close();
                                }
                            });
                            if row.clicked() || row.double_clicked() {
                                actions.push(Action::OpenDefinition {
                                    title: r.name.clone(),
                                    sql: r.body.clone(),
                                    kind: tab_kind,
                                });
                            }
                        },
                    );
                });
            });
        }

        // ── Triggers: leaf rows; click opens the trigger's CREATE text for reading. ──
        let triggers: Vec<&dbcore::TriggerInfo> = active
            .schema
            .triggers
            .iter()
            .filter(|t| matches(&t.name))
            .collect();
        if !triggers.is_empty() {
            object_group(ui, "trig_group", "Triggers", triggers.len(), |ui| {
                virtualized_object_rows(ui, &triggers, |ui, index, t| {
                    let tab_kind = crate::components::QueryTabKind::Trigger;
                    ui.push_id(
                        (
                            "trigger",
                            index,
                            t.schema.as_deref(),
                            t.table.as_str(),
                            t.name.as_str(),
                        ),
                        |ui| {
                            let row = object_leaf_row(
                                ui,
                                tab_kind.icon(),
                                tab_kind.color(),
                                &t.name,
                                &t.display(),
                            );
                            row.context_menu(|ui| {
                                ui.set_min_width(170.0);
                                if components::button(ui, icons::edit(), "Edit Trigger…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenEditTrigger((*t).clone()));
                                    ui.close();
                                }
                                if components::button(ui, icons::trash(), "Drop Trigger…", true)
                                    .clicked()
                                {
                                    actions.push(Action::DropTrigger((*t).clone()));
                                    ui.close();
                                }
                            });
                            if row.clicked() || row.double_clicked() {
                                actions.push(Action::OpenDefinition {
                                    title: t.name.clone(),
                                    sql: t.action.clone(),
                                    kind: tab_kind,
                                });
                            }
                        },
                    );
                });
            });
        }
    }

    /// The TablePlus-style filter strip directly above the grid. Only shown when toggled on
    /// and a result with columns is loaded. Edits mutate `self.filter` directly; an Apply or
    /// Clear rebuilds the view.
    pub(super) fn filter_bar(&mut self, root: &mut egui::Ui) {
        let idx = self.active_query_tab;
        // The filter applies to data rows; it has no meaning over another result view or
        // while the schema editor occupies the central panel.
        if !self.tabs[idx].filter.visible
            || self.tabs[idx].view != TabView::Data
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

    /// Contextual result switch next to the query toolbar. Query tabs show Data / Message /
    /// Chart; table and view tabs show Data / Structure / Edit Table.
    pub(super) fn view_mode_bar(
        &mut self,
        root: &mut egui::Ui,
        placement: QueryEditorPlacement,
        actions: &mut Vec<Action>,
    ) {
        let idx = self.active_query_tab;
        let query_result_tabs = self.tabs[idx].kind == crate::components::QueryTabKind::Query;
        if !query_result_tabs && self.structure_table(idx).is_none() {
            self.tabs[idx].view = TabView::Data;
            return;
        }
        let table_info = self.structure_table(idx).cloned();
        let panel = match placement {
            QueryEditorPlacement::Top => egui::Panel::top("view_mode_bar"),
            QueryEditorPlacement::Bottom => egui::Panel::bottom("view_mode_bar"),
        };
        panel
            .resizable(false)
            .exact_size(38.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 5))
                    .fill(palette::PANEL()),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.horizontal(|ui| {
                    if query_result_tabs {
                        let modes = [TabView::Data, TabView::Message, TabView::Chart];
                        let selected = modes
                            .iter()
                            .position(|mode| *mode == self.tabs[idx].view)
                            .unwrap_or(0);
                        let choice = components::segmented_sized(
                            ui,
                            &[
                                (icons::table(), "Data"),
                                (icons::code(), "Message"),
                                (icons::diagram(), "Chart"),
                            ],
                            selected,
                            270.0,
                            false,
                        );
                        self.tabs[idx].view = modes[choice];
                        return;
                    }
                    // While the schema editor owns the central panel, neither data mode is
                    // current; clicking one closes the editor and switches back.
                    let editing = self.tabs[idx].schema_editor.is_some();
                    let selected = if editing {
                        2
                    } else if self.tabs[idx].view == TabView::Structure {
                        1
                    } else {
                        0
                    };
                    let choice = components::segmented_sized(
                        ui,
                        &[
                            (icons::table(), "Data"),
                            (icons::column(), "Structure"),
                            (icons::edit(), "Edit Table"),
                        ],
                        selected,
                        300.0,
                        false,
                    );
                    if choice != selected {
                        match choice {
                            0 => {
                                self.tabs[idx].view = TabView::Data;
                                if editing {
                                    actions.push(Action::CancelSchema);
                                }
                            }
                            1 => {
                                self.tabs[idx].view = TabView::Structure;
                                if editing {
                                    actions.push(Action::CancelSchema);
                                }
                            }
                            2 => {
                                if let Some(info) = table_info {
                                    actions.push(Action::OpenEditTable(info));
                                }
                            }
                            _ => {}
                        }
                    }
                    // Undo/redo of staged cell edits, mirroring Cmd/Ctrl+Z — a visible affordance
                    // for the keyboard shortcut, greyed out when there's nothing to step through.
                    if self.tabs[idx].edits.editable() {
                        ui.separator();
                        let (can_undo, can_redo) = {
                            let e = &self.tabs[idx].edits;
                            (e.can_undo(), e.can_redo())
                        };
                        let hint = |ui: &egui::Ui, shift: bool| {
                            let mods = if shift {
                                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
                            } else {
                                egui::Modifiers::COMMAND
                            };
                            ui.ctx()
                                .format_shortcut(&egui::KeyboardShortcut::new(mods, egui::Key::Z))
                        };
                        if components::Btn::ghost_icon(icons::undo())
                            .enabled(can_undo)
                            .show(ui)
                            .on_hover_text(format!("Undo  ({})", hint(ui, false)))
                            .clicked()
                        {
                            actions.push(Action::Undo);
                        }
                        if components::Btn::ghost_icon(icons::redo())
                            .enabled(can_redo)
                            .show(ui)
                            .on_hover_text(format!("Redo  ({})", hint(ui, true)))
                            .clicked()
                        {
                            actions.push(Action::Redo);
                        }
                    }
                    // The server-side pager lives on the right, directly under the grid.
                    self.pager(ui, actions);
                });
            });
    }

    pub(super) fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        // Saved queries is a workspace tab, not editor content. It owns the entire center so the
        // result grid and its empty/error states never compete with the saved-query list.
        if self.show_query_console
            && self.show_saved_queries
            && self.tabs[idx].kind == crate::components::QueryTabKind::Query
        {
            let kind = self.tabs[idx].kind;
            let frame = egui::Frame::central_panel(root.style())
                .inner_margin(egui::Margin::symmetric(8, 2));
            egui::CentralPanel::default()
                .frame(frame)
                .show_inside(root, |ui| {
                    self.query_workspace_bar(ui, kind, actions);
                    ui.separator();
                    self.favorites_tab(ui, actions);
                });
            return;
        }
        // The ER diagram is app-wide (per connection, not per tab) and wins over
        // everything else in the central panel while open.
        if self.erd.is_some() {
            egui::CentralPanel::default().show_inside(root, |ui| {
                self.erd_view(ui, actions);
            });
            return;
        }
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
        if self.tabs[idx].kind == crate::components::QueryTabKind::Query {
            match self.tabs[idx].view {
                TabView::Message => {
                    let message = self.tabs[idx]
                        .result
                        .as_ref()
                        .map(result_status)
                        .unwrap_or_else(|| "Run a query to see execution details".to_string());
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(root.style())
                                .inner_margin(egui::Margin::same(12)),
                        )
                        .show_inside(root, |ui| {
                            ui.label(
                                egui::RichText::new(message)
                                    .monospace()
                                    .color(palette::TEXT_WEAK()),
                            );
                        });
                    return;
                }
                TabView::Chart => {
                    egui::CentralPanel::default().show_inside(root, |ui| {
                        components::empty_state(
                            ui,
                            icons::diagram(),
                            "Chart",
                            "Chart visualization is coming soon",
                        );
                    });
                    return;
                }
                TabView::Data | TabView::Structure => {}
            }
        }
        let editable = self.tabs[idx].edits.editable();
        // Per-column FK labels for the grid's link/"Follow →" affordance (owned, so it doesn't
        // hold a borrow across the mutable tab access below).
        let fk_cols = self.fk_column_labels(idx);
        let status_msg = &self.status_msg;
        let emoji = &self.emoji;
        let tab_id = self.tabs[idx].id;
        let kind = self.tabs[idx].kind;
        let query_error = self.tabs[idx].query_error.clone();
        let loading = self.querying_tab_id == Some(tab_id);
        let QueryTab {
            result,
            row_order,
            sort,
            selection,
            edits,
            pending_scroll,
            ..
        } = &mut self.tabs[idx];
        let sort = *sort;
        egui::CentralPanel::default().show_inside(root, |ui| {
            if let Some(error) = query_error.as_deref() {
                query_error_state(ui, error);
                return;
            }
            match result.as_ref() {
                Some(result) if result.column_count() > 0 => {
                    let resp = results_grid(
                        ui,
                        result,
                        row_order,
                        sort,
                        selection,
                        edits,
                        editable,
                        tab_id,
                        pending_scroll.take(),
                        emoji,
                        &fk_cols,
                    );
                    if let Some(cmd) = resp.sort {
                        actions.push(match cmd {
                            crate::grid::SortCmd::Asc(col) => Action::SetSort { col, asc: true },
                            crate::grid::SortCmd::Desc(col) => Action::SetSort { col, asc: false },
                            crate::grid::SortCmd::Clear => Action::ClearSort,
                        });
                    }
                    if let Some(col) = resp.filter_column {
                        actions.push(Action::FilterColumn(col));
                    }
                    if let Some(click) = resp.selected {
                        selection.apply_click(click);
                    }
                    // Right-click "Copy as …": a row right-clicked while outside the selection
                    // becomes the sole target first, then the whole selection is copied.
                    if let Some((disp, fmt)) = resp.copy {
                        if !selection.contains(disp) {
                            selection.select_one(disp);
                        }
                        actions.push(Action::CopyRows(fmt));
                    }
                    // "Follow →" on a foreign-key cell: open the referenced table, filtered.
                    if let Some((row, col)) = resp.follow_fk {
                        actions.push(Action::FollowForeignKey { row, col });
                    }
                    use crate::edit::{
                        begin_cell_edit, disp_to_raw, original_value, settle_active,
                    };
                    if let Some(fill) = resp.fill {
                        settle_active(edits, result);
                        if let Some(src_raw) =
                            disp_to_raw(row_order, edits.new_rows, fill.from_disp)
                        {
                            let source = edits
                                .staged(src_raw, fill.col)
                                .cloned()
                                .or_else(|| original_value(result, src_raw, fill.col));
                            if let Some(value) = source {
                                // One undo group so the whole fill-drag takes a single Cmd/Ctrl+Z.
                                edits.begin_undo_group();
                                for disp in fill.from_disp.min(fill.to_disp)
                                    ..=fill.from_disp.max(fill.to_disp)
                                {
                                    if disp == fill.from_disp {
                                        continue;
                                    }
                                    if let Some(raw) = disp_to_raw(row_order, edits.new_rows, disp)
                                    {
                                        if edits.deleted.contains(&raw) {
                                            continue;
                                        }
                                        if let Some(orig) = original_value(result, raw, fill.col) {
                                            edits.stage(raw, fill.col, value.clone(), &orig);
                                        }
                                    }
                                }
                                edits.end_undo_group();
                                selection.select_one(fill.from_disp);
                                selection.range_to(fill.to_disp);
                                selection.set_cursor(fill.to_disp, fill.col);
                                *pending_scroll = Some(fill.to_disp);
                            }
                        }
                    }
                    if let Some(advance) = resp.commit_edit {
                        settle_active(edits, result);
                        // Tab/Shift+Tab: the commit landed → move the cursor and keep editing
                        // there (skipping bools/binary — the cursor still parks on them).
                        if edits.active.is_none() {
                            if let Some(dir) = advance {
                                let (dr, dc) = match dir {
                                    crate::edit::CursorDir::Left => (0, -1),
                                    crate::edit::CursorDir::Right => (0, 1),
                                };
                                let len = row_order.len() + edits.new_rows;
                                if selection.move_cursor(dr, dc, len, result.column_count(), false)
                                {
                                    if let Some((nd, nc)) = selection.cursor() {
                                        *pending_scroll = Some(nd);
                                        if let Some(raw) =
                                            disp_to_raw(row_order, edits.new_rows, nd)
                                        {
                                            let bytes = original_value(result, raw, nc)
                                                .is_some_and(|v| {
                                                    matches!(v, dbcore::Value::Bytes(_))
                                                });
                                            if edits.col_kind(nc) != crate::edit::EditorKind::Bool
                                                && !bytes
                                            {
                                                begin_cell_edit(edits, result, raw, nc);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if resp.cancel_edit {
                        edits.cancel_active();
                    }
                    if let Some((disp, c)) = resp.begin_edit {
                        if let Some(raw) = disp_to_raw(row_order, edits.new_rows, disp) {
                            begin_cell_edit(edits, result, raw, c);
                            // The cursor tracks the editor so Tab-advance moves relative to it.
                            selection.set_cursor(disp, c);
                        }
                    }
                    // A boolean cell flips in place rather than opening an editor. If another
                    // cell's editor is still open (e.g. the user clicked straight from it onto this
                    // bool), settle that first so its typed value isn't silently dropped.
                    if let Some((disp, c)) = resp.toggle {
                        if let Some(raw) = disp_to_raw(row_order, edits.new_rows, disp) {
                            if edits
                                .active
                                .as_ref()
                                .is_some_and(|a| (a.row, a.col) != (raw, c))
                            {
                                settle_active(edits, result);
                            }
                            if let Some(orig) = original_value(result, raw, c) {
                                edits.toggle_bool(raw, c, &orig);
                            }
                            selection.set_cursor(disp, c);
                        }
                    }
                    // Double-clicking empty table space appends a new (insert) row, selects it,
                    // and opens an editor on the first text-editable column right away.
                    if resp.add_row {
                        settle_active(edits, result);
                        let new_id = edits.add_new_row();
                        let disp = row_order.len() + edits.new_rows - 1;
                        selection.select_one(disp);
                        let first_col = (0..result.column_count())
                            .find(|&c| edits.col_kind(c) != crate::edit::EditorKind::Bool);
                        if let Some(c) = first_col {
                            edits.begin(
                                new_id,
                                c,
                                &dbcore::Value::Null,
                                crate::edit::EditOrigin::Grid,
                            );
                            selection.set_cursor(disp, c);
                        }
                    }
                }
                Some(_) => {
                    components::empty_state(ui, icons::table(), "No columns", status_msg);
                }
                None if loading => {
                    components::loading_state(ui, status_msg);
                }
                None => match kind {
                    crate::components::QueryTabKind::Query => {
                        components::empty_illustration(ui);
                    }
                    crate::components::QueryTabKind::Function
                    | crate::components::QueryTabKind::Procedure
                    | crate::components::QueryTabKind::Trigger => components::empty_state(
                        ui,
                        icons::code(),
                        "No output",
                        "This definition has not been run",
                    ),
                    crate::components::QueryTabKind::Table
                    | crate::components::QueryTabKind::View => {
                        components::empty_illustration(ui);
                    }
                },
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
                    (100.0, 95.0, 3.5, 0.45),
                    (190.0, 72.0, 4.5, 0.60),
                    (270.0, 105.0, 5.5, 0.75),
                    (55.0, 120.0, 3.0, 0.35),
                    (155.0, 115.0, 4.0, 0.50),
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
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_center_size(center, egui::vec2(sz, sz))),
                    |ui| {
                        icons::show_colored(ui, icons::database(), sz, palette::ACCENT());
                    },
                );

                // Small table icon — upper-right orbit.
                let tbl = center + egui::vec2(56.0, -54.0);
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_center_size(tbl, egui::vec2(22.0, 22.0))),
                    |ui| {
                        icons::show_colored(
                            ui,
                            icons::table(),
                            22.0,
                            palette::ACCENT().linear_multiply(0.7),
                        );
                    },
                );

                // Small key icon — lower-left orbit.
                let key = center + egui::vec2(-56.0, 52.0);
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_center_size(key, egui::vec2(20.0, 20.0))),
                    |ui| {
                        icons::show_colored(
                            ui,
                            icons::key(),
                            20.0,
                            palette::ACCENT().linear_multiply(0.55),
                        );
                    },
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
                components::section_header(ui, "Choose a theme");
                ui.add_space(10.0);
                // Snapshot (key, label) so the picker holds no borrow on `self` while we may
                // call `set_theme(&mut self, …)` right after.
                let options: Vec<(String, String)> = self
                    .themes
                    .entries()
                    .iter()
                    .map(|e| (e.key.clone(), e.name.clone()))
                    .collect();
                let mut chosen = self.theme.clone();
                for (key, label) in &options {
                    ui.radio_value(&mut chosen, key.clone(), label.as_str());
                    ui.add_space(3.0);
                }
                if chosen != self.theme {
                    self.set_theme(&ctx, chosen);
                }

                ui.add_space(30.0);

                // --- CTA ---
                if components::primary_button(ui, icons::play(), "Get Started", true).clicked() {
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

        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
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
                    components::section_header(ui, "Release notes");
                    egui::ScrollArea::vertical()
                        .id_salt("update_notes_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.label(notes.trim());
                        });
                    ui.add_space(8.0);
                }

                components::dialog_footer(ui, |ui| {
                    if ready {
                        if components::primary_button(ui, icons::save(), "Install & Restart", true)
                            .clicked()
                        {
                            install = true;
                        }
                    } else if downloading {
                        ui.add_enabled(false, egui::Button::new("Downloading…"));
                    } else if failed.is_some() {
                        if components::button(ui, icons::play(), "Retry download", true).clicked() {
                            download = true;
                        }
                    } else if components::primary_button(ui, icons::play(), "Download update", true)
                        .clicked()
                    {
                        download = true;
                    }

                    if components::button(ui, icons::close(), "Later", true).clicked() {
                        dismiss = true;
                    }
                    if components::button(ui, icons::close(), "Close", true).clicked() {
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

        components::dialog_window("What's New")
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    egui::RichText::new(format!("plusplus v{}", crate::update::CURRENT_VERSION))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(8.0);

                components::section_header(ui, "Release notes");
                egui::ScrollArea::vertical()
                    .id_salt("whats_new_notes_scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.label("• Implement query history feature with local audit log\n• Refactor dialog UI components for improved consistency and layout\n• Improve light-mode readability\n• Added \"What's New\" dialog on update");
                    });
                ui.add_space(8.0);

                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::play(), "Awesome", true).clicked() {
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
        let mut reload_themes = false;
        let mut chosen = self.theme.clone();
        // Snapshot (key, label, builtin, author) so the picker holds no borrow on `self`.
        let options: Vec<(String, String, bool, Option<String>)> = self
            .themes
            .entries()
            .iter()
            .map(|e| (e.key.clone(), e.name.clone(), e.builtin, e.author.clone()))
            .collect();
        let themes_dir = dbcore::config::themes_dir()
            .ok()
            .map(|p| p.display().to_string());

        components::dialog_window("Settings")
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                components::section_header(ui, "Appearance");
                ui.label(egui::RichText::new("Theme").color(palette::TEXT_WEAK()));
                ui.add_space(6.0);

                for (key, label, builtin, author) in &options {
                    let resp = ui.radio_value(&mut chosen, key.clone(), label.as_str());
                    let tooltip = if *builtin {
                        "Built-in theme".to_string()
                    } else {
                        match author {
                            Some(author) => format!("Custom theme · by {author}"),
                            None => "Custom theme".to_string(),
                        }
                    };
                    resp.on_hover_text(tooltip);
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if components::Btn::new("Reload themes")
                        .show(ui)
                        .on_hover_text(
                            themes_dir
                                .as_deref()
                                .map(|d| format!("Drop *.json theme files in:\n{d}"))
                                .unwrap_or_else(|| "Re-scan the themes folder".to_string()),
                        )
                        .clicked()
                    {
                        reload_themes = true;
                    }
                });

                ui.add_space(10.0);
                components::section_header(ui, "Privacy");
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
                if ui
                    .checkbox(&mut self.audit_enabled, "Record audit trail")
                    .on_hover_text(
                        "Append connections and executed statements to an append-only \
                         monthly log the app never rewrites or clears — a local record \
                         of what touched which database. SQL may contain data values.",
                    )
                    .changed()
                {
                    self.persist_settings();
                }
                if ui
                    .checkbox(
                        &mut self.update_check_enabled,
                        "Check for updates at launch",
                    )
                    .on_hover_text(
                        "Ask GitHub for the latest release when the app starts. This is \
                         the app's only network request besides your own database \
                         connections; it sends no telemetry. Takes effect next launch.",
                    )
                    .changed()
                {
                    self.persist_settings();
                }

                components::dialog_footer(ui, |ui| {
                    if components::button(ui, icons::close(), "Close", true).clicked() {
                        close = true;
                    }
                });
            });

        if reload_themes {
            self.themes.reload();
            // A previously-selected custom theme may have been removed; re-resolve so the
            // active colours and the persisted key stay valid.
            let resolved = self.themes.resolve_key(&self.theme);
            if resolved != self.theme {
                self.set_theme(ctx, resolved);
            }
        }
        if chosen != self.theme {
            self.set_theme(ctx, chosen);
        }
        if !open || close {
            actions.push(Action::CloseSettings);
        }
    }

    /// Right-hand query-history panel: every executed statement with its connection,
    /// time, duration, and outcome, newest first. Toggled from the title bar. (The
    /// append-only compliance record is separate — see `dbcore::audit`.)
    /// (Favorites live in the query bar's ★ menu, next to where queries are written.)
    pub(super) fn history_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::right("history_panel")
            .resizable(true)
            .default_size(300.0)
            .show_separator_line(true)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    components::section_header(ui, "Query History");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if components::icon_button(ui, icons::close(), "Hide history").clicked() {
                            actions.push(Action::ToggleHistory);
                        }
                        if components::icon_button(ui, icons::trash(), "Delete the entire history")
                            .clicked()
                        {
                            actions.push(Action::ClearHistory);
                        }
                    });
                });
                ui.add_space(4.0);
                self.history_list(ui, actions);
            });
    }

    /// The query-history list (newest first): each executed statement with its connection,
    /// time, duration, and outcome. Rendered inside [`Self::history_panel`].
    fn history_list(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.history_cache.is_empty() {
            ui.label(egui::RichText::new("No queries recorded yet.").color(palette::TEXT_WEAK()));
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
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if components::Btn::new("Use")
                                .show(ui)
                                .on_hover_text("Put this SQL into the active tab")
                                .clicked()
                            {
                                actions.push(Action::UseHistorySql(idx));
                            }
                            if components::Btn::new("Copy").show(ui).clicked() {
                                ui.ctx().copy_text(entry.sql.clone());
                            }
                            if components::Btn::new("Save")
                                .show(ui)
                                .on_hover_text("Save as a favorite")
                                .clicked()
                            {
                                actions.push(Action::SaveFavoriteFromHistory(idx));
                            }
                        });
                    });

                    // Line 2: when, how many rows, how long.
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(entry.at.replace('T', " ").trim_end_matches('Z'))
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
                        egui::Label::new(egui::RichText::new(detail).font(font.clone()).color(
                            if entry.ok {
                                palette::TEXT_WEAK()
                            } else {
                                palette::DANGER()
                            },
                        ))
                        .truncate(),
                    )
                    .on_hover_text(&entry.sql);
                    ui.separator();
                }
            });
    }

    /// Full-width Saved queries workspace. Selecting Use returns to the editor tab, so browsing
    /// saved SQL never competes with either the editor or its result surface.
    fn favorites_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.add_space(8.0);

        if self.favorites_cache.is_empty() {
            components::empty_state(
                ui,
                icons::star(),
                "No saved queries",
                "Write a query in the editor, then save it to reuse here",
            );
            return;
        }

        let font = egui::TextStyle::Monospace.resolve(ui.style());
        egui::ScrollArea::vertical()
            .id_salt("saved_queries_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for idx in 0..self.favorites_cache.len() {
                    let fav = &self.favorites_cache[idx];
                    let name = fav.name.clone();
                    let sql = fav.sql.clone();
                    let preview = first_line(&sql).to_string();

                    let is_renaming_this = self
                        .favorite_pending
                        .as_ref()
                        .and_then(|d| d.editing_id.as_ref())
                        .is_some_and(|id| id == &fav.id);

                    // Minimal still needs rhythm: inset the content while the separator continues
                    // edge-to-edge, so rows feel spacious without becoming decorated cards.
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if is_renaming_this {
                                    if let Some(draft) = self.favorite_pending.as_mut() {
                                        let w = (ui.available_width() - 150.0).max(180.0);
                                        let resp =
                                            components::text_input(ui, &mut draft.name, "", w);
                                        resp.request_focus();
                                        if resp.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                        {
                                            actions.push(Action::ConfirmSaveFavorite);
                                        } else if resp.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Escape))
                                        {
                                            actions.push(Action::CancelSaveFavorite);
                                        }
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new(&name).strong().color(palette::TEXT()),
                                    );
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        if is_renaming_this {
                                            if components::Btn::new("Cancel").show(ui).clicked() {
                                                actions.push(Action::CancelSaveFavorite);
                                            }
                                            if components::Btn::new("Save").show(ui).clicked() {
                                                actions.push(Action::ConfirmSaveFavorite);
                                            }
                                        } else {
                                            let dots = egui::Image::new(icons::more_vert())
                                                .fit_to_exact_size(egui::vec2(16.0, 16.0))
                                                .tint(palette::TEXT_WEAK())
                                                .alt_text("Query actions")
                                                .sense(egui::Sense::click());
                                            let menu = ui.add(dots).on_hover_text("Query actions");
                                            egui::Popup::menu(&menu)
                                                .close_behavior(
                                                    egui::PopupCloseBehavior::CloseOnClickOutside,
                                                )
                                                .show(|ui| {
                                                    ui.set_min_width(140.0);
                                                    if ui.button("Use").clicked() {
                                                        actions.push(Action::UseFavorite(idx));
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if ui.button("Rename").clicked() {
                                                        actions.push(Action::RenameFavorite(idx));
                                                        ui.close();
                                                    }
                                                    if ui.button("Copy").clicked() {
                                                        ui.ctx().copy_text(sql.clone());
                                                        ui.close();
                                                    }
                                                    if ui.button("Delete").clicked() {
                                                        actions.push(Action::DeleteFavorite(idx));
                                                        ui.close();
                                                    }
                                                });
                                        }
                                    },
                                );
                            });

                            ui.add_space(4.0);
                            // A single muted SQL line keeps scanning fast without turning each
                            // item into a decorated card. Hover still exposes the full statement.
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&preview)
                                        .font(font.clone())
                                        .color(palette::TEXT_WEAK()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&sql);
                        });
                    ui.separator();
                }
            });
    }

    /// Modal showing the SQL that will be executed, with Commit and Cancel buttons.
    /// Opened by Cmd+S; the user reviews the statements before anything is sent to the DB.
    pub(super) fn commit_preview_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(stmts) = self.commit_pending.clone() else {
            return;
        };

        let title = format!("Review {} Change(s)", stmts.len());
        let mut open = true;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 440.0])
            .frame(components::dialog_frame(ctx))
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

                components::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if components::primary_button(ui, icons::save(), "Commit", can_act)
                        .on_hover_text("Execute all statements in a single transaction")
                        .clicked()
                    {
                        actions.push(Action::ConfirmEdits);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelEdits);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelEdits);
        }
    }

    /// Small modal to name a query when saving (or renaming) a favorite. Enter or Save
    /// commits; Escape / Cancel / closing the window dismisses it.
    pub(super) fn favorite_name_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(draft) = self.favorite_pending.as_ref() else {
            return;
        };
        let is_rename = draft.editing_id.is_some();
        if is_rename {
            return; // Renaming is done inline in the Saved queries tab.
        }
        let preview = first_line(&draft.sql).to_string();
        let title = if is_rename {
            "Rename favorite"
        } else {
            "Save query to favorites"
        };

        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .default_size([440.0, 0.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Name").color(palette::TEXT_WEAK()));
                if let Some(draft) = self.favorite_pending.as_mut() {
                    let w = ui.available_width();
                    let resp = components::text_input(ui, &mut draft.name, "My query", w);
                    resp.request_focus();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                }
                ui.add_space(6.0);
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(preview)
                            .font(font)
                            .color(palette::TEXT_FAINT()),
                    )
                    .truncate(),
                );
                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::save(), "Save", true).clicked() {
                        submit = true;
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            actions.push(Action::ConfirmSaveFavorite);
        } else if cancel || !open {
            actions.push(Action::CancelSaveFavorite);
        }
    }

    /// Modal listing the destructive statements about to hit a production connection,
    /// with Run and Cancel buttons. Opened by Run when the tab's connection is marked
    /// production and the batch contains UPDATE/DELETE/DROP/TRUNCATE/ALTER.
    pub(super) fn danger_confirm_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(stmts) = self.danger_pending.clone() else {
            return;
        };

        let title = format!("Production: {} Destructive Statement(s)", stmts.len());
        let mut open = true;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([640.0, 440.0])
            .frame(components::dialog_frame(ctx))
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

                components::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if components::primary_button(ui, icons::connect(), "Run", can_act)
                        .on_hover_text("Execute against the production connection")
                        .clicked()
                    {
                        actions.push(Action::ConfirmDangerQuery);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDangerQuery);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelDangerQuery);
        }
    }

    /// Map a CSV/JSON file's columns onto the target table's, preview the result, and confirm.
    ///
    /// The dialog is built to answer three questions at a glance: *what file*, *which columns
    /// land where*, and *what is about to be written*. A leading status dot per row makes the
    /// mapped/skipped split scannable, the type badge matches the Details panel's colour
    /// language, and the preview dims the source columns nothing reads from.
    ///
    /// The source column list only ever chooses an *index*; the identifiers in the generated
    /// `INSERT` come from the table's introspected columns, so nothing in the file can reach
    /// the SQL as an identifier.
    pub(super) fn import_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(draft) = self.import_pending.as_ref() else {
            return;
        };
        let busy = self.busy;
        let target = draft.table_label();

        let binary = draft.binary_conflicts();
        let required = draft.unmapped_required();
        let mapped = draft.mapping.iter().filter(|m| m.is_some()).count();
        let can_import = busy == Busy::Idle && binary.is_empty() && mapped > 0;

        // Source columns that feed at least one target. The rest are dimmed in the preview so
        // the user can see exactly what the import ignores.
        let mut used = vec![false; draft.headers.len()];
        for src in draft.mapping.iter().flatten() {
            if let Some(slot) = used.get_mut(*src) {
                *slot = true;
            }
        }

        let mut open = true;
        components::dialog_window(format!("Import into {target}"))
            .open(&mut open)
            .resizable(true)
            // Width is fixed by design; height hugs the content and stops growing once the body
            // scroll hits its cap, so a six-column table gets a short dialog and a sixty-column
            // one doesn't run off the screen.
            .default_width(820.0)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                // Everything except the footer lives in ONE vertical scroll — the file name,
                // the header switch, the callouts, the mapping, and the preview all move
                // together. Nothing above the buttons is pinned, so a long warning or a wide
                // table never squeezes the form. `auto_shrink` vertically means a short form
                // leaves no dead space above the footer; a long one scrolls at `MAX_BODY_H`.
                const MAX_BODY_H: f32 = 520.0;
                let has_columns = !draft.headers.is_empty();

                egui::ScrollArea::vertical()
                    .id_salt("import_body_scroll")
                    .max_height(MAX_BODY_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // ── Source file ────────────────────────────────────────
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            icons::show_weak(ui, icons::code(), icons::SIZE);
                            ui.label(egui::RichText::new(draft.file_name()).strong());
                            components::type_badge(ui, draft.format.label(), palette::ACCENT());
                        });

                        // JSON objects are always keyed by name, so the switch is CSV-only.
                        if draft.format == dbcore::ImportFormat::Csv {
                            ui.add_space(4.0);
                            // `accent_checkbox` allocates the box then the label in sequence, so
                            // it needs a horizontal layout or the label drops to the next line.
                            ui.horizontal(|ui| {
                                let mut has_header = draft.has_header;
                                if components::accent_checkbox(
                                    ui,
                                    busy == Busy::Idle,
                                    &mut has_header,
                                    Some("First row is a header"),
                                )
                                .on_hover_text("Uncheck if the file's first row is already data")
                                .changed()
                                {
                                    actions.push(Action::SetImportHasHeader(has_header));
                                }
                            });
                        }

                        // ── Problems ───────────────────────────────────────────
                        if !binary.is_empty() {
                            ui.add_space(8.0);
                            components::callout(
                                ui,
                                icons::warning(),
                                &format!(
                                    "Binary columns can't be imported: {}. Set them to “Skip”.",
                                    binary.join(", ")
                                ),
                                palette::DANGER(),
                            );
                        }
                        if !required.is_empty() {
                            ui.add_space(8.0);
                            components::callout(
                                ui,
                                icons::warning(),
                                &format!(
                                    "Not null and unmapped: {}. The import will fail unless the \
                                     database supplies a default.",
                                    required.join(", ")
                                ),
                                palette::WARNING(),
                            );
                        }

                        // A file with no columns has nothing to map — say so instead of drawing
                        // two empty grids. The footer still renders, outside this scroll.
                        if !has_columns {
                            components::empty_state(
                                ui,
                                icons::table(),
                                "This file has no columns",
                                "It looks empty. Pick another file, or uncheck “First row is a \
                                 header”.",
                            );
                            return;
                        }

                        ui.add_space(10.0);

                        // ── Column mapping ─────────────────────────────────────
                        ui.horizontal(|ui| {
                            components::section_header(ui, "Column mapping");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // The body's scrollbar is drawn over the content's right
                                    // edge; in a right-to-left layout the first thing added is
                                    // the rightmost, so this reserves the gutter.
                                    ui.add_space(SCROLLBAR_GUTTER);
                                    if components::button(
                                        ui,
                                        icons::close(),
                                        "Skip all",
                                        mapped > 0,
                                    )
                                    .on_hover_text("Unmap every column")
                                    .clicked()
                                    {
                                        actions.push(Action::ClearImportMapping);
                                    }
                                    if components::button(ui, icons::redo(), "Match by name", true)
                                        .on_hover_text(
                                            "Re-match columns by name, discarding manual choices",
                                        )
                                        .clicked()
                                    {
                                        actions.push(Action::AutoMapImport);
                                    }
                                },
                            );
                        });

                        egui::Grid::new("import_mapping_grid")
                            .num_columns(4)
                            .spacing([10.0, 7.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (i, col) in draft.table.columns.iter().enumerate() {
                                    let source = draft.mapping.get(i).copied().flatten();
                                    let is_binary = dbcore::import::is_binary_type(&col.data_type);
                                    let blocked = is_binary && source.is_some();

                                    // Dot: red = blocked, accent = mapped, faint = skipped.
                                    let dot = if blocked {
                                        palette::DANGER()
                                    } else if source.is_some() {
                                        palette::ACCENT()
                                    } else {
                                        palette::TEXT_FAINT()
                                    };
                                    components::status_dot(ui, dot);

                                    let name = egui::RichText::new(&col.name);
                                    ui.label(if source.is_some() {
                                        name.strong().color(palette::TEXT())
                                    } else {
                                        name.color(palette::TEXT_WEAK())
                                    });

                                    let kind = dbcore::EditorKind::classify(&col.data_type);
                                    let badge_color = if is_binary {
                                        palette::DANGER()
                                    } else {
                                        kind_color(kind)
                                    };
                                    components::type_badge(ui, &col.data_type, badge_color);

                                    let selected = source
                                        .and_then(|s| draft.headers.get(s))
                                        .map_or("Skip", String::as_str);
                                    // `ui.available_width()` here is the row's *remaining* width,
                                    // which on a wide dialog is enormous. A picker for one column
                                    // name has no business being 700px, so cap it — the space to
                                    // its right stays empty on purpose.
                                    let combo_w = (ui.available_width() - 4.0).clamp(180.0, 320.0);
                                    egui::ComboBox::from_id_salt(("import_map", i))
                                        .width(combo_w)
                                        .selected_text(selected)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    source.is_none(),
                                                    egui::RichText::new("Skip")
                                                        .color(palette::TEXT_WEAK()),
                                                )
                                                .clicked()
                                            {
                                                actions.push(Action::SetImportMapping {
                                                    target: i,
                                                    source: None,
                                                });
                                            }
                                            for (s, header) in draft.headers.iter().enumerate() {
                                                if ui
                                                    .selectable_label(source == Some(s), header)
                                                    .clicked()
                                                {
                                                    actions.push(Action::SetImportMapping {
                                                        target: i,
                                                        source: Some(s),
                                                    });
                                                }
                                            }
                                        });
                                    ui.end_row();
                                }
                            });

                        // ── File preview ───────────────────────────────────────
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            components::section_header(ui, "File preview");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(SCROLLBAR_GUTTER);
                                    let note = if draft.more {
                                        format!("first {} rows", draft.preview_rows.len())
                                    } else {
                                        format!(
                                            "{} row{}",
                                            draft.preview_rows.len(),
                                            if draft.preview_rows.len() == 1 {
                                                ""
                                            } else {
                                                "s"
                                            }
                                        )
                                    };
                                    ui.label(
                                        egui::RichText::new(note).color(palette::TEXT_FAINT()),
                                    );
                                },
                            );
                        });

                        // Horizontal only: vertical scrolling belongs to the body above, so a
                        // wide file pans sideways without trapping the wheel.
                        egui::ScrollArea::horizontal()
                            .id_salt("import_preview_scroll")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                egui::Grid::new("import_preview_grid")
                                    .striped(true)
                                    .spacing([16.0, 4.0])
                                    .show(ui, |ui| {
                                        for (s, header) in draft.headers.iter().enumerate() {
                                            let text = egui::RichText::new(header).strong();
                                            // Unused source columns read as ignored, not missing.
                                            ui.label(if used.get(s).copied().unwrap_or(false) {
                                                text.color(palette::TEXT())
                                            } else {
                                                text.color(palette::TEXT_FAINT()).strikethrough()
                                            });
                                        }
                                        ui.end_row();

                                        for row in &draft.preview_rows {
                                            for (s, field) in row.iter().enumerate() {
                                                let dim = !used.get(s).copied().unwrap_or(false);
                                                match field {
                                                    // A JSON null, not an empty string.
                                                    None => {
                                                        ui.label(
                                                            egui::RichText::new("NULL")
                                                                .italics()
                                                                .color(palette::TEXT_FAINT()),
                                                        );
                                                    }
                                                    Some(text) => preview_cell(ui, text, dim),
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                    });

                // ── Commit ─────────────────────────────────────────────────────
                // Pinned below the scroll, so the buttons are always reachable.
                components::dialog_footer(ui, |ui| {
                    if !has_columns {
                        if components::button(ui, icons::close(), "Cancel", true).clicked() {
                            actions.push(Action::CancelImport);
                        }
                        return;
                    }
                    let label = if mapped == 0 {
                        "Import".to_string()
                    } else {
                        format!(
                            "Import {mapped} column{}",
                            if mapped == 1 { "" } else { "s" }
                        )
                    };
                    let resp = components::primary_button(ui, icons::save(), &label, can_import);
                    // Say *why* the button is dead rather than leaving the user guessing.
                    let hint = if busy != Busy::Idle {
                        "Waiting for the current operation to finish"
                    } else if !binary.is_empty() {
                        "Skip the binary columns first"
                    } else if mapped == 0 {
                        "Map at least one column first"
                    } else {
                        "Read the whole file and insert every row in one transaction"
                    };
                    if resp.on_hover_text(hint).clicked() {
                        actions.push(Action::ConfirmImport);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelImport);
                    }
                    if busy == Busy::Importing {
                        ui.add(components::spinner(14.0));
                    }
                    // The title already names the table; say what gets left behind instead.
                    let skipped = draft.table.columns.len() - mapped;
                    if skipped > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{skipped} column{} skipped",
                                if skipped == 1 { "" } else { "s" }
                            ))
                            .color(palette::TEXT_FAINT()),
                        );
                    }
                });
            });

        if !open {
            actions.push(Action::CancelImport);
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
        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
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
                        components::db_kind_combo(ui, &mut editor.config.kind, "kind", field_w);
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
                            if ui.button("Clear").clicked()
                                && editor.config.title_bar_color.take().is_some()
                            {
                                form_changed = true;
                            }
                        });
                        ui.end_row();

                        ui.label("Production");
                        form_changed |= ui
                            .checkbox(&mut editor.config.production, "Confirm destructive queries")
                            .on_hover_text(
                                "UPDATE, DELETE, DROP, TRUNCATE, ALTER, and MERGE must \
                                 be confirmed in a dialog before they run",
                            )
                            .changed();
                        ui.end_row();

                        ui.label("Read-only");
                        form_changed |= ui
                            .checkbox(&mut editor.config.read_only, "Block all writes")
                            .on_hover_text(
                                "Only reads (SELECT, SHOW, EXPLAIN, …) are allowed to \
                                 run; in-grid editing and schema changes are refused. \
                                 Where the database supports it the session itself is \
                                 opened read-only, so even writes hidden inside \
                                 functions are rejected by the server. Takes effect on \
                                 the next connect.",
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
                                    components::password_input(
                                        ui,
                                        &mut editor.password,
                                        "",
                                        field_w,
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

                            // Flag the modes that don't verify the server's identity, so the
                            // weaker choices read as a deliberate trade-off rather than a default.
                            if let Some(warning) = editor.config.ssl_mode.security_warning() {
                                ui.label("");
                                ui.label(
                                    egui::RichText::new(format!("⚠ {warning}"))
                                        .size(11.0)
                                        .color(palette::WARNING()),
                                );
                                ui.end_row();
                            }

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
                                form_changed |= components::password_input(
                                    ui,
                                    &mut editor.ssh_password,
                                    "",
                                    field_w,
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
                            ui.add(components::spinner(style::CONTROL_H));
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
                components::dialog_footer(ui, |ui| {
                    let testing = matches!(editor.test_state, ConnTestState::Testing(_));
                    if components::button(ui, icons::connect(), "Test", !testing).clicked() {
                        actions.push(Action::TestConnection);
                    }
                    if components::button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SaveConnection);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
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
        let idx = self.active_query_tab;
        match self.tabs[idx].schema_editor.as_mut() {
            Some(crate::schema::ObjectEditor::Table(editor)) => {
                table_editor_view(ui, actions, editor)
            }
            Some(crate::schema::ObjectEditor::View(editor)) => {
                view_editor_view(ui, actions, editor)
            }
            Some(crate::schema::ObjectEditor::Trigger(editor)) => {
                trigger_editor_view(ui, actions, editor)
            }
            Some(crate::schema::ObjectEditor::Routine(editor)) => {
                routine_editor_view(ui, actions, editor)
            }
            None => {}
        }
    }

    // ─── Schema DDL preview dialog ────────────────────────────────────────────

    pub(super) fn schema_preview_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(stmts) = self.schema_pending.clone() else {
            return;
        };

        let title = format!("Preview Migration — {} Statement(s)", stmts.len());
        let mut open = true;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(true)
            .default_size([660.0, 460.0])
            .frame(components::dialog_frame(ctx))
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

                components::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if components::primary_button(ui, icons::save(), "Apply Migration", can_act)
                        .on_hover_text("Execute all DDL statements in a single transaction")
                        .clicked()
                    {
                        actions.push(Action::ApplySchema);
                    }
                    if components::button(ui, icons::close(), "Back", true).clicked() {
                        actions.push(Action::CancelSchema);
                    }
                });
            });

        if !open {
            actions.push(Action::CancelSchema);
        }
    }
}

/// The header (title + Preview SQL / Cancel buttons) shared by every object editor. Returns
/// nothing; the buttons push actions directly.
fn object_editor_header(ui: &mut egui::Ui, actions: &mut Vec<Action>, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        components::section_header(ui, title);
        // Action buttons on the right of the header, where the eye lands first.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::primary_button(ui, icons::code(), "Preview SQL", true).clicked() {
                actions.push(Action::GenerateSchema);
            }
            ui.add_space(6.0);
            if components::button(ui, icons::close(), "Cancel", true).clicked() {
                actions.push(Action::CancelSchema);
            }
        });
    });
    ui.add_space(6.0);
}

/// Render the table create/edit form (columns, indexes, foreign keys) into the central panel.
fn table_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::SchemaEditor,
) {
    use crate::schema::{SchemaEditorMode, SchemaTab};
    let title = match editor.mode {
        SchemaEditorMode::NewTable => "Create Table".to_string(),
        SchemaEditorMode::EditTable => format!("Edit Table — {}", editor.table_name),
    };
    object_editor_header(ui, actions, &title);

    // Table name (only editable in NewTable mode; read-only in EditTable).
    ui.horizontal(|ui| {
        ui.label("Table name:");
        components::text_input_enabled(
            ui,
            editor.mode == SchemaEditorMode::NewTable,
            &mut editor.table_name,
            "my_table",
            200.0,
        );
        if !editor.schema_name.is_empty() || editor.mode == SchemaEditorMode::NewTable {
            ui.label("Schema:");
            components::text_input(ui, &mut editor.schema_name, "public", 120.0);
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
            SchemaTab::Indexes => schema_indexes_tab(ui, &mut editor.indexes),
            SchemaTab::ForeignKeys => schema_fk_tab(ui, &mut editor.fks),
        });
}

/// Render the view create/edit form: name/schema, an optional materialized toggle (Postgres),
/// and the defining `SELECT` as a multi-line editor.
fn view_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::ViewEditor,
) {
    use crate::schema::ObjectMode;
    let title = match editor.mode {
        ObjectMode::Create => "Create View".to_string(),
        ObjectMode::Edit => format!("Edit View — {}", editor.name),
    };
    object_editor_header(ui, actions, &title);

    ui.horizontal(|ui| {
        ui.label("View name:");
        components::text_input(ui, &mut editor.name, "my_view", 200.0);
        if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
            ui.label("Schema:");
            components::text_input(ui, &mut editor.schema_name, "public", 120.0);
        }
        // Materialized views are Postgres-only.
        if editor.db_kind == dbcore::DbKind::Postgres {
            ui.checkbox(&mut editor.materialized, "Materialized");
        }
    });
    ui.add_space(6.0);

    ui.label(
        egui::RichText::new("Defining query (the SELECT after AS)")
            .color(palette::TEXT_WEAK())
            .size(12.0),
    );
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .id_salt("view_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut editor.select_body)
                    .code_editor()
                    .desired_rows(16)
                    .desired_width(f32::INFINITY)
                    .hint_text("SELECT ..."),
            );
        });
}

/// A placeholder body for a trigger, tailored to the dialect's procedural style.
fn trigger_body_hint(kind: dbcore::DbKind) -> &'static str {
    match kind {
        dbcore::DbKind::Postgres => "BEGIN\n  -- NEW / OLD available\n  RETURN NEW;\nEND;",
        dbcore::DbKind::Sqlite => "INSERT INTO audit(msg) VALUES ('changed');",
        dbcore::DbKind::SqlServer => {
            "BEGIN\n  SET NOCOUNT ON;\n  -- inserted / deleted tables\nEND"
        }
        _ => "SET NEW.col = ...;  -- or a BEGIN ... END block",
    }
}

/// Render the dialect-adaptive trigger create/edit form. Controls a dialect can't express are
/// hidden (e.g. row/statement level off Postgres, WHEN off MySQL/SQL Server), so the same
/// editor serves all four backends.
fn trigger_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::TriggerEditor,
) {
    use crate::schema::ObjectMode;
    use dbcore::{DbKind, TriggerEvent, TriggerLevel, TriggerTiming};

    let title = match editor.mode {
        ObjectMode::Create => "Create Trigger".to_string(),
        ObjectMode::Edit => format!("Edit Trigger — {}", editor.name),
    };
    object_editor_header(ui, actions, &title);
    let kind = editor.db_kind;

    egui::ScrollArea::vertical()
        .id_salt("trigger_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name:");
                components::text_input(ui, &mut editor.name, "my_trigger", 180.0);
                if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
                    ui.label("Schema:");
                    components::text_input(ui, &mut editor.schema_name, "public", 110.0);
                }
            });
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Table:");
                let selected = if editor.table.is_empty() {
                    "select…".to_string()
                } else {
                    editor.table.clone()
                };
                egui::ComboBox::from_id_salt("trig_table")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for t in editor.tables.clone() {
                            ui.selectable_value(&mut editor.table, t.clone(), t);
                        }
                    });
            });
            ui.add_space(6.0);

            // Timing — the available options depend on the dialect.
            let timings: &[TriggerTiming] = match kind {
                DbKind::MySql | DbKind::MariaDb => &[TriggerTiming::Before, TriggerTiming::After],
                DbKind::SqlServer => &[TriggerTiming::After, TriggerTiming::InsteadOf],
                _ => TriggerTiming::ALL,
            };
            ui.horizontal(|ui| {
                ui.label("Timing:");
                for &t in timings {
                    ui.selectable_value(&mut editor.timing, t, t.label());
                }
            });
            ui.add_space(4.0);

            // Events — MySQL/SQLite fire on one (radio); Postgres/SQL Server allow several.
            let single = matches!(kind, DbKind::MySql | DbKind::MariaDb | DbKind::Sqlite);
            ui.horizontal(|ui| {
                ui.label("Events:");
                for &e in TriggerEvent::ALL {
                    let mut on = editor.has_event(e);
                    if single {
                        if ui.selectable_label(on, e.label()).clicked() {
                            editor.events = vec![e];
                        }
                    } else if ui.checkbox(&mut on, e.label()).changed() {
                        editor.set_event(e, on);
                    }
                }
            });
            if single {
                ui.label(
                    egui::RichText::new("This dialect fires on a single event.")
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
            }
            ui.add_space(4.0);

            // Row vs statement — only Postgres lets you choose; the others are fixed.
            if kind == DbKind::Postgres {
                ui.horizontal(|ui| {
                    ui.label("For each:");
                    ui.selectable_value(&mut editor.level, TriggerLevel::Row, "ROW");
                    ui.selectable_value(&mut editor.level, TriggerLevel::Statement, "STATEMENT");
                });
                ui.add_space(4.0);
            }

            // WHEN guard — Postgres & SQLite only.
            if matches!(kind, DbKind::Postgres | DbKind::Sqlite) {
                ui.horizontal(|ui| {
                    ui.label("When:");
                    components::text_input(
                        ui,
                        &mut editor.when_condition,
                        "optional: NEW.col > 0",
                        320.0,
                    );
                });
                ui.add_space(6.0);
            }

            // Body — Postgres can execute an existing function instead of an inline body.
            if kind == DbKind::Postgres {
                ui.checkbox(
                    &mut editor.pg_existing_function,
                    "Execute existing function",
                );
                if editor.pg_existing_function {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Function to execute")
                            .color(palette::TEXT_WEAK())
                            .size(12.0),
                    );
                    components::text_input(ui, &mut editor.body, "my_trigger_fn", 280.0);
                    return;
                }
                ui.label(
                    egui::RichText::new(
                        "PL/pgSQL function body (a RETURNS trigger function is generated)",
                    )
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
                );
            } else {
                ui.label(
                    egui::RichText::new("Trigger body")
                        .color(palette::TEXT_WEAK())
                        .size(12.0),
                );
            }
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut editor.body)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text(trigger_body_hint(kind)),
            );
        });
}

/// A placeholder routine body, tailored to the dialect and routine kind.
fn routine_body_hint(kind: dbcore::DbKind, is_function: bool) -> &'static str {
    use dbcore::DbKind;
    match (kind, is_function) {
        (DbKind::Postgres, _) => "BEGIN\n  RETURN ...;\nEND;",
        (DbKind::SqlServer, true) => "BEGIN\n  RETURN ...;\nEND",
        (DbKind::SqlServer, false) => "BEGIN\n  SELECT ...;\nEND",
        (_, true) => "RETURN ...;  -- or a BEGIN ... END block",
        (_, false) => "BEGIN\n  ...\nEND",
    }
}

/// Render the function/procedure create/edit form: a parameter grid plus return type,
/// language (Postgres), and body. Dialect-adaptive — the mode column is hidden for MySQL
/// functions, the language picker shows only on Postgres.
fn routine_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::RoutineEditor,
) {
    use crate::schema::{ObjectMode, ParamDraft};
    use dbcore::{DbKind, ParamMode, RoutineKind};

    let title = match editor.mode {
        ObjectMode::Create if editor.kind == RoutineKind::Function => "Create Function".to_string(),
        ObjectMode::Create => "Create Procedure".to_string(),
        ObjectMode::Edit => format!("Edit {} — {}", editor.kind.label(), editor.name),
    };
    object_editor_header(ui, actions, &title);
    let kind = editor.db_kind;
    let is_fn = editor.kind == RoutineKind::Function;

    egui::ScrollArea::vertical()
        .id_salt("routine_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Function/Procedure switch (create mode only; the kind is fixed once it exists).
            if editor.mode == ObjectMode::Create {
                ui.horizontal(|ui| {
                    ui.label("Kind:");
                    ui.selectable_value(&mut editor.kind, RoutineKind::Function, "Function");
                    ui.selectable_value(&mut editor.kind, RoutineKind::Procedure, "Procedure");
                });
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                ui.label("Name:");
                components::text_input(ui, &mut editor.name, "my_routine", 180.0);
                if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
                    ui.label("Schema:");
                    components::text_input(ui, &mut editor.schema_name, "public", 110.0);
                }
            });
            ui.add_space(4.0);

            // Return type (functions) and language (Postgres).
            if is_fn || kind == DbKind::Postgres {
                ui.horizontal(|ui| {
                    if is_fn {
                        ui.label("Returns:");
                        components::text_input(ui, &mut editor.return_type, "integer", 150.0);
                    }
                    if kind == DbKind::Postgres {
                        ui.label("Language:");
                        egui::ComboBox::from_id_salt("routine_lang")
                            .selected_text(editor.language.clone())
                            .show_ui(ui, |ui| {
                                for l in ["plpgsql", "sql"] {
                                    ui.selectable_value(&mut editor.language, l.to_string(), l);
                                }
                            });
                    }
                });
                ui.add_space(6.0);
            }

            // Parameters grid.
            ui.label(
                egui::RichText::new("Parameters")
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
            );
            ui.add_space(2.0);
            // MySQL/MariaDB functions take no parameter mode.
            let show_mode = !(matches!(kind, DbKind::MySql | DbKind::MariaDb) && is_fn);
            let mut remove: Option<usize> = None;
            for (i, p) in editor.params.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    components::text_input(ui, &mut p.name, "name", 110.0);
                    components::text_input(ui, &mut p.data_type, "type", 120.0);
                    if show_mode {
                        egui::ComboBox::from_id_salt(("pmode", i))
                            .selected_text(p.mode.label())
                            .width(82.0)
                            .show_ui(ui, |ui| {
                                for m in ParamMode::ALL {
                                    ui.selectable_value(&mut p.mode, *m, m.label());
                                }
                            });
                    }
                    components::text_input(ui, &mut p.default, "default", 100.0);
                    if components::button(ui, icons::trash(), "", true).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                editor.params.remove(i);
            }
            if components::button(ui, icons::plus(), "Add parameter", true).clicked() {
                editor.params.push(ParamDraft::new_empty());
            }
            ui.add_space(6.0);

            ui.label(
                egui::RichText::new("Body")
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
            );
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut editor.body)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text(routine_body_hint(kind, is_fn)),
            );
        });
}

/// Paint a small disclosure triangle centred at `center`, rotating from right-pointing
/// (`openness == 0`) to down-pointing (`openness == 1`) so it animates with the body.
fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, openness: f32, color: egui::Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let (s, c) = a.sin_cos();
    let rot = |p: egui::Vec2| egui::vec2(p.x * c - p.y * s, p.x * s + p.y * c);
    const R: f32 = 3.5;
    let pts = [
        egui::vec2(-R * 0.5, -R),
        egui::vec2(R * 0.9, 0.0),
        egui::vec2(-R * 0.5, R),
    ];
    let poly: Vec<egui::Pos2> = pts.iter().map(|p| center + rot(*p)).collect();
    painter.add(egui::Shape::convex_polygon(poly, color, egui::Stroke::NONE));
}

/// The expandable body of a table row in the explorer: its columns (PK marked), then any
/// indexes and foreign keys. Shared by the "Pinned" group and the main table list.
fn schema_table_body(ui: &mut egui::Ui, table: &dbcore::TableInfo) {
    for col in &table.columns {
        ui.horizontal(|ui| {
            let glyph = if col.primary_key {
                icons::key()
            } else {
                icons::column()
            };
            icons::show_weak(ui, glyph, 13.0);
            ui.add_space(2.0);
            components::truncated_label(ui, &col.name, None, false, egui::Sense::hover());
            let nn = if col.nullable { "" } else { " · not null" };
            let meta = format!("{}{nn}", col.data_type);
            components::truncated_label(ui, &meta, Some(&meta), true, egui::Sense::hover());
        });
    }
    if !table.indexes.is_empty() {
        ui.add_space(3.0);
        for idx in &table.indexes {
            ui.horizontal(|ui| {
                icons::show_weak(ui, icons::index(), 13.0);
                ui.add_space(2.0);
                let u = if idx.unique { "unique " } else { "" };
                let detail = format!("{u}{} ({})", idx.name, idx.columns.join(", "));
                components::truncated_label(ui, &detail, Some(&detail), true, egui::Sense::hover());
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
                    format!("{} · {detail} · on delete {}", fk.name, fk.on_delete)
                };
                components::truncated_label(ui, &detail, Some(&hover), true, egui::Sense::hover());
            });
        }
    }
}

/// The table actions menu (pin, edit, clone, export, truncate, drop) shared by the row's
/// right-click context menu. `pinned` selects the pin/unpin wording.
fn table_actions_menu(
    ui: &mut egui::Ui,
    table: &dbcore::TableInfo,
    pinned: bool,
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(180.0);
    let pin_label = if pinned {
        "Unpin from Top"
    } else {
        "Pin to Top"
    };
    if components::button(ui, icons::star(), pin_label, true).clicked() {
        actions.push(Action::ToggleBookmark {
            schema: table.schema.clone(),
            table: table.name.clone(),
        });
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::edit(), "Edit Table…", true).clicked() {
        actions.push(Action::OpenEditTable(table.clone()));
        ui.close();
    }
    if components::button(ui, icons::table(), "Clone Table…", true)
        .on_hover_text("Copy this table's structure and rows into a new table")
        .clicked()
    {
        actions.push(Action::CloneTable(table.clone()));
        ui.close();
    }
    let export_label = egui::Image::new(icons::save())
        .fit_to_exact_size(egui::vec2(icons::SIZE, icons::SIZE))
        .tint(ui.visuals().widgets.inactive.fg_stroke.color);
    ui.menu_button((export_label, "Export Table…"), |ui| {
        ui.set_min_width(160.0);
        for fmt in [dbcore::ExportFormat::Csv, dbcore::ExportFormat::Json] {
            if ui
                .button(format!("Export as {}…", fmt.label()))
                .on_hover_text("Stream every row of this table to a file")
                .clicked()
            {
                actions.push(Action::ExportTable {
                    table: table.clone(),
                    format: fmt,
                });
                ui.close();
            }
        }
    });
    if components::button(ui, icons::table(), "Import Data…", true)
        .on_hover_text("Load rows into this table from a CSV or JSON file")
        .clicked()
    {
        actions.push(Action::ImportIntoTable(table.clone()));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::warning(), "Truncate Table…", true)
        .on_hover_text("Remove all rows but keep the table")
        .clicked()
    {
        actions.push(Action::TruncateTable(table.clone()));
        ui.close();
    }
    if components::button(ui, icons::trash(), "Drop Table…", true)
        .on_hover_text("Delete this table and all of its data")
        .clicked()
    {
        actions.push(Action::DropTable(table.clone()));
        ui.close();
    }
}

/// A collapsible group header ("Views (3)", "Triggers (1)", …) for a class of schema objects
/// in the sidebar tree, with `body` rendering its rows. Collapsed by default to keep the tree
/// compact when a database has many objects; clicking anywhere on the header toggles it.
fn object_group(
    ui: &mut egui::Ui,
    id_key: &str,
    title: &str,
    count: usize,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{title} ({count})"))
            .color(palette::TEXT_WEAK())
            .size(12.0),
    )
    .id_salt(id_key)
    .show(ui, body);
}

const OBJECT_ROW_HEIGHT: f32 = 22.0;

/// Reserve the full height of a large object list, but build widgets only for rows intersecting
/// the outer schema scroll area's clip rectangle. This keeps expanding a group with thousands of
/// routines cheap without introducing a nested scrollbar.
fn virtualized_object_rows<T>(
    ui: &mut egui::Ui,
    items: &[T],
    mut show_row: impl FnMut(&mut egui::Ui, usize, &T),
) {
    if items.is_empty() {
        return;
    }

    let spacing = ui.spacing().item_spacing.y;
    let stride = OBJECT_ROW_HEIGHT + spacing;
    let full_height = stride * items.len() as f32 - spacing;
    let (_, full_rect) = ui.allocate_space(egui::vec2(ui.available_width().max(0.0), full_height));
    let visible = visible_object_row_range(full_rect, ui.clip_rect(), stride, items.len());
    if visible.is_empty() {
        return;
    }

    let rows_rect = egui::Rect::from_min_max(
        egui::pos2(
            full_rect.left(),
            full_rect.top() + visible.start as f32 * stride,
        ),
        egui::pos2(
            full_rect.right(),
            full_rect.top() + visible.end as f32 * stride - spacing,
        ),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rows_rect), |ui| {
        ui.skip_ahead_auto_ids(visible.start);
        for index in visible {
            show_row(ui, index, &items[index]);
        }
    });
}

fn visible_object_row_range(
    full_rect: egui::Rect,
    clip_rect: egui::Rect,
    stride: f32,
    total: usize,
) -> std::ops::Range<usize> {
    if total == 0 || stride <= 0.0 || !full_rect.intersects(clip_rect) {
        return 0..0;
    }
    let first = ((clip_rect.top() - full_rect.top()) / stride)
        .floor()
        .max(0.0) as usize;
    let end = (((clip_rect.bottom() - full_rect.top()) / stride).ceil() as usize + 1).min(total);
    first.min(end)..end
}

/// A single clickable leaf row (a routine or trigger) in the sidebar tree: an icon, the object
/// name, and `detail` shown as a hover tooltip. The full row is interactive so a click beside a
/// short name still opens the intended object.
fn object_leaf_row(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    color: egui::Color32,
    name: &str,
    detail: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(0.0), OBJECT_ROW_HEIGHT),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                icons::show_colored(ui, icon, 14.0, color);
                ui.add_space(2.0);
                components::truncated_label(ui, name, None, false, egui::Sense::hover());
            },
        );
    }
    response.on_hover_text(detail)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod object_row_tests {
    use super::visible_object_row_range;

    #[test]
    fn large_object_lists_only_build_visible_rows() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(300.0, 30_000.0));
        let top = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(300.0, 300.0));
        let middle = egui::Rect::from_min_size(egui::pos2(0.0, 15_100.0), egui::vec2(300.0, 300.0));

        assert_eq!(visible_object_row_range(full, top, 30.0, 1_024), 0..11);
        assert_eq!(
            visible_object_row_range(full, middle, 30.0, 1_024),
            500..511
        );
    }

    #[test]
    fn offscreen_object_lists_build_no_rows() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 500.0), egui::vec2(300.0, 300.0));
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0));
        assert_eq!(visible_object_row_range(full, clip, 30.0, 10), 0..0);
    }
}

/// The Structure view of a table tab: its introspected columns, indexes, and foreign keys
/// as read-only grids, styled after the results grid (TablePlus's "Structure" mode).
fn structure_view(ui: &mut egui::Ui, info: &dbcore::TableInfo) {
    use egui_extras::{Column, TableBuilder};

    let row_height = egui::TextStyle::Monospace.resolve(ui.style()).size + 8.0;
    let header = |ui: &mut egui::Ui, title: &str| {
        components::paint_table_header_cell(ui);
        ui.add(
            egui::Label::new(egui::RichText::new(title).strong().color(palette::TEXT()))
                .selectable(false),
        );
    };

    egui::ScrollArea::vertical()
        .id_salt("structure_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            components::section_header(ui, "Columns");
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
                        components::paint_table_header_cell(ui);
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
                components::section_header(ui, "Indexes");
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
                            components::paint_table_header_cell(ui);
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
                components::section_header(ui, "Foreign Keys");
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
                            components::paint_table_header_cell(ui);
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("#")
                                    .color(palette::TEXT_FAINT())
                                    .monospace(),
                            );
                        });
                        for title in [
                            "constraint_name",
                            "columns",
                            "references",
                            "on_delete",
                            "on_update",
                        ] {
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
                                    ui.label(format!("{target} ({})", fk.ref_columns.join(", ")));
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

// ─── ER diagram ──────────────────────────────────────────────────────────────

impl DbGuiApp {
    /// The ER diagram view: a pan/zoom canvas (`egui::Scene`) of draggable table boxes
    /// connected by foreign-key curves. Takes over the central panel while open.
    pub(super) fn erd_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(erd) = self.erd.as_mut() else { return };

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            icons::show_native(ui, icons::diagram(), icons::SIZE);
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!("ER Diagram — {}", erd.database))
                    .strong()
                    .color(palette::TEXT()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::icon_button(ui, icons::close(), "Close the diagram").clicked() {
                    actions.push(Action::ToggleErd);
                }
                if components::Btn::new("Refresh")
                    .show(ui)
                    .on_hover_text("Rebuild from the current schema")
                    .clicked()
                {
                    actions.push(Action::RefreshErd);
                }
                if components::Btn::new("Re-layout")
                    .show(ui)
                    .on_hover_text("Recompute the automatic arrangement")
                    .clicked()
                {
                    erd.layout();
                }
                if components::Btn::new("Fit")
                    .show(ui)
                    .on_hover_text("Zoom to fit all tables")
                    .clicked()
                {
                    erd.request_fit();
                }
                ui.add_space(6.0);
                ui.colored_label(
                    palette::TEXT_FAINT(),
                    format!("{} tables · {} relations", erd.nodes.len(), erd.edges.len()),
                );
            });
        });
        ui.add_space(2.0);
        ui.separator();

        if erd.nodes.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(palette::TEXT_FAINT(), "This database has no tables.");
            });
            return;
        }

        let mut scene_rect = erd.scene_rect;
        egui::Scene::new()
            .zoom_range(0.1..=2.5)
            .show(ui, &mut scene_rect, |ui| {
                erd_canvas(ui, erd);
            });
        erd.scene_rect = scene_rect;
    }
}

/// Draw the diagram content inside the scene: FK curves first (under), then the
/// draggable table boxes. All coordinates are scene-local; `egui::Scene` applies
/// the pan/zoom transform around us.
fn erd_canvas(ui: &mut egui::Ui, erd: &mut crate::erd::ErDiagram) {
    use crate::erd::{HEADER_H, ROW_H};

    // Node titles get one point over Body for hierarchy; Heading is avoided on purpose —
    // its custom font family only exists once the app installs fonts (not in headless tests).
    let title_font = egui::FontId::proportional(13.5);
    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let small_font = egui::TextStyle::Small.resolve(ui.style());
    let painter = ui.painter().clone();

    // Measure boxes once with real font metrics (the layout used char-count estimates).
    for node in &mut erd.nodes {
        if node.size != egui::Vec2::ZERO {
            continue;
        }
        let mut width: f32 = painter
            .layout_no_wrap(node.title.clone(), title_font.clone(), palette::TEXT())
            .size()
            .x
            + 24.0;
        for col in &node.columns {
            let name = painter.layout_no_wrap(col.name.clone(), body_font.clone(), palette::TEXT());
            let ty =
                painter.layout_no_wrap(col.data_type.clone(), small_font.clone(), palette::TEXT());
            // marker + name + gap + type + padding
            width = width.max(16.0 + name.size().x + 24.0 + ty.size().x + 12.0);
        }
        node.size = egui::vec2(
            width.clamp(170.0, 380.0),
            HEADER_H + node.columns.len() as f32 * ROW_H + 6.0,
        );
    }

    // Edges first, so the boxes draw over them.
    for edge in &erd.edges {
        let highlighted = erd.selected.is_some_and(|s| s == edge.from || s == edge.to);
        let color = if highlighted {
            palette::ACCENT()
        } else {
            palette::BORDER_STRONG()
        };
        let stroke = egui::Stroke::new(if highlighted { 2.0 } else { 1.4 }, color);

        let from_rect = erd.nodes[edge.from].rect();
        let to_rect = erd.nodes[edge.to].rect();
        let from_y = from_rect.top() + HEADER_H + (edge.from_row as f32 + 0.5) * ROW_H;
        let to_y = match edge.to_row {
            Some(row) => to_rect.top() + HEADER_H + (row as f32 + 0.5) * ROW_H,
            None => to_rect.top() + HEADER_H * 0.5,
        };

        if edge.from == edge.to {
            // Self-reference: a small loop out of the right side.
            let r = from_rect.right();
            let p0 = egui::pos2(r, from_y);
            let p1 = egui::pos2(
                r,
                to_y + if edge.to_row == Some(edge.from_row) {
                    ROW_H * 0.6
                } else {
                    0.0
                },
            );
            let reach = 46.0;
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    p0,
                    p0 + egui::vec2(reach, 0.0),
                    p1 + egui::vec2(reach, 0.0),
                    p1,
                ],
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            ));
            let out = egui::vec2(1.0, 0.0); // both ends leave through the right edge
            erd_child_mark(&painter, p0, out, edge.many, stroke);
            erd_parent_mark(&painter, p1, out, edge.optional, stroke);
            continue;
        }

        // Exit/enter on the sides that face each other.
        let from_right = to_rect.center().x >= from_rect.center().x;
        let p0 = egui::pos2(
            if from_right {
                from_rect.right()
            } else {
                from_rect.left()
            },
            from_y,
        );
        let p1 = egui::pos2(
            if from_right {
                to_rect.left()
            } else {
                to_rect.right()
            },
            to_y,
        );
        let reach = ((p1.x - p0.x).abs() * 0.5).clamp(32.0, 140.0);
        let out0 = egui::vec2(if from_right { 1.0 } else { -1.0 }, 0.0);
        let c0 = p0 + out0 * reach;
        let c1 = p1 - out0 * reach;
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, c0, c1, p1],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        ));
        erd_child_mark(&painter, p0, out0, edge.many, stroke);
        erd_parent_mark(&painter, p1, -out0, edge.optional, stroke);
    }

    // Nodes: drag to move, click to highlight a table's relations.
    let mut clicked: Option<usize> = None;
    for (i, node) in erd.nodes.iter_mut().enumerate() {
        let id = ui.id().with(("erd_node", i));
        let rect = node.rect();
        let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
        if resp.dragged() {
            node.pos += resp.drag_delta();
        }
        if resp.clicked() {
            clicked = Some(i);
        }
        let rect = node.rect(); // after the drag delta
        let selected = erd.selected == Some(i);

        let border = if selected {
            egui::Stroke::new(1.6, palette::ACCENT())
        } else if resp.hovered() {
            egui::Stroke::new(1.2, palette::BORDER_STRONG())
        } else {
            egui::Stroke::new(1.0, palette::BORDER())
        };
        painter.rect(
            rect,
            6.0,
            palette::SURFACE(),
            border,
            egui::StrokeKind::Inside,
        );
        // Header band + title.
        let header_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER_H));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.top() + HEADER_H),
                egui::pos2(rect.right(), rect.top() + HEADER_H),
            ],
            egui::Stroke::new(1.0, palette::BORDER()),
        );
        let title = painter.layout_no_wrap(node.title.clone(), title_font.clone(), palette::TEXT());
        painter.galley(
            egui::pos2(
                header_rect.left() + 10.0,
                header_rect.center().y - title.size().y / 2.0,
            ),
            title,
            palette::TEXT(),
        );

        // Column rows: a marker (PK dot / FK ring), the name, and the type right-aligned.
        for (r, col) in node.columns.iter().enumerate() {
            let y = rect.top() + HEADER_H + (r as f32 + 0.5) * ROW_H;
            let marker = egui::pos2(rect.left() + 11.0, y);
            if col.primary_key {
                painter.circle_filled(marker, 2.8, palette::ACCENT());
            } else if col.foreign_key {
                painter.circle_stroke(marker, 2.8, egui::Stroke::new(1.2, palette::ACCENT()));
            }
            let name_color = if col.primary_key {
                palette::TEXT()
            } else {
                palette::TEXT_WEAK()
            };
            let name = painter.layout_no_wrap(col.name.clone(), body_font.clone(), name_color);
            painter.galley(
                egui::pos2(rect.left() + 20.0, y - name.size().y / 2.0),
                name,
                name_color,
            );
            let ty = painter.layout_no_wrap(
                col.data_type.clone(),
                small_font.clone(),
                palette::TEXT_FAINT(),
            );
            painter.galley(
                egui::pos2(rect.right() - 8.0 - ty.size().x, y - ty.size().y / 2.0),
                ty,
                palette::TEXT_FAINT(),
            );
        }

        // The FK summary for this table, on hover.
        if resp.hovered() && !erd.edges.is_empty() {
            let details: Vec<&str> = erd
                .edges
                .iter()
                .filter(|e| e.from == i)
                .map(|e| e.detail.as_str())
                .collect();
            if !details.is_empty() {
                resp.on_hover_text(details.join("\n"));
            }
        }
    }
    if let Some(i) = clicked {
        erd.selected = if erd.selected == Some(i) {
            None
        } else {
            Some(i)
        };
    }
}

/// Crow's-foot mark at the referencing (FK) end of an edge. `p` sits on the box border
/// and `out` is the unit direction the edge leaves the box in: a three-prong foot fanning
/// into the border for "many", a single perpendicular bar for "one" (unique FK).
fn erd_child_mark(
    painter: &egui::Painter,
    p: egui::Pos2,
    out: egui::Vec2,
    many: bool,
    stroke: egui::Stroke,
) {
    let n = egui::vec2(-out.y, out.x);
    if many {
        let q = p + out * 10.0; // the point on the line the prongs fan out from
        for k in [-1.0, 0.0, 1.0] {
            painter.line_segment([q, p + n * (4.5 * k)], stroke);
        }
    } else {
        let q = p + out * 7.0;
        painter.line_segment([q + n * 4.5, q - n * 4.5], stroke);
    }
}

/// Cardinality mark at the referenced (parent) end: a double bar for "exactly one", or a
/// hollow circle plus bar for "zero or one" (nullable FK). `out` points away from the box.
fn erd_parent_mark(
    painter: &egui::Painter,
    p: egui::Pos2,
    out: egui::Vec2,
    optional: bool,
    stroke: egui::Stroke,
) {
    let n = egui::vec2(-out.y, out.x);
    let bar = |at: f32| {
        let q = p + out * at;
        painter.line_segment([q + n * 4.5, q - n * 4.5], stroke);
    };
    if optional {
        bar(6.0);
        painter.circle_stroke(p + out * 13.5, 3.2, stroke);
    } else {
        bar(6.0);
        bar(10.0);
    }
}

/// Semantic colour for a column's editor kind, used by the Details panel's type badges:
/// numbers amber, booleans green, dates/times blue, free text neutral.
/// Longest preview value shown before eliding. A `Label::truncate()` would shrink to whatever
/// width the grid cell happened to get, which collapsed timestamps to `2026-07…`; a fixed
/// character budget keeps every column legible and the layout predictable.
const IMPORT_PREVIEW_CHARS: usize = 28;

/// Width the body scroll's bar overlays on the right edge of its content. Section-header rows
/// right-align things into it, so they reserve this much.
const SCROLLBAR_GUTTER: f32 = 14.0;

/// One cell of the import dialog's file preview: elided past [`IMPORT_PREVIEW_CHARS`], with the
/// full value on hover, and dimmed when no target column reads this source column.
fn preview_cell(ui: &mut egui::Ui, text: &str, dim: bool) {
    let color = if dim {
        palette::TEXT_FAINT()
    } else {
        palette::TEXT()
    };
    let short: String = text.chars().take(IMPORT_PREVIEW_CHARS).collect();
    let elided = short.chars().count() < text.chars().count();
    let shown = if elided { format!("{short}…") } else { short };
    let resp = ui.add(
        egui::Label::new(egui::RichText::new(shown).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend),
    );
    if elided {
        resp.on_hover_text(text);
    }
}

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
            // Tab-advance is a grid affordance; in the Details panel it just commits.
            crate::edit::EditOutcome::Commit { .. } => {
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
                // The click test stays inside the arm (not a match guard) to match the sibling
                // Bool/Date arms, and because `ui.button` draws as a side effect.
                #[allow(clippy::collapsible_match)]
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
            "TEXT",
            "VARCHAR(255)",
            "INTEGER",
            "BIGINT",
            "SERIAL",
            "BIGSERIAL",
            "NUMERIC",
            "REAL",
            "DOUBLE PRECISION",
            "BOOLEAN",
            "DATE",
            "TIME",
            "TIMESTAMP",
            "TIMESTAMPTZ",
            "UUID",
            "JSONB",
            "BYTEA",
        ],
        DbKind::MySql | DbKind::MariaDb => &[
            "VARCHAR(255)",
            "TEXT",
            "INT",
            "BIGINT",
            "TINYINT",
            "DECIMAL(10,2)",
            "FLOAT",
            "DOUBLE",
            "BOOLEAN",
            "DATE",
            "DATETIME",
            "TIMESTAMP",
            "TIME",
            "JSON",
            "BLOB",
        ],
        DbKind::SqlServer => &[
            "NVARCHAR(255)",
            "NVARCHAR(MAX)",
            "INT",
            "BIGINT",
            "BIT",
            "DECIMAL(18,2)",
            "FLOAT",
            "REAL",
            "DATE",
            "DATETIME2",
            "TIME",
            "UNIQUEIDENTIFIER",
            "VARBINARY(MAX)",
        ],
        DbKind::Sqlite => &[
            "TEXT", "INTEGER", "REAL", "NUMERIC", "BLOB", "BOOLEAN", "DATE", "DATETIME",
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

    const NAME_W: f32 = 150.0;
    const TYPE_W: f32 = 132.0;
    const NULL_W: f32 = 34.0;
    const PK_W: f32 = 30.0;
    const DEFAULT_W: f32 = 108.0;
    const ACTION_W: f32 = 24.0;

    let mut to_remove: Option<usize> = None;

    ui.spacing_mut().item_spacing.x = 4.0;
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        schema_column_header(ui, "Name", NAME_W);
        schema_column_header(ui, "Type", TYPE_W);
        schema_column_header(ui, "Null", NULL_W);
        schema_column_header(ui, "Key", PK_W);
        schema_column_header(ui, "Default", DEFAULT_W);
        ui.allocate_exact_size(egui::vec2(ACTION_W, 1.0), egui::Sense::hover());
    });
    ui.add_space(3.0);

    for (i, col) in columns.iter_mut().enumerate() {
        let row_color = if col.drop {
            palette::DANGER().linear_multiply(0.12)
        } else if col.is_existing {
            palette::SURFACE().linear_multiply(0.70)
        } else {
            palette::ACCENT().linear_multiply(0.10)
        };

        let frame = egui::Frame::new()
            .fill(row_color)
            .stroke(egui::Stroke::new(1.0, palette::BORDER()))
            .corner_radius(egui::CornerRadius::same(style::radius::SM))
            .inner_margin(egui::Margin::symmetric(4, 3));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!col.drop, |ui| {
                    components::text_input(ui, &mut col.name, "column_name", NAME_W);
                });
                ui.add_enabled_ui(!col.drop, |ui| {
                    ui.spacing_mut().interact_size.y = style::CONTROL_H;
                    egui::ComboBox::from_id_salt(("schema_col_type", i))
                        .selected_text(if col.data_type.is_empty() {
                            "TEXT"
                        } else {
                            &col.data_type
                        })
                        .width(TYPE_W)
                        .show_ui(ui, |ui| {
                            for ty in db_type_options(db_kind) {
                                ui.selectable_value(&mut col.data_type, ty.to_string(), *ty);
                            }
                        });
                });
                ui.allocate_ui_with_layout(
                    egui::vec2(NULL_W, style::CONTROL_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        components::accent_checkbox(ui, !col.drop, &mut col.nullable, None);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(PK_W, style::CONTROL_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        components::accent_checkbox(ui, !col.drop, &mut col.primary_key, None);
                    },
                );
                ui.add_enabled_ui(!col.drop, |ui| {
                    components::text_input(ui, &mut col.default, "default...", DEFAULT_W);
                });

                let removable =
                    (col.is_existing || mode == SchemaEditorMode::EditTable || i > 0) && !col.drop;
                let remove_hover = if col.is_existing {
                    "Mark column for deletion"
                } else {
                    "Remove column"
                };
                let keep_hover = "Keep this column";

                ui.allocate_ui_with_layout(
                    egui::vec2(ACTION_W, style::CONTROL_H),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if col.drop {
                            if components::Btn::new("Keep")
                                .tooltip(keep_hover)
                                .show(ui)
                                .clicked()
                            {
                                col.drop = false;
                            }
                        } else {
                            let img = egui::Image::new(icons::trash())
                                .fit_to_exact_size(egui::vec2(13.0, 13.0))
                                .tint(palette::DANGER());
                            let resp = ui
                                .add_enabled(
                                    removable,
                                    egui::Button::image(img)
                                        .frame(false)
                                        .min_size(egui::vec2(20.0, 20.0)),
                                )
                                .on_hover_text(remove_hover);
                            if resp.clicked() {
                                if col.is_existing {
                                    col.drop = true;
                                } else {
                                    to_remove = Some(i);
                                }
                            }
                        }
                    },
                );
            });
        });
        ui.add_space(4.0);
    }

    if columns.is_empty() {
        let frame = egui::Frame::new()
            .fill(palette::SURFACE().linear_multiply(0.45))
            .stroke(egui::Stroke::new(1.0, palette::BORDER()))
            .corner_radius(egui::CornerRadius::same(style::radius::SM))
            .inner_margin(egui::Margin::same(12));
        frame.show(ui, |ui| {
            ui.label(
                egui::RichText::new("No columns yet")
                    .color(palette::TEXT_FAINT())
                    .size(12.0),
            );
        });
    }

    if let Some(i) = to_remove {
        columns.remove(i);
    }

    ui.add_space(6.0);
    if components::button(ui, icons::plus(), "Add Column", true).clicked() {
        columns.push(crate::schema::ColumnDraft::new_empty());
    }
}

fn schema_column_header(ui: &mut egui::Ui, label: &str, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 17.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(10.5)
                    .strong()
                    .color(palette::TEXT_FAINT()),
            );
        },
    );
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
                components::text_input_enabled(ui, !idx.drop, &mut idx.name, "index_name", 150.0);
                components::text_input_enabled(
                    ui,
                    !idx.drop,
                    &mut idx.columns_raw,
                    "col1, col2",
                    160.0,
                );
                ui.add_space(2.0);
                components::accent_checkbox(ui, !idx.drop, &mut idx.unique, Some("Unique"));

                if idx.is_existing {
                    let (label, hover) = if idx.drop {
                        ("Restore", "Keep this index")
                    } else {
                        ("Drop", "Mark index for removal")
                    };
                    if components::Btn::new(label)
                        .show(ui)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        idx.drop = !idx.drop;
                    }
                } else if components::Btn::new("✕")
                    .show(ui)
                    .on_hover_text("Remove index")
                    .clicked()
                {
                    to_remove = Some(i);
                }
            });
        });
    }

    if let Some(i) = to_remove {
        indexes.remove(i);
    }

    ui.add_space(4.0);
    if components::Btn::new("+ Add Index").show(ui).clicked() {
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
            egui::Frame::new()
                .fill(c)
                .inner_margin(egui::Margin::symmetric(4, 2))
        } else {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 2))
        };

        frame.show(ui, |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Constraint:");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.constraint_name,
                        "fk_name (optional)",
                        160.0,
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Columns:");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.columns_raw,
                        "col1, col2",
                        130.0,
                    );
                    ui.label("→");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.ref_table,
                        "ref_table",
                        110.0,
                    );
                    ui.label("(");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.ref_columns_raw,
                        "ref_col",
                        90.0,
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
                            if components::Btn::new(label)
                                .show(ui)
                                .on_hover_text(hover)
                                .clicked()
                            {
                                fk.drop = !fk.drop;
                            }
                        } else if components::Btn::new("✕")
                            .show(ui)
                            .on_hover_text("Remove FK")
                            .clicked()
                        {
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
    if components::Btn::new("+ Add Foreign Key").show(ui).clicked() {
        fks.push(crate::schema::FkDraft::new_empty());
    }
}
