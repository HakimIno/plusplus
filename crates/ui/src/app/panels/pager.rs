//! Pager rendering and interaction.

use crate::app::{Action, Busy, DbGuiApp, PageNav, MAX_FETCH_ROWS};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

/// Group a number's digits with commas (`1234567` → `"1,234,567"`) for the pager.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[derive(Clone)]
struct PagerDraft {
    limit: String,
    offset: String,
    focus_limit: bool,
}

fn parse_pager_window(limit: &str, offset: &str) -> Result<(u64, u64), &'static str> {
    let parse = |value: &str| value.trim().replace([',', '_'], "").parse::<u64>();
    let limit = parse(limit).map_err(|_| "Enter a valid limit.")?;
    if limit == 0 {
        return Err("Limit must be at least 1.");
    }
    if limit > MAX_FETCH_ROWS as u64 {
        return Err("Limit can be at most 100,000.");
    }
    let offset = if offset.trim().is_empty() {
        0
    } else {
        parse(offset).map_err(|_| "Enter a valid offset.")?
    };
    Ok((limit, offset))
}

impl DbGuiApp {
    /// Result filter toggle, styled like the pager keys. Lives next to page navigation
    /// rather than in the window title bar. `active` follows the open filter strip.
    pub(super) fn result_filter_button(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.tab().result.is_none() {
            return;
        }
        let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::F,
        ));
        let hint = format!("Filter results  ({shortcut})");
        if components::soft_icon_button_state(
            ui,
            icons::filter(),
            &hint,
            true,
            self.tab().filter.visible,
        )
        .clicked()
        {
            actions.push(Action::ToggleFilter(self.tab().id));
        }
    }

    /// Server-side pager, right-aligned in the view-mode bar. The centre control opens a
    /// compact Limit/Offset popover; values remain local until Go/Enter so editing never
    /// fires a query per keystroke. Page flips re-run only the requested server-side window.
    pub(super) fn pager(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let tab = self.tab();
        let show_filter = tab.result.is_some();
        let page = tab
            .result
            .is_some()
            .then(|| dbcore::parse_page_window(&tab.sql))
            .flatten()
            .and_then(|win| {
                let limit = win.limit.filter(|&l| l > 0)?;
                (tab.edits.source.is_some() || tab.edits.pending_source.is_some())
                    .then_some((win, limit))
            });
        if page.is_none() && !show_filter {
            return;
        }
        let Some((win, limit)) = page else {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                self.result_filter_button(ui, actions);
            });
            return;
        };
        let idle = self.busy == Busy::Idle;
        let at_start = win.offset == 0;
        let loaded = tab
            .result
            .as_ref()
            .map_or(0_u64, |result| result.row_count() as u64);
        let has_more = tab.total_rows.map_or(!tab.page_exhausted, |total| {
            win.offset.saturating_add(loaded) < total
        });
        let row_summary = if loaded == 0 {
            match tab.total_rows {
                Some(total) => format!("0 of {} rows", group_digits(total)),
                None if tab.total_rows_pending => "0 of … rows".to_string(),
                None => "0 rows".to_string(),
            }
        } else {
            let first = win.offset.saturating_add(1);
            let last = win.offset.saturating_add(loaded);
            let total = match tab.total_rows {
                Some(total) => group_digits(total),
                None if tab.total_rows_pending => "…".to_string(),
                None if has_more => format!("{}+", group_digits(last)),
                None => group_digits(last),
            };
            format!(
                "{}–{} of {total} rows",
                group_digits(first),
                group_digits(last)
            )
        };

        let nav_button = |ui: &mut egui::Ui, right: bool, enabled: bool, hint: &str| {
            let src = if right {
                icons::chevron_right()
            } else {
                icons::chevron_left()
            };
            components::soft_icon_button(ui, src, hint, enabled && idle).clicked()
        };

        // Right-to-left puts Next at the outer edge: ‹ filter ›, matching the familiar
        // pager cluster. Filter sits between the arrows so it lives with the result, not
        // the window title bar.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            if nav_button(ui, true, has_more, "Next page") {
                actions.push(Action::Page(PageNav::Next));
            }
            self.result_filter_button(ui, actions);
            let pager_hint = format!(
                "Limit {} · Offset {}",
                group_digits(limit),
                group_digits(win.offset)
            );
            let pager_button = components::soft_icon_button(ui, icons::pager(), &pager_hint, idle);
            let popup_id = pager_button.id.with("window");
            let draft_id = popup_id.with("draft");
            if pager_button.clicked() {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        draft_id,
                        PagerDraft {
                            limit: limit.to_string(),
                            offset: if win.offset == 0 {
                                String::new()
                            } else {
                                win.offset.to_string()
                            },
                            focus_limit: true,
                        },
                    );
                });
            }

            let popup_frame = egui::Frame::popup(ui.style())
                .fill(palette::PANEL())
                .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
                .corner_radius(egui::CornerRadius::same(14))
                .inner_margin(egui::Margin::same(10));
            let popup = egui::Popup::from_toggle_button_response(&pager_button)
                .id(popup_id)
                .align(egui::RectAlign::TOP)
                .align_alternatives(&[])
                .gap(9.0)
                .width(180.0)
                .frame(popup_frame)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .layout(egui::Layout::top_down(egui::Align::Min))
                .show(|ui| {
                    ui.set_width(160.0);
                    let mut draft = ui.ctx().data_mut(|data| {
                        data.get_temp::<PagerDraft>(draft_id).unwrap_or(PagerDraft {
                            limit: limit.to_string(),
                            offset: win.offset.to_string(),
                            focus_limit: false,
                        })
                    });

                    let mut limit_response = None;
                    egui::Grid::new(popup_id.with("fields"))
                        .num_columns(2)
                        .spacing(egui::vec2(8.0, 6.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Limit").strong());
                            limit_response = Some(components::text_input(
                                ui,
                                &mut draft.limit,
                                "Rows to load",
                                110.0,
                            ));
                            ui.end_row();
                            ui.label(egui::RichText::new("Offset").strong());
                            components::text_input(ui, &mut draft.offset, "0", 110.0);
                            ui.end_row();
                        });
                    if draft.focus_limit {
                        if let Some(response) = limit_response {
                            response.request_focus();
                        }
                        draft.focus_limit = false;
                    }

                    let parsed = parse_pager_window(&draft.limit, &draft.offset);
                    if let Err(message) = parsed {
                        ui.label(
                            egui::RichText::new(message)
                                .size(10.5)
                                .color(palette::DANGER()),
                        );
                    } else {
                        ui.add_space(2.0);
                    }
                    ui.add_space(4.0);
                    let go = ui
                        .add_enabled(
                            idle && parsed.is_ok(),
                            egui::Button::new(
                                egui::RichText::new("Go").strong().color(palette::TEXT()),
                            )
                            .corner_radius(egui::CornerRadius::same(8))
                            .min_size(egui::vec2(ui.available_width(), style::CONTROL_H)),
                        )
                        .clicked();
                    let enter = ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    });
                    let submit = if (go || enter) && idle {
                        parsed.ok()
                    } else {
                        None
                    };
                    ui.ctx().data_mut(|data| data.insert_temp(draft_id, draft));
                    if submit.is_some() {
                        ui.close();
                    }
                    submit
                });
            if let Some(response) = popup {
                let rect = response.response.rect;
                let anchor_x = pager_button
                    .rect
                    .center()
                    .x
                    .clamp(rect.left() + 10.0, rect.right() - 10.0);
                let left = egui::pos2(anchor_x - 8.0, rect.bottom() - 1.0);
                let right = egui::pos2(anchor_x + 8.0, rect.bottom() - 1.0);
                let tip = egui::pos2(anchor_x, rect.bottom() + 8.0);
                let painter = ui.ctx().layer_painter(response.response.layer_id);
                painter.add(egui::Shape::convex_polygon(
                    vec![left, right, tip],
                    palette::PANEL(),
                    egui::Stroke::NONE,
                ));
                let stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG());
                painter.line_segment([left, tip], stroke);
                painter.line_segment([tip, right], stroke);
                if let Some((limit, offset)) = response.inner {
                    actions.push(Action::SetPageWindow { limit, offset });
                }
            }

            if nav_button(ui, false, !at_start, "Previous page") {
                actions.push(Action::Page(PageNav::Prev));
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(row_summary)
                    .size(11.5)
                    .color(palette::TEXT_WEAK()),
            );
        });
    }
}

#[cfg(test)]
mod pager_tests {
    use super::parse_pager_window;

    #[test]
    fn pager_window_accepts_blank_offset_and_grouped_numbers() {
        assert_eq!(parse_pager_window("75,000", ""), Ok((75_000, 0)));
        assert_eq!(parse_pager_window("1_000", "250"), Ok((1_000, 250)));
    }

    #[test]
    fn pager_window_rejects_invalid_or_oversized_limits() {
        assert!(parse_pager_window("", "0").is_err());
        assert!(parse_pager_window("0", "0").is_err());
        assert!(parse_pager_window("100001", "0").is_err());
        assert!(parse_pager_window("100", "-1").is_err());
    }
}
