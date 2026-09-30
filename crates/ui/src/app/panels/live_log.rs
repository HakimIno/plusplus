//! Live log rendering and interaction.

use crate::app::{Action, DbGuiApp, QueryEditorPlacement};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

pub(super) fn live_log_clock(timestamp: &str) -> &str {
    timestamp
        .split_once('T')
        .map(|(_, clock)| clock.trim_end_matches('Z'))
        .unwrap_or(timestamp)
}

const LIVE_LOG_HEADER_H: f32 = 32.0;

const LIVE_LOG_DEFAULT_H: f32 = 124.0;

pub(in crate::app) fn live_log_max_size(available: f32) -> f32 {
    // Let the log consume almost the whole workspace when the user needs to inspect a long
    // trace, while retaining a small usable strip of the primary surface.
    (available - 96.0).max(LIVE_LOG_HEADER_H)
}

impl DbGuiApp {
    /// Session-only stream of statements completed for the connection bound to this tab.
    /// It shares the SQL console instead of becoming another workspace/sidebar destination:
    /// the editor answers "what will run", while this panel answers "what just ran".
    pub(in crate::app) fn live_log_panel(
        &mut self,
        root: &mut egui::Ui,
        tab_id: u64,
        include_mode_bar: bool,
        actions: &mut Vec<Action>,
    ) {
        let conn_id = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.conn_id.clone());
        let visible = |entry: &&dbcore::history::HistoryEntry| {
            conn_id
                .as_deref()
                .is_none_or(|id| entry.conn_id.as_str() == id)
        };
        let count = self.live_log.iter().filter(visible).count();
        let available = root.available_height();
        // On a compact bottom-docked table console, default to the title strip so the SQL
        // editor keeps enough room to type. A generously-sized console opens the stream.
        let log_default_size = if available >= 220.0 {
            LIVE_LOG_DEFAULT_H
        } else {
            LIVE_LOG_HEADER_H
        };
        let mode_bar_height = if include_mode_bar { 38.0 } else { 0.0 };
        let default_size = log_default_size + mode_bar_height;
        let min_size = LIVE_LOG_HEADER_H + mode_bar_height;
        let max_size = live_log_max_size(available).max(min_size);

        let panel_response =
            egui::Panel::bottom(egui::Id::new(("live_log", tab_id, include_mode_bar)))
                .resizable(true)
                .default_size(default_size)
                .min_size(min_size)
                .max_size(max_size)
                .frame(style::workspace_frame(palette::PANEL()))
                .show_separator_line(false)
                .show_inside(root, |ui| {
                    if include_mode_bar {
                        self.view_mode_bar(ui, QueryEditorPlacement::Top, true, actions);
                    }
                    let mut clear = false;
                    let mut close = false;
                    let (header_rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), LIVE_LOG_HEADER_H),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(
                        header_rect,
                        egui::CornerRadius::ZERO,
                        palette::PANEL(),
                    );
                    ui.painter().hline(
                        header_rect.x_range(),
                        header_rect.bottom(),
                        egui::Stroke::new(1.0_f32, palette::BORDER()),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(header_rect.shrink2(egui::vec2(8.0, 2.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            icons::show_colored(ui, icons::history(), 14.0, palette::ACCENT());
                            ui.label(egui::RichText::new("Live log").strong().size(12.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if components::Btn::ghost_icon(icons::close())
                                        .tooltip("Close Live log")
                                        .show(ui)
                                        .clicked()
                                    {
                                        close = true;
                                    }
                                    if components::Btn::ghost_icon(icons::trash())
                                        .enabled(count > 0)
                                        .tooltip("Clear this connection's live log")
                                        .show(ui)
                                        .clicked()
                                    {
                                        clear = true;
                                    }
                                },
                            );
                        },
                    );

                    if close {
                        self.show_live_log = false;
                        return;
                    }
                    if clear {
                        if let Some(conn_id) = &conn_id {
                            self.live_log.retain(|entry| &entry.conn_id != conn_id);
                        } else {
                            self.live_log.clear();
                        }
                    }
                    if ui.available_height() < 8.0 {
                        return;
                    }

                    // Keep the dock chrome consistent with the other workspace panels while
                    // retaining a dark code well behind timestamps and highlighted SQL.
                    ui.painter().rect_filled(
                        ui.available_rect_before_wrap(),
                        egui::CornerRadius::ZERO,
                        palette::CODE_BG(),
                    );

                    let font = egui::FontId::new(11.5, egui::FontFamily::Monospace);
                    egui::ScrollArea::vertical()
                        .id_salt(("live_log_scroll", tab_id))
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let mut entries = self.live_log.iter().filter(visible).peekable();
                            if entries.peek().is_none() {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("Run a query to see it here")
                                                .small()
                                                .color(palette::TEXT_FAINT()),
                                        );
                                    });
                                return;
                            }

                            while let Some(entry) = entries.next() {
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::symmetric(10, 6))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let status = if entry.ok { "--" } else { "-- error" };
                                            let color = if entry.ok {
                                                palette::SUCCESS()
                                            } else {
                                                palette::DANGER()
                                            };
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{status} {}",
                                                    live_log_clock(&entry.at)
                                                ))
                                                .font(font.clone())
                                                .color(color),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let rows = entry
                                                        .rows
                                                        .map(|rows| format!("{rows} rows · "))
                                                        .unwrap_or_default();
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{rows}{:.0} ms",
                                                            entry.elapsed_ms
                                                        ))
                                                        .small()
                                                        .color(palette::TEXT_FAINT()),
                                                    );
                                                },
                                            );
                                        });

                                        let mut job = crate::highlight::highlight_sql_cached(
                                            ui.ctx(),
                                            entry.sql.trim(),
                                            font.clone(),
                                        );
                                        job.wrap.max_width = ui.available_width().max(40.0);
                                        ui.add(egui::Label::new(job).wrap())
                                            .on_hover_text(entry.sql.trim());
                                        if let Some(error) = &entry.error {
                                            ui.label(
                                                egui::RichText::new(format!("Error: {error}"))
                                                    .small()
                                                    .color(palette::DANGER()),
                                            );
                                        }
                                    });
                                if entries.peek().is_some() {
                                    ui.separator();
                                }
                            }
                        });
                });
        panel_response.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Panel, true, "Live log dock")
        });
        style::workspace_resize_grip(
            root,
            egui::Id::new(("live_log", tab_id, include_mode_bar)),
            true,
        );
    }
}
