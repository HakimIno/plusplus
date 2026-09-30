//! Chrome rendering and interaction.

use super::connections::connection_color_to_egui;
use super::connections::mix_color;
use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;
use crate::title_bar;

impl DbGuiApp {
    pub(in crate::app) fn top_bar(
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
                        let open_anything_shortcut = ui.ctx().format_shortcut(
                            &egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::P),
                        );
                        if components::toolbar_icon_button(
                            ui,
                            icons::search(),
                            &format!("Open Anything ({open_anything_shortcut})"),
                        )
                        .clicked()
                        {
                            self.open_open_anything();
                        }
                        if components::toolbar_icon_button(
                            ui,
                            icons::split_editor(),
                            if self.split_tab.is_some() {
                                "Close split workspace"
                            } else {
                                "Split query workspace"
                            },
                        )
                        .clicked()
                        {
                            if self.split_tab.is_some() {
                                self.close_split_workspace();
                            } else {
                                self.open_split_workspace();
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        title_bar::group_separator(ui);
                        components::layout_menu(
                            ui,
                            &mut components::LayoutChrome {
                                connections: &mut self.show_connection_tabs,
                                schema: &mut self.show_schema_panel,
                                details: &mut self.show_details_panel,
                                query: &mut self.show_query_console,
                                live_log: &mut self.show_live_log,
                            },
                        );
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
    pub(in crate::app) fn query_tab_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
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
                                let pointer = ui.ctx().pointer_interact_pos();
                                for idx in 0..self.tabs.len() {
                                    if self.tab_is_in_split_group(idx) {
                                        continue;
                                    }
                                    let selected =
                                        !self.settings_open && idx == self.active_query_tab;
                                    let label = self.tab_label(idx);
                                    let kind = self.tab_kind(idx);
                                    let db_kind = (kind == crate::components::QueryTabKind::Query)
                                        .then(|| self.tab_db_kind(idx))
                                        .flatten();
                                    let preview = self.tabs[idx].preview;
                                    // While this tab is dragged, its chip floats under the
                                    // pointer while its original slot remains as a placeholder.
                                    let drag_float_pos = match (self.tab_drag, pointer) {
                                        (Some(drag), Some(pointer))
                                            if drag.id == self.tabs[idx].id =>
                                        {
                                            Some(pointer - drag.grab_offset)
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
                                        drag_float_pos,
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
                                        self.tab_drag = Some(crate::app::TabDrag {
                                            id: self.tabs[idx].id,
                                            grab_offset: pointer.unwrap_or(resp.rect.left_top())
                                                - resp.rect.left_top(),
                                            origin_active_id: self.tabs[self.active_query_tab].id,
                                        });
                                        actions.push(Action::SelectTab(idx));
                                    }
                                    rects.push(resp.rect);
                                    ui.add_space(2.0);
                                }
                                if self.split_tab.is_none() {
                                    self.handle_tab_drag(ui, &rects, actions);
                                }
                                if self.settings_open {
                                    let resp = components::settings_tab_item(ui);
                                    if resp.close {
                                        actions.push(Action::CloseSettings);
                                    }
                                    ui.add_space(2.0);
                                }
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

    /// A split workspace owns an independent tab group in each pane. Keeping these headers
    /// inside each pane makes ownership explicit and aligns them across the divider.
    pub(in crate::app) fn split_pane_tab_bar(
        &mut self,
        root: &mut egui::Ui,
        active_idx: usize,
        right: bool,
        actions: &mut Vec<Action>,
    ) {
        let Some(active_id) = self.tabs.get(active_idx).map(|tab| tab.id) else {
            return;
        };
        let indices: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(idx, tab)| (self.split_tab_ids.contains(&tab.id) == right).then_some(idx))
            .collect();
        egui::Panel::top(egui::Id::new(("split_pane_tabs", right)))
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
                    .id_salt(("split_pane_tab_scroll", right))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for idx in indices {
                                let label = self.tab_label(idx);
                                let kind = self.tab_kind(idx);
                                let db_kind = (kind == crate::components::QueryTabKind::Query)
                                    .then(|| self.tab_db_kind(idx))
                                    .flatten();
                                let response = components::query_tab_item(
                                    ui,
                                    &label,
                                    kind,
                                    db_kind,
                                    self.tabs[idx].id == active_id,
                                    self.tabs[idx].preview,
                                    None,
                                );
                                if response.close {
                                    actions.push(Action::CloseSplitPaneTab { idx, right });
                                } else if response.pinned {
                                    actions.push(Action::PinSplitPaneTab { idx, right });
                                } else if response.clicked {
                                    actions.push(Action::SelectSplitPaneTab { idx, right });
                                }
                            }
                            if components::toolbar_icon_button(
                                ui,
                                icons::plus(),
                                if right {
                                    "New query tab in right pane"
                                } else {
                                    "New query tab in left pane"
                                },
                            )
                            .clicked()
                            {
                                actions.push(Action::NewSplitPaneTab(right));
                            }
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
            // The workspace drop target is evaluated later in this frame and needs this id
            // to decide whether the release creates a split pane.
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
        let float_center = pointer.x - drag.grab_offset.x + rects[from].width() * 0.5;
        let to = rects
            .iter()
            .enumerate()
            .filter(|(i, r)| *i != from && float_center > r.center().x)
            .count();
        if to != from {
            actions.push(Action::MoveTab { from, to });
        }
    }
}
