//! Query workspace rendering and interaction.

use crate::app::{Action, DbGuiApp, QueryParameterKind};
use crate::components;
use crate::icons;
use crate::style::palette;

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
                ui.label(
                    egui::RichText::new(format!("{dialect_label} workspace"))
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
                let dot = if self.active().is_some() {
                    palette::SUCCESS()
                } else {
                    palette::TEXT_FAINT()
                };
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(8.0, row_h), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.0, dot);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_run = self.active().is_some()
                        && self.query_can_run(self.active_query_tab)
                        && has_sql;
                    if self.is_tab_querying(self.tab().id)
                        && components::button(ui, icons::close(), "Cancel query", true).clicked()
                    {
                        actions.push(Action::CancelTabQuery(self.tab().id));
                    }
                    let run = components::run_button(ui, can_run, has_sql, self.run_all_by_default);
                    if let Some(run_all_by_default) = run.default_run_all {
                        self.run_all_by_default = run_all_by_default;
                        self.persist_settings();
                    }
                    if run.run_current || run.run_all {
                        // A split pane is a real workspace in its own right. Remember which
                        // pane launched Run so the action executes against that pane's SQL and
                        // stores rows in that pane's result view.
                        self.split_focus = self.split_tab == Some(self.active_query_tab);
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
