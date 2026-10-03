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
                            if title_bar::window_controls(ui) {
                                actions.push(Action::Quit);
                            }
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
        // A native menu (or any modal AppKit tracking loop) swallows the mouse-up, so the
        // release that normally ends a tab drag never reaches egui and the tab stays glued
        // to the pointer. A drag with no button held and no release this frame is stale.
        if self.tab_drag.is_some()
            && root.input(|i| !i.pointer.primary_down() && !i.pointer.any_released())
        {
            self.tab_drag = None;
        }
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
                                // `self.tabs` index of each chip in `rects`: the bar skips
                                // split-pane tabs and other connections' tabs.
                                let mut shown = Vec::with_capacity(self.tabs.len());
                                let pointer = ui.ctx().pointer_interact_pos();
                                // Only this connection's tabs are in the bar.
                                let in_bar: Vec<usize> = (0..self.tabs.len())
                                    .filter(|&i| {
                                        !self.tab_is_in_split_group(i)
                                            && self.tab_in_current_connection(i)
                                    })
                                    .collect();
                                for &idx in &in_bar {
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
                                    let can_close_others = in_bar.len() > 1;
                                    let can_close_right = in_bar.last().is_some_and(|&l| l > idx);
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
                                    shown.push(idx);
                                    ui.add_space(2.0);
                                }
                                if !self.is_split() {
                                    self.handle_tab_drag(ui, &rects, &shown, actions);
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
        pane: usize,
        actions: &mut Vec<Action>,
    ) {
        let Some(active_id) = self.tabs.get(active_idx).map(|tab| tab.id) else {
            return;
        };
        let indices: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(idx, tab)| {
                (tab.pane == pane && tab.conn_id == self.tabs[active_idx].conn_id).then_some(idx)
            })
            .collect();
        let bar = egui::Panel::top(egui::Id::new(("split_pane_tabs", pane)))
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
                    .id_salt(("split_pane_tab_scroll", pane))
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
                                    actions.push(Action::CloseSplitPaneTab { idx, pane });
                                } else if response.pinned {
                                    actions.push(Action::PinSplitPaneTab { idx, pane });
                                } else if response.clicked {
                                    actions.push(Action::SelectSplitPaneTab { idx, pane });
                                }
                            }
                            if components::toolbar_icon_button(
                                ui,
                                icons::plus(),
                                &format!("New query tab in pane {}", pane + 1),
                            )
                            .clicked()
                            {
                                actions.push(Action::NewSplitPaneTab(pane));
                            }
                        });
                    });
            });
        // Mark the pane that keyboard actions and Details currently follow.
        if self.is_split() && self.focused_pane == pane {
            let rect = bar.response.rect;
            root.painter().hline(
                rect.x_range(),
                rect.bottom() - 1.0,
                egui::Stroke::new(2.0_f32, palette::ACCENT()),
            );
        }
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
    fn handle_tab_drag(
        &mut self,
        ui: &egui::Ui,
        rects: &[egui::Rect],
        shown: &[usize],
        actions: &mut Vec<Action>,
    ) {
        let Some(drag) = self.tab_drag else { return };
        if !ui.input(|i| i.pointer.primary_down()) {
            // The workspace drop target is evaluated later in this frame and needs this id
            // to decide whether the release creates a split pane.
            return;
        }
        let Some(from_tab) = self.tabs.iter().position(|t| t.id == drag.id) else {
            // The dragged tab vanished (e.g. closed via shortcut mid-drag).
            self.tab_drag = None;
            return;
        };
        // Slots are counted among the chips actually on screen; `MoveTab` takes `self.tabs`
        // indices, which differ whenever another connection's tabs sit in between.
        let Some(from) = shown.iter().position(|&i| i == from_tab) else {
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
            actions.push(Action::MoveTab {
                from: from_tab,
                to: shown[to],
            });
        }
    }
}
