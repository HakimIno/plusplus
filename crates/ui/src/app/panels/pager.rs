//! Pager rendering and interaction.

use crate::app::{Action, DbGuiApp, PageNav, MAX_FETCH_ROWS};
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

/// A compact page-window form with theme-aware labels.
fn pager_window_form(ui: &mut egui::Ui, draft: &mut PagerDraft, idle: bool) -> Option<(u64, u64)> {
    let mut limit_response = None;
    ui.spacing_mut().item_spacing.y = 4.0;
    for (index, (label, hint, value)) in [
        ("Limit", "100", &mut draft.limit),
        ("Offset", "0", &mut draft.offset),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            ui.add_space(4.0);
        }
        let label = ui.label(egui::RichText::new(label).size(11.5).color(palette::TEXT()));
        let response =
            components::text_input(ui, value, hint, ui.available_width()).labelled_by(label.id);
        if index == 0 {
            limit_response = Some(response);
        }
    }
    if draft.focus_limit && !ui.is_sizing_pass() {
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
    }
    ui.add_space(8.0);
    let button_text = ui.visuals().widgets.inactive.fg_stroke.color;
    let load = ui
        .add_enabled(
            idle && parsed.is_ok(),
            egui::Button::image_and_text(
                egui::Image::new(icons::play())
                    .fit_to_exact_size(egui::Vec2::splat(icons::SIZE))
                    .tint(button_text),
                egui::RichText::new("Load rows").color(button_text),
            )
            .min_size(egui::vec2(ui.available_width(), style::CONTROL_H)),
        )
        .clicked();
    let enter = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    if (load || enter) && idle {
        parsed.ok()
    } else {
        None
    }
}

/// How much of the view-mode bar fits. A split column can be a third of the window, so the bar
/// sheds its least essential parts instead of letting segments, the row summary and the pager
/// paint over each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::app) enum BarDensity {
    /// Labelled add button, row summary, previous/next, filter and limit/offset.
    Full,
    /// Add button and the pager's icons only; the row summary moves into the tooltip.
    Compact,
    /// Add button, filter and limit/offset only.
    Tight,
}

impl BarDensity {
    pub(in crate::app) fn for_width(width: f32) -> Self {
        if width >= 700.0 {
            Self::Full
        } else if width >= 420.0 {
            Self::Compact
        } else {
            Self::Tight
        }
    }

    /// Width the right-aligned cluster (pager and filter) claims.
    pub(in crate::app) fn right_reserved(self) -> f32 {
        match self {
            Self::Full => 290.0,
            Self::Compact => 150.0,
            Self::Tight => 80.0,
        }
    }

    /// Width of the "+ Row" style button next to the segmented control.
    pub(in crate::app) fn add_button_width(self) -> f32 {
        match self {
            Self::Full => 78.0,
            Self::Compact | Self::Tight => 36.0,
        }
    }

    /// Width for a segmented control that must leave room for the add button and the
    /// right-hand cluster. `extra` covers anything else on the left (the DDL button).
    pub(in crate::app) fn segment_width(self, bar_width: f32, preferred: f32, extra: f32) -> f32 {
        let room = bar_width - self.right_reserved() - self.add_button_width() - extra - 16.0;
        room.clamp(150.0, preferred)
    }

    pub(in crate::app) fn labelled_buttons(self) -> bool {
        self == Self::Full
    }
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
    pub(super) fn pager(&self, ui: &mut egui::Ui, density: BarDensity, actions: &mut Vec<Action>) {
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
        let idle = self.query_can_run(self.active_query_tab);
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
            if density != BarDensity::Tight && nav_button(ui, true, has_more, "Next page") {
                actions.push(Action::ForTab {
                    tab_id: tab.id,
                    action: Box::new(Action::Page(PageNav::Next)),
                });
            }
            self.result_filter_button(ui, actions);
            let pager_hint = format!(
                "Limit {} · Offset {}",
                group_digits(limit),
                group_digits(win.offset)
            );
            // Without room for the summary label it rides along in the tooltip.
            let pager_hint = if density == BarDensity::Full {
                window_hint
            } else {
                format!("{row_summary} · {window_hint}")
            };
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

            if density != BarDensity::Tight && nav_button(ui, false, !at_start, "Previous page") {
                actions.push(Action::ForTab {
                    tab_id: tab.id,
                    action: Box::new(Action::Page(PageNav::Prev)),
                });
            }
            if density == BarDensity::Full {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(row_summary)
                        .size(11.5)
                        .color(palette::TEXT_WEAK()),
                );
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
    use super::*;

    #[test]
    fn pager_form_labels_follow_every_builtin_theme() {
        fn find_text<'a>(
            shape: &'a egui::Shape,
            label: &str,
        ) -> Option<&'a egui::epaint::TextShape> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find_text(s, label)),
                _ => None,
            }
        }
        let registry = crate::theme::ThemeRegistry::load();
        for key in ["carbon", "midnight", "daylight", "blue-studio"] {
            let theme = registry.theme_of(key);
            crate::theme::set_current(theme);
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            style::apply(&ctx);
            let mut draft = PagerDraft {
                limit: "100".into(),
                offset: "250".into(),
                focus_limit: false,
            };
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(PAGER_FORM_W, 240.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    pager_window_form(ui, &mut draft, true);
                },
            );
            for label in ["Limit", "Offset"] {
                let text = output
                    .shapes
                    .iter()
                    .find_map(|s| find_text(&s.shape, label))
                    .unwrap_or_else(|| panic!("{key}: missing {label}"));
                assert!(
                    text.galley
                        .job
                        .sections
                        .iter()
                        .all(|s| s.format.color == theme.text),
                    "{key}: {label} must use the theme's text colour"
                );
            }
            for removed in [
                "Page window",
                "Rows per page",
                "Rows to skip",
                "Rows 251–350",
            ] {
                assert!(
                    !output
                        .shapes
                        .iter()
                        .any(|s| find_text(&s.shape, removed).is_some())
                );
            }
        }
    }

    #[test]
    fn pager_form_enter_only_submits_a_valid_window_when_idle() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        style::apply(&ctx);
        for (limit, idle, expected) in [
            ("100", true, Some((100, 250))),
            ("0", true, None),
            ("100", false, None),
        ] {
            let mut draft = PagerDraft {
                limit: limit.into(),
                offset: "250".into(),
                focus_limit: false,
            };
            let mut submitted = None;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ui| submitted = pager_window_form(ui, &mut draft, idle),
            );
            assert_eq!(submitted, expected);
        }
    }

    #[test]
    #[ignore = "renders page-window previews; run manually with --ignored"]
    fn preview_pager_window_themes() {
        let registry = crate::theme::ThemeRegistry::load();
        for key in ["daylight", "carbon"] {
            crate::theme::set_current(registry.theme_of(key));
            let mut setup = false;
            let mut draft = PagerDraft {
                limit: "100".into(),
                offset: "".into(),
                focus_limit: true,
            };
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(300.0, 230.0))
                .with_pixels_per_point(2.0)
                .build_ui(move |ui| {
                    if !setup {
                        egui_extras::install_image_loaders(ui.ctx());
                        style::apply(ui.ctx());
                        setup = true;
                    }
                    ui.add_space(12.0);
                    egui::Frame::popup(ui.style())
                        .fill(palette::PANEL())
                        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
                        .corner_radius(egui::CornerRadius::same(style::radius::WINDOW))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(PAGER_FORM_W);
                            pager_window_form(ui, &mut draft, true);
                        });
                });
            harness.run_steps(8);
            harness
                .render()
                .unwrap()
                .save(std::env::temp_dir().join(format!("plusplus-pager-{key}.png")))
                .unwrap();
        }
    }

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
