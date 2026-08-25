//! Searchable picker for long option lists.

use std::hash::Hash;

use crate::style::{palette, CONTROL_H};

const MIN_POPUP_WIDTH: f32 = 210.0;
const MAX_VISIBLE_ROWS: usize = 7;

/// Show a combo box with a focused search field and a virtualized result list.
///
/// The outer `Option` is `Some` only when the user made a choice. The inner value is the
/// option index, or `None` when `none_label` (for example "Skip") was chosen.
pub(crate) fn searchable_combo_box(
    ui: &mut egui::Ui,
    id_salt: impl Hash + Copy,
    selected_text: &str,
    width: f32,
    options: &[String],
    selected: Option<usize>,
    none_label: Option<&str>,
) -> Option<Option<usize>> {
    let button_id = ui.make_persistent_id(id_salt);
    let query_id = button_id.with("search_query");
    let open_id = button_id.with("was_open");
    let popup_id = button_id.with("popup");
    let was_open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let mut query = ui.data(|d| d.get_temp::<String>(query_id).unwrap_or_default());
    let mut picked = None;

    let button = searchable_combo_button(ui, button_id, selected_text, width, was_open);
    egui::Popup::menu(&button)
        .id(popup_id)
        .width(width.max(MIN_POPUP_WIDTH))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(width.max(MIN_POPUP_WIDTH));
            if !was_open {
                query.clear();
            }
            let search = super::icon_text_input(
                ui,
                &mut query,
                "Search…",
                crate::icons::search(),
                ui.available_width(),
            );
            if !was_open {
                search.request_focus();
            }
            ui.separator();

            let needle = query.trim().to_lowercase();
            let matches = matching_options(options, none_label, &needle);
            let choose_first =
                search.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if choose_first {
                if let Some(&first) = matches.first() {
                    picked = Some(first);
                    ui.close();
                }
            }

            // Fit short lists closely. Past the visible-row cap the exact viewport stays fixed
            // and the inner ScrollArea takes over; recomputing this every frame also lets the
            // popup grow back immediately when a search is cleared.
            let results_h = results_height(matches.len(), ui.spacing().item_spacing.y);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), results_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if matches.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("No matches").color(palette::TEXT_FAINT()));
                        return;
                    }

                    let row_h = CONTROL_H;
                    egui::ScrollArea::vertical()
                        .id_salt(button_id.with("results"))
                        .auto_shrink([false, false])
                        .show_rows(ui, row_h, matches.len(), |ui, range| {
                            // Popup menus default to centered content. Reset the list layout so
                            // names share one strong left edge and scan like database fields.
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                for row in range {
                                    let option = matches[row];
                                    let (is_selected, label) = match option {
                                        Some(index) => {
                                            (selected == Some(index), options[index].as_str())
                                        }
                                        None => (selected.is_none(), none_label.unwrap_or("—")),
                                    };
                                    let text_color = if option.is_none() {
                                        palette::TEXT_WEAK()
                                    } else {
                                        palette::TEXT()
                                    };
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), row_h),
                                        egui::Sense::click(),
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            true,
                                            is_selected,
                                            label,
                                        )
                                    });
                                    if ui.is_rect_visible(rect) {
                                        if is_selected {
                                            ui.painter().rect_filled(
                                                rect.shrink2(egui::vec2(2.0, 1.0)),
                                                egui::CornerRadius::same(6),
                                                palette::SELECTION(),
                                            );
                                        } else if response.hovered() {
                                            ui.painter().rect_filled(
                                                rect.shrink2(egui::vec2(2.0, 1.0)),
                                                egui::CornerRadius::same(6),
                                                palette::SURFACE_HOVER(),
                                            );
                                        }
                                        ui.painter().with_clip_rect(rect.shrink(2.0)).text(
                                            egui::pos2(rect.left() + 10.0, rect.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            label,
                                            egui::TextStyle::Button.resolve(ui.style()),
                                            text_color,
                                        );
                                    }
                                    if response.clicked() {
                                        picked = Some(option);
                                        ui.close();
                                    }
                                }
                            });
                        });
                },
            );
        });

    let open_now = egui::Popup::is_id_open(ui.ctx(), popup_id);
    ui.data_mut(|d| {
        d.insert_temp(open_id, open_now);
        if open_now {
            d.insert_temp(query_id, query);
        } else {
            d.remove::<String>(query_id);
        }
    });
    picked
}

fn searchable_combo_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected_text: &str,
    width: f32,
    open: bool,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(width.min(ui.available_width()), CONTROL_H));
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::WidgetType::ComboBox);
        info.current_text_value = Some(selected_text.to_string());
        info
    });

    if ui.is_rect_visible(rect) {
        let visuals = if open {
            &ui.visuals().widgets.open
        } else {
            ui.style().interact(&response)
        };
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );

        let chevron_center = egui::pos2(rect.right() - 14.0, rect.center().y);
        let chevron_stroke = egui::Stroke::new(1.5, visuals.fg_stroke.color);
        ui.painter().line_segment(
            [
                chevron_center + egui::vec2(-4.0, -2.0),
                chevron_center + egui::vec2(0.0, 2.0),
            ],
            chevron_stroke,
        );
        ui.painter().line_segment(
            [
                chevron_center + egui::vec2(0.0, 2.0),
                chevron_center + egui::vec2(4.0, -2.0),
            ],
            chevron_stroke,
        );

        let text_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(chevron_center.x - 8.0, rect.bottom()));
        ui.painter().with_clip_rect(text_clip).text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            selected_text,
            egui::TextStyle::Button.resolve(ui.style()),
            visuals.text_color(),
        );
    }
    response
}

fn matching_options(
    options: &[String],
    none_label: Option<&str>,
    needle: &str,
) -> Vec<Option<usize>> {
    let mut matches = Vec::with_capacity(options.len() + usize::from(none_label.is_some()));
    if none_label.is_some_and(|label| needle.is_empty() || label.to_lowercase().contains(needle)) {
        matches.push(None);
    }
    matches.extend(options.iter().enumerate().filter_map(|(index, label)| {
        (needle.is_empty() || label.to_lowercase().contains(needle)).then_some(Some(index))
    }));
    matches
}

fn results_height(item_count: usize, spacing: f32) -> f32 {
    let rows = item_count.clamp(1, MAX_VISIBLE_ROWS);
    CONTROL_H * rows as f32 + spacing * rows.saturating_sub(1) as f32
}

#[cfg(test)]
mod tests {
    use super::{matching_options, results_height, MAX_VISIBLE_ROWS};
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn search_is_case_insensitive_and_preserves_original_indices() {
        let options = vec!["Company ID".into(), "created_at".into(), "Country".into()];
        assert_eq!(
            matching_options(&options, None, "co"),
            vec![Some(0), Some(2)]
        );
    }

    #[test]
    fn optional_empty_choice_is_searchable_too() {
        let options = vec!["company_id".into()];
        assert_eq!(matching_options(&options, Some("Skip"), "ski"), vec![None]);
    }

    #[test]
    fn popup_height_tracks_short_lists_and_caps_long_ones() {
        let spacing = 4.0;
        assert!(results_height(2, spacing) < results_height(5, spacing));
        assert_eq!(
            results_height(MAX_VISIBLE_ROWS, spacing),
            results_height(1_000, spacing)
        );
    }

    #[test]
    fn typing_in_the_popup_filters_and_selects_an_option() {
        let selected = std::rc::Rc::new(std::cell::RefCell::new(Some(0usize)));
        let state = selected.clone();
        let options = vec![
            "company_id".to_string(),
            "company_name".to_string(),
            "thailand_time".to_string(),
            "dashboard_today".to_string(),
        ];
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(420.0, 360.0))
            .build_ui(move |ui| {
                let current = *state.borrow();
                let label = current.map_or("—", |index| options[index].as_str());
                if let Some(choice) = super::searchable_combo_box(
                    ui,
                    "test_combo",
                    label,
                    180.0,
                    &options,
                    current,
                    None,
                ) {
                    *state.borrow_mut() = choice;
                }
            });

        harness.run_steps(2);
        harness.get_by_value("company_id").click();
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("thai");
        harness.run_steps(2);
        assert!(harness.query_by_label("dashboard_today").is_none());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.key_press(egui::Key::Backspace);
        harness.run_steps(2);
        assert!(harness.query_by_label("dashboard_today").is_some());
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("thai");
        harness.run_steps(2);
        harness.get_by_label("thailand_time").click();
        harness.run();
        assert_eq!(*selected.borrow(), Some(2));
    }
}
