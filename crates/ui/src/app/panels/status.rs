//! Status rendering and interaction.

use crate::app::DbGuiApp;
use crate::icons;
use crate::style::palette;

impl DbGuiApp {
    /// Thin status strip pinned to the very bottom edge: row count / selection / errors.
    pub(in crate::app) fn status_bar(&mut self, root: &mut egui::Ui) {
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
                            icons::show_native(ui, icons::table(), 12.0);
                            ui.label(
                                egui::RichText::new(&self.status_msg)
                                    .size(11.0)
                                    .color(palette::TEXT_WEAK()),
                            );
                            if let Some(tab) = self.tabs.get(self.active_query_tab) {
                                if tab.sort.is_some() {
                                    ui.colored_label(
                                        palette::TEXT_WEAK(),
                                        "· Sorted loaded rows only",
                                    );
                                }
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
}
