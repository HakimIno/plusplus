//! Activity tab: the server's sessions, with cancel / terminate.

use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;
use dbcore::activity::{self as act, StopMode};

/// Left inset inside every cell, matching the results grid.
const CELL_INSET: f32 = 8.0;
/// Header row height, matching the results grid.
const HEADER_H: f32 = 28.0;
/// Sum of the initial column widths plus room for the trailing filler.
const NATURAL_WIDTH: f32 = 150.0 + 80.0 + 120.0 + 180.0 + 140.0 + 80.0 + 160.0 + 560.0 + 40.0;

/// `83` → `1m 23s`; sub-minute stays in seconds.
fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    match total {
        0..=59 => format!("{total}s"),
        60..=3599 => format!("{}m {:02}s", total / 60, total % 60),
        _ => format!("{}h {:02}m", total / 3600, total % 3600 / 60),
    }
}

impl DbGuiApp {
    /// The Activity tab's workspace: header, filter, and a resizable, horizontally scrollable
    /// session table in the same style as the results grid.
    pub(in crate::app) fn activity_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let Some(monitor) = self.tabs[idx].activity.as_mut() else {
            return;
        };
        let mut refresh = false;

        // Poll only while this tab is the one on screen; nothing else repaints an idle app.
        if let Some(wait) = monitor.next_refresh_in() {
            if wait.is_zero() {
                refresh = true;
            } else {
                ui.ctx().request_repaint_after(wait);
            }
        }

        // ── Header ──────────────────────────────────────────────────────────
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(
                egui::RichText::new(&monitor.conn_name)
                    .strong()
                    .size(15.0)
                    .color(palette::TEXT()),
            );
            ui.label(
                egui::RichText::new(monitor.kind.label())
                    .size(11.0)
                    .color(palette::TEXT_WEAK()),
            );
            if monitor.production {
                ui.label(
                    egui::RichText::new("Production")
                        .size(11.0)
                        .color(palette::DANGER()),
                );
            }
            if monitor.read_only {
                ui.label(
                    egui::RichText::new("Read-only")
                        .size(11.0)
                        .color(palette::TEXT_WEAK()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                refresh |=
                    components::button(ui, icons::refresh(), "Refresh", !monitor.loading).clicked();
                ui.label(
                    egui::RichText::new(format!(
                        "{} sessions · {} active",
                        monitor.sessions.len(),
                        monitor.active_count()
                    ))
                    .size(11.5)
                    .color(palette::TEXT_WEAK()),
                );
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut monitor.filter)
                    .hint_text("Filter by user, database, client, SQL or id")
                    .desired_width(300.0),
            );
            components::accent_checkbox(ui, true, &mut monitor.hide_idle, Some("Hide idle"));
            ui.add_space(8.0);
            if components::accent_checkbox(
                ui,
                true,
                &mut monitor.auto_refresh,
                Some("Auto-refresh"),
            )
            .changed()
                && monitor.auto_refresh
            {
                refresh = true;
            }
        });
        ui.add_space(4.0);
        if let Some(error) = &monitor.error {
            notice(ui, icons::warning(), error, palette::DANGER());
        }
        match &monitor.notice {
            Some(Ok(text)) => notice(ui, icons::check(), text, palette::SUCCESS()),
            Some(Err(text)) => notice(ui, icons::warning(), text, palette::DANGER()),
            None => {}
        }
        ui.add_space(4.0);

        // ── Sessions ────────────────────────────────────────────────────────
        let can_cancel = act::can_cancel(monitor.kind);
        let can_stop = !monitor.read_only;
        let mut pending: Option<(String, StopMode)> = None;
        let mut confirmed: Option<(String, StopMode)> = None;
        let mut declined = false;
        let rows: Vec<act::Session> = monitor.visible().cloned().collect();
        let confirm = monitor.confirm.clone();
        let selected = monitor.selected.clone();
        let mut clicked: Option<String> = None;
        let loading = monitor.loading;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;

        // `TableBuilder` cannot scroll sideways, so keep it inside a horizontal ScrollArea at the
        // columns' natural width, the way the results grid does: narrow windows scroll instead
        // of squeezing the columns, and a column dragged wider just extends the scroll range.
        let viewport_width = ui.available_width();
        egui::ScrollArea::horizontal()
            .id_salt(("activity_hscroll", tab_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(NATURAL_WIDTH.max(viewport_width));
                ui.visuals_mut().faint_bg_color = palette::STRIPE();
                egui_extras::TableBuilder::new(ui)
                    // Per-tab id: widgets across Activity tabs never share resize/scroll memory.
                    .id_salt(("activity_grid", tab_id))
                    // Cells sense clicks so a row can be selected, as in the results grid.
                    .sense(egui::Sense::click())
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    // Actions first so they stay reachable however far the table is scrolled.
                    .column(egui_extras::Column::exact(150.0))
                    .column(column(80.0))
                    .column(column(120.0))
                    .column(column(180.0))
                    .column(column(140.0))
                    .column(column(80.0))
                    .column(column(160.0))
                    .column(column(560.0))
                    .column(egui_extras::Column::remainder().clip(true))
                    .header(HEADER_H, |mut header| {
                        for title in [
                            "", "ID", "User", "Database", "State", "Time", "Waiting", "Query",
                        ] {
                            header.col(|ui| header_cell(ui, title));
                        }
                        header.col(components::paint_table_header_cell);
                    })
                    .body(|body| {
                        body.rows(row_height, rows.len(), |mut table_row| {
                            let session = &rows[table_row.index()];
                            row(
                                &mut table_row,
                                session,
                                confirm.as_ref(),
                                selected.as_deref(),
                                &mut clicked,
                                can_cancel,
                                can_stop,
                                &mut pending,
                                &mut confirmed,
                                &mut declined,
                            );
                        });
                    });
                if rows.is_empty() {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(if loading {
                            "Loading…"
                        } else {
                            "No sessions to show."
                        })
                        .color(palette::TEXT_WEAK()),
                    );
                }
            });

        if let Some(monitor) = self.tabs[idx].activity.as_mut() {
            if let Some(request) = pending {
                monitor.confirm = Some(request);
            }
            if declined {
                monitor.confirm = None;
            }
            if clicked.is_some() {
                monitor.selected = clicked;
            }
        }
        if let Some((id, mode)) = confirmed {
            actions.push(Action::ForTab {
                tab_id,
                action: Box::new(Action::StopSession { id, mode }),
            });
        }
        if refresh {
            actions.push(Action::ForTab {
                tab_id,
                action: Box::new(Action::RefreshActivity),
            });
        }
    }
}

/// A resizable, clipped data column, like the results grid's.
fn column(width: f32) -> egui_extras::Column {
    egui_extras::Column::initial(width)
        .at_least(48.0)
        .clip(true)
        .resizable(true)
}

/// Same look as the results grid's column header: Heading font, full-strength text, centred.
fn header_cell(ui: &mut egui::Ui, title: &str) {
    components::paint_table_header_cell(ui);
    if title.is_empty() {
        return;
    }
    let cell = ui.max_rect();
    let label_rect = egui::Rect::from_min_max(
        egui::pos2(cell.left() + CELL_INSET, cell.top()),
        egui::pos2(cell.right() - CELL_INSET, cell.bottom()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(label_rect)
            .layout(egui::Layout::centered_and_justified(
                egui::Direction::LeftToRight,
            )),
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .font(egui::TextStyle::Heading.resolve(ui.style()))
                        .color(palette::TEXT()),
                )
                .truncate()
                .halign(egui::Align::Center)
                .selectable(false),
            );
        },
    );
}

fn notice(ui: &mut egui::Ui, icon: egui::ImageSource<'static>, text: &str, color: egui::Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add(
            egui::Image::new(icon)
                .fit_to_exact_size(egui::vec2(14.0, 14.0))
                .tint(color),
        );
        ui.label(egui::RichText::new(text).color(color));
    });
}

/// One table row, drawn like a results-grid row: full-strength text, numbers right-aligned,
/// click to select. Stopping takes two clicks: the button turns into "Confirm" / "Keep".
/// The accent marks a running session and is the only hue used.
#[allow(clippy::too_many_arguments)]
fn row(
    row: &mut egui_extras::TableRow<'_, '_>,
    session: &act::Session,
    confirm: Option<&(String, StopMode)>,
    selected: Option<&str>,
    clicked: &mut Option<String>,
    can_cancel: bool,
    can_stop: bool,
    pending: &mut Option<(String, StopMode)>,
    confirmed: &mut Option<(String, StopMode)>,
    declined: &mut bool,
) {
    let active = session.is_active();
    let text = |s: &str, color: egui::Color32| egui::RichText::new(s).color(color);
    let one_line = session.sql.split_whitespace().collect::<Vec<_>>().join(" ");
    row.set_selected(selected == Some(session.id.as_str()));

    row.col(|ui| {
        ui.add_space(CELL_INSET);
        if !can_stop {
            return;
        }
        match confirm {
            Some((id, mode)) if *id == session.id => {
                let tip = match mode {
                    StopMode::Cancel => "Stop this session's running query",
                    StopMode::Terminate => "Close this session's connection",
                };
                if components::Btn::danger("Confirm")
                    .tooltip(tip)
                    .show(ui)
                    .clicked()
                {
                    *confirmed = Some((id.clone(), *mode));
                }
                if components::Btn::new("Keep").show(ui).clicked() {
                    *declined = true;
                }
            }
            _ => {
                if can_cancel
                    && active
                    && components::Btn::new("Cancel")
                        .variant(components::ButtonVariant::Ghost)
                        .tooltip("Stop the running query, keep the connection")
                        .show(ui)
                        .clicked()
                {
                    *pending = Some((session.id.clone(), StopMode::Cancel));
                }
                if components::Btn::new("Kill")
                    .variant(components::ButtonVariant::Ghost)
                    .tooltip("Close the connection and roll back what it had open")
                    .show(ui)
                    .clicked()
                {
                    *pending = Some((session.id.clone(), StopMode::Terminate));
                }
            }
        }
    });
    // Numbers end right so their digits line up, as in the grid.
    row.col(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(CELL_INSET);
            ui.add(
                egui::Label::new(text(&session.id, palette::TEXT()).monospace()).selectable(false),
            );
        });
    });
    row.col(|ui| {
        ui.add_space(CELL_INSET);
        ui.add(
            egui::Label::new(text(&session.user, palette::TEXT()))
                .truncate()
                .selectable(false),
        );
    });
    row.col(|ui| {
        ui.add_space(CELL_INSET);
        ui.add(
            egui::Label::new(text(&session.database, palette::TEXT()))
                .truncate()
                .selectable(false),
        )
        .on_hover_text(&session.client);
    });
    row.col(|ui| {
        ui.add_space(CELL_INSET);
        let color = if active {
            palette::ACCENT()
        } else {
            palette::TEXT_WEAK()
        };
        ui.add(
            egui::Label::new(text(&session.state, color))
                .truncate()
                .selectable(false),
        );
    });
    row.col(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(CELL_INSET);
            ui.add(
                egui::Label::new(text(&format_duration(session.seconds), palette::TEXT()))
                    .selectable(false),
            );
        });
    });
    // Empty for most rows, like a NULL cell in the grid.
    row.col(|ui| {
        ui.add_space(CELL_INSET);
        ui.add(
            egui::Label::new(text(&session.waiting, palette::TEXT_FAINT()))
                .truncate()
                .selectable(false),
        );
    });
    row.col(|ui| {
        ui.add_space(CELL_INSET);
        let label = ui.add(
            egui::Label::new(text(&one_line, palette::TEXT()).monospace())
                .truncate()
                .selectable(false),
        );
        if !session.sql.is_empty() {
            label.on_hover_text(session.sql.chars().take(2000).collect::<String>());
        }
    });
    row.col(|_| {});
    if row.response().clicked() {
        *clicked = Some(session.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(0.4), "0s");
        assert_eq!(format_duration(83.0), "1m 23s");
        assert_eq!(format_duration(7260.0), "2h 01m");
        assert_eq!(format_duration(-5.0), "0s");
    }
}
