//! Query workspace rendering and interaction.

use crate::app::{Action, DbGuiApp, QueryParameterKind};
use crate::components;
use crate::icons;
use crate::style::palette;

/// The editor font-size range and default, matching the Settings slider.
const MIN_FONT: f32 = 9.0;
const MAX_FONT: f32 = 24.0;
const DEFAULT_FONT: f32 = 14.0;

impl DbGuiApp {
    pub(super) fn query_workspace_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        self.tab_mut().sync_query_parameters();
        let dialect_label = self.active().map(|a| a.db.kind().label()).unwrap_or("SQL");
        let has_sql = !self.tab().sql.trim().is_empty();
        let parameter_count = self.tab().query_parameters.len();
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
                self.editor_options_button(ui, actions);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_run = self.active().is_some()
                        && self.query_can_run(self.active_query_tab)
                        && has_sql;
                    let run = components::run_button(ui, can_run, has_sql, self.run_all_by_default);
                    if let Some(run_all_by_default) = run.default_run_all {
                        self.run_all_by_default = run_all_by_default;
                        self.persist_settings();
                    }
                    if run.run_current || run.run_all {
                        // A split pane is a real workspace in its own right. Remember which
                        // pane launched Run so the action executes against that pane's SQL and
                        // stores rows in that pane's result view.
                        self.focused_pane = self.tabs[self.active_query_tab].pane;
                        actions.push(if run.run_current {
                            Action::RunCurrentQuery
                        } else {
                            Action::RunQuery
                        });
                    }
                    if run.save_query {
                        actions.push(Action::SaveCurrentAsFavorite);
                    }
                    if parameter_count > 0
                        && components::button::soft_icon_button_state(
                            ui,
                            icons::key(),
                            if self.tab().parameters_expanded {
                                "Hide query parameters"
                            } else {
                                "Show query parameters"
                            },
                            true,
                            self.tab().parameters_expanded,
                        )
                        .clicked()
                    {
                        self.tab_mut().parameters_expanded = !self.tab().parameters_expanded;
                    }
                    let resp =
                        components::beautify_button(ui, &mut self.beautify, has_sql, dialect_label);
                    if resp.clicked {
                        actions.push(Action::BeautifySql);
                    }
                    if resp.prefs_changed {
                        self.persist_settings();
                    }
                });
            });
        });
    }

    /// One icon button whose dropdown holds the SQL editor's options — font size, wrapping,
    /// whitespace, the current-query tint, bracket pairing and every autocomplete behaviour —
    /// so they are a click away instead of in Settings. They change the same persisted
    /// preferences the Settings page edits. Sits at the toolbar's left end, its dropdown
    /// hanging flush with it.
    fn editor_options_button(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let (rect, button) = ui.allocate_exact_size(egui::vec2(42.0, 22.0), egui::Sense::click());
        button.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Editor options")
        });
        if ui.is_rect_visible(rect) {
            let fill = if button.hovered() || egui::Popup::is_id_open(ui.ctx(), button.id) {
                palette::SURFACE_HOVER()
            } else {
                palette::SURFACE()
            };
            ui.painter().rect(
                rect,
                egui::CornerRadius::same(5),
                fill,
                egui::Stroke::new(1.0_f32, palette::BORDER()),
                egui::StrokeKind::Outside,
            );
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 14.0, rect.center().y),
                egui::Vec2::splat(14.0),
            );
            egui::Image::new(icons::settings())
                .fit_to_exact_size(icon.size())
                .tint(palette::TEXT())
                .paint_at(ui, icon);
            let chevron = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Vec2::splat(12.0),
            );
            egui::Image::new(icons::chevron_down())
                .fit_to_exact_size(chevron.size())
                .tint(palette::TEXT_WEAK())
                .paint_at(ui, chevron);
        }
        let button = button.on_hover_text("Editor options");

        let before = (
            self.editor_font_size,
            self.editor_wrap_lines,
            self.autocomplete_enabled,
            self.ghost_suggestions_enabled,
            self.editor_options.clone(),
        );
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_START)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .gap(6.0)
            .frame(components::menu_popup_frame(ui.style()))
            .show(|ui| {
                ui.set_width(250.0);
                components::style_menu_submenus(ui);
                self.font_size_row(ui);
                ui.separator();
                let options = &mut self.editor_options;
                components::menu_checkbox(ui, &mut self.editor_wrap_lines, "Wrap long lines");
                components::menu_checkbox(
                    ui,
                    &mut options.show_invisibles,
                    "Show invisible characters",
                );
                components::menu_checkbox(
                    ui,
                    &mut options.highlight_current_statement,
                    "Highlight current query",
                );
                components::menu_checkbox(
                    ui,
                    &mut options.auto_close_pairs,
                    "Auto-close brackets and quotes",
                );
                ui.separator();
                components::menu_submenu(ui, "Autocomplete", |ui| {
                    ui.set_width(260.0);
                    components::menu_checkbox(
                        ui,
                        &mut self.autocomplete_enabled,
                        "Suggest while typing",
                    );
                    components::menu_checkbox(
                        ui,
                        &mut self.ghost_suggestions_enabled,
                        "Inline suggestions",
                    );
                    ui.separator();
                    let options = &mut self.editor_options;
                    components::menu_checkbox(ui, &mut options.suggest_tables, "Tables and views");
                    components::menu_checkbox(ui, &mut options.suggest_columns, "Columns");
                    components::menu_checkbox(ui, &mut options.suggest_functions, "Functions");
                    components::menu_checkbox(ui, &mut options.suggest_keywords, "Keywords");
                    ui.separator();
                    components::menu_checkbox(
                        ui,
                        &mut options.add_space_after_completion,
                        "Add a space after completing",
                    );
                    components::menu_checkbox(
                        ui,
                        &mut options.prefix_schema,
                        "Prefix schema names",
                    );
                    components::menu_checkbox(
                        ui,
                        &mut options.uppercase_keywords,
                        "Uppercase keywords",
                    );
                });
                ui.separator();
                if components::menu_item(ui, "All settings…", None, true).clicked() {
                    actions.push(Action::OpenSettings);
                    ui.close();
                }
            });
        let after = (
            self.editor_font_size,
            self.editor_wrap_lines,
            self.autocomplete_enabled,
            self.ghost_suggestions_enabled,
            self.editor_options.clone(),
        );
        if after != before {
            self.persist_settings();
        }
    }

    /// "Font size  − 14 +": steps the SQL editor font one point at a time; clicking the number
    /// resets it.
    fn font_size_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(components::MENU_ROW_H);
            ui.add_space(34.0);
            ui.label("Font size");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let size = self.editor_font_size.round();
                if components::soft_icon_button(ui, icons::plus(), "Larger", size < MAX_FONT)
                    .clicked()
                {
                    self.editor_font_size = (size + 1.0).min(MAX_FONT);
                }
                let value = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(format!("{size:.0}")).color(palette::TEXT()),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!("Click to reset to {DEFAULT_FONT:.0}"));
                if value.clicked() {
                    self.editor_font_size = DEFAULT_FONT;
                }
                if components::soft_icon_button(ui, icons::minus(), "Smaller", size > MIN_FONT)
                    .clicked()
                {
                    self.editor_font_size = (size - 1.0).max(MIN_FONT);
                }
            });
        });
    }

    pub(super) fn query_parameter_panel(&mut self, ui: &mut egui::Ui) {
        let idx = self.active_query_tab;
        self.tabs[idx].sync_query_parameters();
        if self.tabs[idx].query_parameters.is_empty() || !self.tabs[idx].parameters_expanded {
            return;
        }

        egui::Frame::new()
            .fill(palette::SURFACE())
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Parameters")
                            .size(11.5)
                            .strong()
                            .color(palette::TEXT_WEAK()),
                    );
                    ui.label(
                        egui::RichText::new("{{name}}")
                            .monospace()
                            .size(10.5)
                            .color(palette::TEXT_FAINT()),
                    );
                });
                ui.add_space(5.0);

                egui::ScrollArea::both()
                    .id_salt(("query_parameters", self.tabs[idx].id))
                    .max_height(118.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        for parameter in &mut self.tabs[idx].query_parameters {
                            ui.horizontal(|ui| {
                                ui.set_min_height(26.0);
                                ui.add_sized(
                                    [132.0, 24.0],
                                    egui::Label::new(
                                        egui::RichText::new(&parameter.name)
                                            .monospace()
                                            .color(palette::TEXT()),
                                    ),
                                );
                                let old_kind = parameter.kind;
                                egui::ComboBox::from_id_salt((
                                    "query_parameter_kind",
                                    &parameter.name,
                                ))
                                .width(86.0)
                                .selected_text(parameter.kind.label())
                                .show_ui(ui, |ui| {
                                    for kind in QueryParameterKind::ALL {
                                        ui.selectable_value(
                                            &mut parameter.kind,
                                            kind,
                                            kind.label(),
                                        );
                                    }
                                });
                                if parameter.kind != old_kind {
                                    parameter.set = parameter.kind == QueryParameterKind::Null
                                        || !parameter.value.is_empty();
                                }
                                if parameter.kind == QueryParameterKind::Null {
                                    ui.label(
                                        egui::RichText::new("SQL NULL")
                                            .size(11.5)
                                            .color(palette::TEXT_FAINT()),
                                    );
                                } else {
                                    let response = ui.add_sized(
                                        [ui.available_width().max(100.0), 24.0],
                                        egui::TextEdit::singleline(&mut parameter.value).hint_text(
                                            match parameter.kind {
                                                QueryParameterKind::Boolean => "true or false",
                                                QueryParameterKind::Number => "0",
                                                QueryParameterKind::Text => "Value",
                                                QueryParameterKind::Null => "",
                                            },
                                        ),
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::TextEdit,
                                            true,
                                            format!("Parameter {}", parameter.name),
                                        )
                                    });
                                    if response.changed() {
                                        parameter.set = true;
                                    }
                                }
                            });
                        }
                    });
            });
        ui.painter().hline(
            ui.min_rect().x_range(),
            ui.min_rect().bottom(),
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );
    }
}
