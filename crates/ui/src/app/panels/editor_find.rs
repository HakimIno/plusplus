//! Editor find rendering and interaction.

use super::editor_cursors::highlight_shapes;
use crate::app::DbGuiApp;
use crate::style::palette;

/// Height of one row of the editor's find widget.
const FIND_FIELD_H: f32 = 24.0;

/// Width of the find widget's query / replacement fields.
const FIND_FIELD_W: f32 = 240.0;

/// Widest the match-count slot gets before its text is truncated.
const FIND_COUNT_MAX_W: f32 = 80.0;

/// Side of the find widget's square icon buttons.
const FIND_BUTTON: f32 = 22.0;

/// A find-widget text field: a rounded box (accent border while focused, danger while the
/// query is invalid) holding a frameless one-line editor and, at its right, `trailing`
/// controls such as the option toggles.
fn find_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    invalid: bool,
    trailing: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let focused = ui.memory(|m| m.has_focus(id));
    let border = if invalid {
        palette::DANGER()
    } else if focused {
        palette::ACCENT()
    } else {
        palette::BORDER()
    };
    egui::Frame::new()
        .fill(palette::CODE_BG())
        .stroke(egui::Stroke::new(1.0_f32, border))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin {
            left: 6,
            right: 2,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(FIND_FIELD_W - 8.0);
            ui.set_height(FIND_FIELD_H - 2.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                trailing(ui);
                ui.add(
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .hint_text(hint)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .vertical_align(egui::Align::Center)
                        .desired_width(ui.available_width()),
                )
            })
            .inner
        })
        .inner
}

/// An option toggle inside the find field (`Aa`, `ab`, `.*`): accent-tinted and outlined
/// while on.
fn find_toggle(
    ui: &mut egui::Ui,
    on: bool,
    label: &str,
    underline: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect(
            rect,
            egui::CornerRadius::same(3),
            palette::ACCENT().gamma_multiply(0.22),
            egui::Stroke::new(1.0_f32, palette::ACCENT()),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(3), palette::SURFACE_HOVER());
    }
    let color = if on {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(12.0), color);
    let pos = rect.center() - galley.size() / 2.0;
    let text_rect = egui::Rect::from_min_size(pos, galley.size());
    painter.galley(pos, galley, color);
    if underline {
        painter.hline(
            text_rect.x_range(),
            text_rect.bottom() - 1.0,
            egui::Stroke::new(1.0_f32, color),
        );
    }
    resp.on_hover_text(tooltip)
}

/// A square icon button in the find widget; dimmed and inert while `enabled` is false.
fn find_icon_button(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    size: egui::Vec2,
    enabled: bool,
    tooltip: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), palette::SURFACE_HOVER());
    }
    let tint = if enabled {
        palette::TEXT_WEAK()
    } else {
        palette::TEXT_FAINT()
    };
    let side = 16.0_f32.min(rect.width()).min(rect.height());
    egui::Image::new(icon).tint(tint).paint_at(
        ui,
        egui::Rect::from_center_size(rect.center(), egui::vec2(side, side)),
    );
    resp.on_hover_text(tooltip)
}

/// Select all of `text` in the one-line editor `id` (the find query when the widget opens).
fn select_all_text(ctx: &egui::Context, id: egui::Id, text: &str) {
    if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            )));
        state.store(ctx, id);
    }
}

impl DbGuiApp {
    /// Open the SQL editor's find widget (with the replace row for Cmd/Ctrl+H) and focus its
    /// query. A one-line selection in the editor becomes the query, as in VS Code.
    pub(in crate::app) fn open_find(&mut self, ctx: &egui::Context, with_replace: bool) {
        let tab_id = self.tab().id;
        let editor_focused =
            ctx.memory(|m| m.focused()) == Some(egui::Id::new(("sql_editor", tab_id, "primary")));
        let tab = self.tab_mut();
        if editor_focused {
            let range = tab.primary_cursor.clone();
            let selected: String = tab
                .sql
                .chars()
                .skip(range.start)
                .take(range.end.saturating_sub(range.start))
                .collect();
            if !selected.is_empty() && !selected.contains('\n') && selected.chars().count() <= 200 {
                tab.find.query = selected;
            }
        }
        tab.find.open = true;
        tab.find.focus_pending = true;
        if with_replace {
            tab.find.replace_open = true;
        }
    }

    /// The editor's find/replace widget, floating at the editor's top-right (VS Code style):
    /// a chevron that shows the replace row, the query field with its Aa / ab / .* toggles,
    /// the match count, previous/next and close; then the replacement field with replace and
    /// replace-all. Enter / Shift+Enter step through matches, Cmd/Ctrl+G too. Match ranges
    /// are char offsets, so Thai and other multi-byte text select and replace correctly.
    pub(super) fn editor_find_bar(&mut self, ui: &mut egui::Ui, editor_id: egui::Id) {
        let idx = self.active_query_tab;
        let find_id = editor_id.with("find_query");
        let replace_id = editor_id.with("find_replace");
        let ctx = ui.ctx().clone();
        let anchor = ui.max_rect();

        // Keys, consumed before the fields are built: a one-line TextEdit gives up focus on
        // Enter, which would read as leaving the widget.
        let (in_find, in_replace) = ctx.memory(|m| (m.has_focus(find_id), m.has_focus(replace_id)));
        let (mut next, mut previous, mut replace, mut replace_all, mut close) =
            (false, false, false, false, false);
        ctx.input_mut(|i| {
            if in_find {
                previous |= i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
                next |= i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            }
            if in_replace {
                replace |= i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            }
            previous |= i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::G,
            );
            next |= i.consume_key(egui::Modifiers::COMMAND, egui::Key::G);
            if in_find || in_replace || self.tabs[idx].find.had_focus {
                close |= i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            }
        });

        let tab = &mut self.tabs[idx];
        let before = (tab.find.query.clone(), tab.find.options);
        tab.find.refresh(&tab.sql, tab.sql_revision);
        let count = tab.find.found().len();
        let invalid = tab.find.error().is_some();
        let current = tab.find.current.min(count.saturating_sub(1));
        let status = if invalid {
            "Invalid regex".to_string()
        } else if tab.find.query.is_empty() {
            String::new()
        } else if count == 0 {
            "No results".to_string()
        } else if count >= crate::editor_tools::MAX_MATCHES {
            format!("{} of {}+", current + 1, count)
        } else {
            format!("{} of {count}", current + 1)
        };
        let mut toggle_replace = false;

        egui::Area::new(editor_id.with("find_widget"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(anchor.right_top() + egui::vec2(-18.0, 6.0))
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(palette::SURFACE())
                    .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(4))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 14,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(70),
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                        let find = &mut self.tabs[idx].find;
                        // The count slot hugs its text, sized for "N of N" at the current
                        // total so stepping through matches never shifts the buttons.
                        let widest = if count > 0 {
                            format!("{count} of {count}")
                        } else {
                            status.clone()
                        };
                        let count_w = if widest.is_empty() {
                            0.0
                        } else {
                            let galley = ui.painter().layout_no_wrap(
                                widest,
                                egui::FontId::proportional(11.5),
                                palette::TEXT_WEAK(),
                            );
                            (galley.size().x + 8.0).min(FIND_COUNT_MAX_W)
                        };
                        let rows = if find.replace_open { 2.0 } else { 1.0 };
                        let height = FIND_FIELD_H * rows + 4.0 * (rows - 1.0);
                        ui.horizontal_top(|ui| {
                            let chevron = if find.replace_open {
                                crate::icons::chevron_down()
                            } else {
                                crate::icons::chevron_right()
                            };
                            toggle_replace = find_icon_button(
                                ui,
                                chevron,
                                egui::vec2(16.0, height),
                                true,
                                "Toggle Replace",
                            )
                            .clicked();
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let field = find_field(
                                        ui,
                                        find_id,
                                        &mut find.query,
                                        "Find",
                                        invalid,
                                        |ui| {
                                            let o = &mut find.options;
                                            if find_toggle(
                                                ui,
                                                o.regex,
                                                ".*",
                                                false,
                                                "Use Regular Expression",
                                            )
                                            .clicked()
                                            {
                                                o.regex = !o.regex;
                                            }
                                            if find_toggle(
                                                ui,
                                                o.whole_word,
                                                "ab",
                                                true,
                                                "Match Whole Word",
                                            )
                                            .clicked()
                                            {
                                                o.whole_word = !o.whole_word;
                                            }
                                            if find_toggle(
                                                ui,
                                                o.match_case,
                                                "Aa",
                                                false,
                                                "Match Case",
                                            )
                                            .clicked()
                                            {
                                                o.match_case = !o.match_case;
                                            }
                                        },
                                    );
                                    if find.focus_pending {
                                        // Retried until it sticks: the widget's first frame is
                                        // an invisible sizing pass that can't hold focus.
                                        field.request_focus();
                                        if field.has_focus() {
                                            find.focus_pending = false;
                                            select_all_text(ui.ctx(), find_id, &find.query);
                                        }
                                    }
                                    let color = if invalid {
                                        palette::DANGER()
                                    } else {
                                        palette::TEXT_WEAK()
                                    };
                                    // Left-aligned in a fixed slot, so the buttons after it don't
                                    // shift as the count changes.
                                    ui.add_space(2.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(count_w, FIND_FIELD_H),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.set_min_width(count_w);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&status)
                                                        .size(11.5)
                                                        .color(color),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    let size = egui::vec2(FIND_BUTTON, FIND_BUTTON);
                                    previous |= find_icon_button(
                                        ui,
                                        crate::icons::chevron_up(),
                                        size,
                                        count > 0,
                                        "Previous Match (⇧Enter)",
                                    )
                                    .clicked();
                                    next |= find_icon_button(
                                        ui,
                                        crate::icons::chevron_down(),
                                        size,
                                        count > 0,
                                        "Next Match (Enter)",
                                    )
                                    .clicked();
                                    close |= find_icon_button(
                                        ui,
                                        crate::icons::close(),
                                        size,
                                        true,
                                        "Close (Escape)",
                                    )
                                    .clicked();
                                });
                                if find.replace_open {
                                    ui.horizontal(|ui| {
                                        find_field(
                                            ui,
                                            replace_id,
                                            &mut find.replacement,
                                            "Replace",
                                            false,
                                            |_| {},
                                        );
                                        // Skip the count slot so replace / replace-all line up
                                        // under previous / next.
                                        ui.add_space(2.0 + count_w + 4.0);
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        let size = egui::vec2(FIND_BUTTON, FIND_BUTTON);
                                        replace |= find_icon_button(
                                            ui,
                                            crate::icons::replace(),
                                            size,
                                            count > 0,
                                            "Replace (Enter)",
                                        )
                                        .clicked();
                                        replace_all |= find_icon_button(
                                            ui,
                                            crate::icons::replace_all(),
                                            size,
                                            count > 0,
                                            "Replace All",
                                        )
                                        .clicked();
                                    });
                                }
                            });
                        });
                    });
            });

        let tab = &mut self.tabs[idx];
        tab.find.had_focus = ctx.memory(|m| m.has_focus(find_id) || m.has_focus(replace_id));
        if toggle_replace {
            tab.find.replace_open = !tab.find.replace_open;
        }
        if close {
            tab.find.open = false;
            tab.find.had_focus = false;
            ctx.memory_mut(|memory| memory.request_focus(editor_id));
            return;
        }
        tab.find.refresh(&tab.sql, tab.sql_revision);
        let found_len = tab.find.found().len();
        // Typing or toggling jumps to the first match from the caret, like VS Code.
        let changed = before != (tab.find.query.clone(), tab.find.options);
        if changed {
            tab.find.current = tab.find.first_from(tab.primary_cursor.start);
        }
        if found_len == 0 {
            tab.find.current = 0;
            return;
        }
        if previous {
            tab.find.current = (tab.find.current + found_len - 1) % found_len;
        } else if next {
            tab.find.current = (tab.find.current + 1) % found_len;
        }

        if replace_all {
            let (query, replacement, options) = (
                tab.find.query.clone(),
                tab.find.replacement.clone(),
                tab.find.options,
            );
            if let Ok(count) =
                crate::editor_tools::replace_all(&mut tab.sql, &query, &replacement, options)
            {
                if count > 0 {
                    self.finish_external_editor_edit(idx);
                    self.status_msg = format!("Replaced {count} matches");
                }
            }
            self.tabs[idx].find.current = 0;
            return;
        }
        if replace {
            let current = tab.find.current.min(found_len - 1);
            let range = tab.find.found()[current].clone();
            let text = crate::editor_tools::replacement_for(
                &tab.sql,
                range.clone(),
                &tab.find.query,
                &tab.find.replacement,
                tab.find.options,
            );
            crate::editor_tools::replace_range(&mut tab.sql, range, &text);
            self.finish_external_editor_edit(idx);
            let tab = &mut self.tabs[idx];
            tab.find.refresh(&tab.sql, tab.sql_revision);
            // The next match slides into the replaced one's index.
            let remaining = tab.find.found().len();
            tab.find.current = if remaining == 0 {
                0
            } else {
                current % remaining
            };
        }
        let tab = &mut self.tabs[idx];
        if (changed || previous || next || replace) && !tab.find.found().is_empty() {
            let range = tab.find.found()[tab.find.current.min(tab.find.found().len() - 1)].clone();
            tab.find.reveal = true;
            // Keep typing in the widget: move the editor's selection without focusing it.
            self.set_editor_selection(&ctx, editor_id, range, false);
        }
    }

    /// Highlight the find widget's matches that are on screen (the current one stronger and
    /// outlined) into `under`, the slot painted beneath the editor's text, and scroll the
    /// current one into view when asked to.
    pub(super) fn paint_find_matches(
        ui: &egui::Ui,
        under: &mut Vec<egui::Shape>,
        find: &mut crate::editor_tools::FindState,
        view: &crate::fold::View,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
    ) {
        let found = find.found();
        if found.is_empty() {
            find.reveal = false;
            return;
        }
        let current = find.current.min(found.len() - 1);
        let offset = galley_pos.to_vec2();
        let rect_of = |range: &std::ops::Range<usize>| -> Option<egui::Rect> {
            let start = view.to_display(range.start)?;
            let end = view.to_display_clamped(range.end);
            let from = galley
                .pos_from_cursor(egui::text::CCursor::new(start))
                .translate(offset);
            let to = galley
                .pos_from_cursor(egui::text::CCursor::new(end))
                .translate(offset);
            // A match that wraps or spans lines is marked on its first row.
            let right = if (to.top() - from.top()).abs() < 1.0 {
                to.left()
            } else {
                from.left() + 8.0
            };
            Some(egui::Rect::from_min_max(
                from.left_top(),
                egui::pos2(right.max(from.left() + 2.0), from.bottom()),
            ))
        };
        // Only the matches between the first and last visible characters are painted.
        let clip = ui.clip_rect();
        let first = view.to_source(galley.cursor_from_pos(clip.min - galley_pos).index);
        let last = view.to_source(galley.cursor_from_pos(clip.max - galley_pos).index);
        let from = found.partition_point(|r| r.end < first);
        let to = found.partition_point(|r| r.start <= last);
        for (i, range) in found.iter().enumerate().take(to).skip(from) {
            if i == current {
                continue;
            }
            if let Some(rect) = rect_of(range) {
                highlight_shapes(
                    under,
                    rect,
                    palette::ACCENT().gamma_multiply(0.18),
                    palette::ACCENT().gamma_multiply(0.4),
                );
            }
        }
        if let Some(rect) = rect_of(&found[current]) {
            highlight_shapes(
                under,
                rect,
                palette::ACCENT().gamma_multiply(0.32),
                palette::ACCENT(),
            );
            if find.reveal {
                ui.scroll_to_rect(rect, Some(egui::Align::Center));
            }
        }
        find.reveal = false;
    }
}
