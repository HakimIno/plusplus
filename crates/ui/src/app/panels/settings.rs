//! Settings rendering and interaction.

use crate::app::{
    Action, DbGuiApp, SettingsSection, MAX_RESULT_MEMORY_BUDGET_MB, MIN_RESULT_MEMORY_BUDGET_MB,
};
use crate::components;
use crate::icons;
use crate::style::palette;

/// One theme card in the appearance picker, snapshotted before rendering so drawing a
/// choice never holds a borrow on `self`:
/// `(key, display name, builtin, author)`.
type ThemeOption = (String, String, bool, Option<String>);

fn settings_nav_item(
    ui: &mut egui::Ui,
    current: &mut SettingsSection,
    candidate: SettingsSection,
    label: &str,
    icon: egui::ImageSource<'static>,
) {
    let selected = *current == candidate;
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label)
    });

    let keyboard_activated = response.has_focus()
        && ui.input(|input| {
            input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
        });
    if response.clicked() || keyboard_activated {
        *current = candidate;
        response.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let text_color = if selected {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let fill = if selected {
            palette::SELECTION()
        } else if response.hovered() || response.has_focus() {
            palette::SURFACE_HOVER()
        } else {
            egui::Color32::TRANSPARENT
        };

        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), fill);

        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        );
        egui::Image::new(icon)
            .fit_to_exact_size(icon_rect.size())
            .tint(text_color)
            .paint_at(ui, icon_rect);
        ui.painter().text(
            egui::pos2(icon_rect.right() + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            text_color,
        );
    }
}

fn settings_toggle_row(ui: &mut egui::Ui, value: &mut bool, title: &str, description: &str) {
    let row_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2((row_width - 66.0).max(180.0), 50.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width((row_width - 66.0).max(180.0));
                ui.label(egui::RichText::new(title).size(14.0).color(palette::TEXT()));
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(description)
                        .size(12.0)
                        .color(palette::TEXT_WEAK()),
                );
            },
        );
        components::input::toggle_switch(ui, value);
    });
}

impl DbGuiApp {
    /// Settings workspace shown behind a selected utility tab. The category rail and active
    /// section share one centered workspace so neither side is pinned to the window edge.
    pub(in crate::app) fn draw_settings_page(
        &mut self,
        root: &mut egui::Ui,
        actions: &mut Vec<Action>,
    ) {
        let ctx = root.ctx().clone();
        if ctx.input(|i| {
            i.key_pressed(egui::Key::Escape) || (i.modifiers.command && i.key_pressed(egui::Key::W))
        }) {
            actions.push(Action::CloseSettings);
        }

        let mut reload_themes = false;
        let mut chosen = self.theme.clone();
        let mut ui_font = self.ui_font.clone();
        let mut code_font = self.code_font.clone();
        let mut editor_font_size = self.editor_font_size;
        let mut editor_wrap_lines = self.editor_wrap_lines;
        let mut autocomplete_enabled = self.autocomplete_enabled;
        let mut ghost_suggestions_enabled = self.ghost_suggestions_enabled;
        let mut import_font = false;
        let custom_fonts = self.custom_fonts.clone();
        let mut section = self.settings_section;
        let mut history_enabled = self.history_enabled;
        let mut audit_enabled = self.audit_enabled;
        let mut review_edits_before_save = self.review_edits_before_save;
        let mut update_check_enabled = self.update_check_enabled;
        let mut result_memory_budget_mb = (self.result_memory_budget / (1024 * 1024)) as u32;
        // Snapshot the display data so rendering a choice never holds a borrow on `self`.
        let options: Vec<ThemeOption> = self
            .themes
            .entries()
            .iter()
            .map(|e| (e.key.clone(), e.name.clone(), e.builtin, e.author.clone()))
            .collect();
        let themes_dir = dbcore::config::themes_dir()
            .ok()
            .map(|p| p.display().to_string());

        let outer_gutter = (root.available_width() * 0.028).clamp(16.0, 56.0);
        let gutter_frame = || egui::Frame::new().fill(palette::BASE());
        egui::Panel::left("settings_left_gutter")
            .resizable(false)
            .exact_size(outer_gutter)
            .frame(gutter_frame())
            .show_inside(root, |_| {});
        egui::Panel::right("settings_right_gutter")
            .resizable(false)
            .exact_size(outer_gutter)
            .frame(gutter_frame())
            .show_inside(root, |_| {});

        egui::Panel::left("settings_category_rail")
            .resizable(false)
            .exact_size(280.0)
            .frame(
                egui::Frame::new()
                    .fill(palette::PANEL())
                    .inner_margin(egui::Margin::symmetric(24, 34)),
            )
            .show_inside(root, |ui| {
                let nav_width = 220.0_f32.min(ui.available_width());
                let nav_inset = ((ui.available_width() - nav_width) * 0.5).max(0.0);
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.add_space(nav_inset);
                    ui.vertical(|ui| {
                        ui.set_width(nav_width);
                        ui.label(
                            egui::RichText::new("Settings")
                                .size(21.0)
                                .strong()
                                .color(palette::TEXT()),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new("Workspace preferences")
                                .size(12.0)
                                .color(palette::TEXT_WEAK()),
                        );
                        ui.add_space(30.0);

                        ui.spacing_mut().item_spacing.y = 4.0;
                        settings_nav_item(
                            ui,
                            &mut section,
                            SettingsSection::General,
                            "General",
                            icons::settings(),
                        );
                        settings_nav_item(
                            ui,
                            &mut section,
                            SettingsSection::Appearance,
                            "Appearance",
                            icons::fit(),
                        );
                        settings_nav_item(
                            ui,
                            &mut section,
                            SettingsSection::Privacy,
                            "Privacy",
                            icons::key(),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette::BASE())
                    .inner_margin(egui::Margin::same(0)),
            )
            .show_inside(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("settings_page_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(38.0);
                        let available = ui.available_width();
                        let content_w = (available - 80.0).clamp(340.0, 980.0);
                        let inset = ((available - content_w) / 2.0).max(0.0);

                        ui.horizontal(|ui| {
                            ui.add_space(inset);
                            ui.vertical(|ui| {
                                ui.set_width(content_w);
                                let (page_title, page_description) = match section {
                                    SettingsSection::General => (
                                        "General",
                                        "Control how plusplus behaves when the application starts.",
                                    ),
                                    SettingsSection::Appearance => (
                                        "Appearance",
                                        "Choose how your workspace and SQL tools look.",
                                    ),
                                    SettingsSection::Privacy => (
                                        "Privacy",
                                        "Control which activity records stay on this machine.",
                                    ),
                                };
                                ui.label(
                                    egui::RichText::new(page_title)
                                        .size(30.0)
                                        .strong()
                                        .color(palette::TEXT()),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(page_description)
                                        .size(14.0)
                                        .color(palette::TEXT_WEAK()),
                                );

                                match section {
                                    SettingsSection::General => {
                                        ui.add_space(30.0);
                                        ui.label(
                                            egui::RichText::new("Workspace")
                                                .size(14.0)
                                                .color(palette::TEXT_WEAK()),
                                        );
                                        ui.add_space(10.0);
                                        settings_toggle_row(
                                            ui,
                                            &mut update_check_enabled,
                                            "Check for updates at launch",
                                            "Ask GitHub for the latest release. No telemetry is sent.",
                                        );
                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(12.0);
                                        settings_toggle_row(
                                            ui,
                                            &mut review_edits_before_save,
                                            "Review changes before saving",
                                            "Show the generated SQL before ⌘S writes row edits. Production connections always confirm.",
                                        );
                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(14.0);
                                        let row_width = ui.available_width();
                                        let labels = |ui: &mut egui::Ui| {
                                            ui.label(
                                                egui::RichText::new("Global memory budget")
                                                    .size(14.0)
                                                    .color(palette::TEXT()),
                                            );
                                            ui.add_space(3.0);
                                            ui.label(
                                                egui::RichText::new(
                                                    "Shared by results in every tab. Inactive results are released first.",
                                                )
                                                .size(12.0)
                                                .color(palette::TEXT_WEAK()),
                                            );
                                        };
                                        let slider = |ui: &mut egui::Ui,
                                                      value: &mut u32,
                                                      width: f32| {
                                            ui.scope(|ui| {
                                                let visuals = ui.visuals_mut();
                                                visuals.extreme_bg_color = palette::BASE();
                                                visuals.widgets.inactive.bg_stroke =
                                                    egui::Stroke::NONE;
                                                visuals.widgets.hovered.bg_stroke =
                                                    egui::Stroke::NONE;
                                                ui.add_sized(
                                                    egui::vec2(width, 28.0),
                                                    egui::Slider::new(
                                                        value,
                                                        MIN_RESULT_MEMORY_BUDGET_MB
                                                            ..=MAX_RESULT_MEMORY_BUDGET_MB,
                                                    )
                                                    .suffix(" MiB")
                                                    .logarithmic(true),
                                                );
                                            });
                                        };
                                        if row_width >= 520.0 {
                                            ui.horizontal(|ui| {
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(row_width - 262.0, 56.0),
                                                    egui::Layout::top_down(egui::Align::Min),
                                                    |ui| {
                                                        ui.set_min_width(row_width - 262.0);
                                                        labels(ui);
                                                    },
                                                );
                                                slider(ui, &mut result_memory_budget_mb, 246.0);
                                            });
                                        } else {
                                            labels(ui);
                                            ui.add_space(10.0);
                                            slider(ui, &mut result_memory_budget_mb, row_width);
                                        }
                                    }
                                    SettingsSection::Appearance => {
                                ui.add_space(30.0);
                                ui.label(
                                    egui::RichText::new("Personalization")
                                        .size(14.0)
                                        .color(palette::TEXT_WEAK()),
                                );
                                ui.add_space(10.0);

                                let row_width = ui.available_width();
                                        let selected_theme = options
                                            .iter()
                                            .find(|(key, _, _, _)| *key == chosen)
                                            .map_or("Default", |(_, label, _, _)| label.as_str())
                                            .to_string();
                                        let theme_labels = |ui: &mut egui::Ui| {
                                            ui.label(
                                                egui::RichText::new("Theme")
                                                    .size(14.0)
                                                    .color(palette::TEXT()),
                                            );
                                            ui.add_space(3.0);
                                            ui.label(
                                                egui::RichText::new(
                                                    "Choose a palette for the workspace and SQL editor.",
                                                )
                                                .size(12.0)
                                                .color(palette::TEXT_WEAK()),
                                            );
                                        };
                                        let theme_combo = |ui: &mut egui::Ui,
                                                           chosen: &mut String,
                                                           width: f32| {
                                            egui::ComboBox::from_id_salt("appearance_theme")
                                                .width(width)
                                                .selected_text(&selected_theme)
                                                .show_ui(ui, |ui| {
                                                    for (key, label, builtin, author) in &options {
                                                        let detail = if *builtin {
                                                            label.clone()
                                                        } else if let Some(author) = author {
                                                            format!("{label} · {author}")
                                                        } else {
                                                            format!("{label} · Custom")
                                                        };
                                                        ui.selectable_value(
                                                            chosen,
                                                            key.clone(),
                                                            detail,
                                                        );
                                                    }
                                                });
                                        };
                                        if row_width >= 540.0 {
                                            ui.horizontal(|ui| {
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(row_width - 280.0, 50.0),
                                                    egui::Layout::top_down(egui::Align::Min),
                                                    |ui| {
                                                        ui.set_min_width(row_width - 280.0);
                                                        theme_labels(ui);
                                                    },
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| theme_combo(ui, &mut chosen, 250.0),
                                                );
                                            });
                                        } else {
                                            theme_labels(ui);
                                            ui.add_space(8.0);
                                            theme_combo(ui, &mut chosen, row_width);
                                        }

                                        ui.add_space(14.0);
                                        ui.separator();
                                        ui.add_space(14.0);
                                        ui.horizontal(|ui| {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(
                                                    (row_width - 150.0).max(180.0),
                                                    44.0,
                                                ),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| {
                                                    ui.set_min_width(
                                                        (row_width - 150.0).max(180.0),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new("Custom themes")
                                                            .size(14.0)
                                                            .color(palette::TEXT()),
                                                    );
                                                    ui.add_space(3.0);
                                                    ui.label(
                                                        egui::RichText::new(
                                                            "Rescan the local themes folder.",
                                                        )
                                                        .size(12.0)
                                                        .color(palette::TEXT_WEAK()),
                                                    );
                                                },
                                            );
                                            if components::Btn::new("Reload")
                                                .icon(icons::refresh())
                                                .show(ui)
                                                .on_hover_text(
                                                    themes_dir
                                                        .as_deref()
                                                        .map(|d| format!("Theme folder:\n{d}"))
                                                        .unwrap_or_else(|| {
                                                            "Re-scan the themes folder".to_string()
                                                        }),
                                                )
                                                .clicked()
                                            {
                                                reload_themes = true;
                                            }
                                        });

                                    ui.add_space(28.0);
                                    ui.label(
                                        egui::RichText::new("Typography")
                                            .size(14.0)
                                            .color(palette::TEXT_WEAK()),
                                    );
                                    ui.add_space(10.0);

                                    let row_width = ui.available_width();
                                    ui.horizontal(|ui| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2((row_width - 230.0).max(180.0), 50.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.set_min_width((row_width - 230.0).max(180.0));
                                                ui.label(
                                                    egui::RichText::new("Editor font size")
                                                        .size(14.0)
                                                        .color(palette::TEXT()),
                                                );
                                                ui.add_space(3.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        "Applies to SQL editors and line numbers.",
                                                    )
                                                    .size(12.0)
                                                    .color(palette::TEXT_WEAK()),
                                                );
                                            },
                                        );
                                        ui.add_sized(
                                            egui::vec2(210.0, 28.0),
                                            egui::Slider::new(&mut editor_font_size, 9.0..=24.0)
                                                .suffix(" pt"),
                                        );
                                    });
                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.add_space(12.0);
                                    settings_toggle_row(
                                        ui,
                                        &mut editor_wrap_lines,
                                        "Wrap long lines",
                                        "Keep long SQL statements visible without horizontal scrolling.",
                                    );
                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.add_space(12.0);
                                    settings_toggle_row(
                                        ui,
                                        &mut autocomplete_enabled,
                                        "Autocomplete",
                                        "Suggest SQL keywords, tables and columns while typing.",
                                    );
                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.add_space(12.0);
                                    settings_toggle_row(
                                        ui,
                                        &mut ghost_suggestions_enabled,
                                        "Inline suggestions",
                                        "Show a short completion after the caret; press Tab to accept it.",
                                    );
                                    ui.add_space(14.0);
                                    ui.separator();
                                    ui.add_space(28.0);

                                    for (id, title, description, selection, default) in [
                                                (
                                                    "interface_font",
                                                    "Interface font",
                                                    "Menus, labels, headings and controls",
                                                    &mut ui_font,
                                                    "Inter (built in)",
                                                ),
                                                (
                                                    "code_font",
                                                    "Editor & data font",
                                                    "SQL, values and code-like metadata",
                                                    &mut code_font,
                                                    "Same as interface",
                                                ),
                                            ] {
                                                let row_width = ui.available_width();
                                                let selected = selection
                                                    .as_deref()
                                                    .and_then(|key| {
                                                        custom_fonts
                                                            .iter()
                                                            .find(|font| font.key == key)
                                                    })
                                                    .map_or_else(
                                                        || default.to_string(),
                                                        |font| font.label.clone(),
                                                    );
                                                let labels = |ui: &mut egui::Ui| {
                                                    ui.label(
                                                        egui::RichText::new(title)
                                                            .size(14.0)
                                                            .color(palette::TEXT()),
                                                    );
                                                    ui.add_space(3.0);
                                                    ui.label(
                                                        egui::RichText::new(description)
                                                            .size(12.0)
                                                            .color(palette::TEXT_WEAK()),
                                                    );
                                                };
                                                let combo = |ui: &mut egui::Ui,
                                                             selection: &mut Option<String>,
                                                             width: f32| {
                                                    egui::ComboBox::from_id_salt(id)
                                                        .width(width)
                                                        .selected_text(&selected)
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(
                                                                selection,
                                                                None,
                                                                default,
                                                            );
                                                            for font in &custom_fonts {
                                                                ui.selectable_value(
                                                                    selection,
                                                                    Some(font.key.clone()),
                                                                    &font.label,
                                                                );
                                                            }
                                                        });
                                                };
                                                if row_width >= 540.0 {
                                                    ui.horizontal(|ui| {
                                                        ui.allocate_ui_with_layout(
                                                            egui::vec2(row_width - 280.0, 50.0),
                                                            egui::Layout::top_down(
                                                                egui::Align::Min,
                                                            ),
                                                            |ui| {
                                                                ui.set_min_width(
                                                                    row_width - 280.0,
                                                                );
                                                                labels(ui);
                                                            },
                                                        );
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| combo(ui, selection, 250.0),
                                                        );
                                                    });
                                                } else {
                                                    labels(ui);
                                                    ui.add_space(8.0);
                                                    combo(ui, selection, row_width);
                                                }
                                                if id == "interface_font" {
                                                    ui.add_space(14.0);
                                                    ui.separator();
                                                    ui.add_space(14.0);
                                                }
                                            }

                                            ui.add_space(14.0);
                                            ui.separator();
                                            ui.add_space(14.0);
                                            let row_width = ui.available_width();
                                            ui.horizontal(|ui| {
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(
                                                        (row_width - 150.0).max(180.0),
                                                        44.0,
                                                    ),
                                                    egui::Layout::top_down(egui::Align::Min),
                                                    |ui| {
                                                        ui.set_min_width(
                                                            (row_width - 150.0).max(180.0),
                                                        );
                                                        ui.label(
                                                            egui::RichText::new("Imported fonts")
                                                                .size(14.0)
                                                                .color(palette::TEXT()),
                                                        );
                                                        ui.add_space(3.0);
                                                        ui.label(
                                                            egui::RichText::new(
                                                                "TTF or OTF · up to 32 MiB",
                                                            )
                                                            .size(12.0)
                                                            .color(palette::TEXT_WEAK()),
                                                        );
                                                    },
                                                );
                                                if components::Btn::new("Import…")
                                                    .show(ui)
                                                    .on_hover_text(
                                                        "Copy a .ttf or .otf file into the plusplus font library",
                                                    )
                                                    .clicked()
                                                {
                                                    import_font = true;
                                                }
                                            });

                                            ui.add_space(14.0);
                                            egui::Frame::new()
                                                .fill(palette::SURFACE())
                                                .corner_radius(egui::CornerRadius::same(9))
                                                .inner_margin(egui::Margin::symmetric(12, 9))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.label(
                                                        egui::RichText::new(
                                                            "Aa  Database workspace  ·  กข  ภาษาไทย",
                                                        )
                                                        .font(egui::FontId::proportional(13.0))
                                                        .color(palette::TEXT()),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(
                                                            "SELECT customer_id, total FROM orders;",
                                                        )
                                                        .font(egui::FontId::monospace(12.0))
                                                        .color(palette::TEXT_WEAK()),
                                                    );
                                                });

                                    }
                                    SettingsSection::Privacy => {
                                ui.add_space(30.0);
                                ui.label(
                                    egui::RichText::new("Local records")
                                        .size(14.0)
                                        .color(palette::TEXT_WEAK()),
                                );
                                ui.add_space(10.0);

                                for (enabled, title, description) in [
                                            (
                                                &mut history_enabled,
                                                "Record query history",
                                                "Keep executed SQL and its outcome in a local history file.",
                                            ),
                                            (
                                                &mut audit_enabled,
                                                "Record audit trail",
                                                "Append connections and statements to a monthly local compliance log.",
                                            ),
                                        ] {
                                            settings_toggle_row(ui, enabled, title, description);
                                            ui.add_space(12.0);
                                            if title != "Record audit trail" {
                                                ui.separator();
                                                ui.add_space(12.0);
                                            }
                                        }
                                    }
                                }

                                ui.add_space(40.0);
                            });
                        });
                    });
            });

        self.settings_section = section;
        if reload_themes {
            self.themes.reload();
            // A previously-selected custom theme may have been removed; re-resolve so the
            // active colours and the persisted key stay valid.
            let resolved = self.themes.resolve_key(&self.theme);
            if resolved != self.theme {
                self.set_theme(&ctx, resolved);
            }
        }
        if chosen != self.theme {
            self.set_theme(&ctx, chosen);
        }

        if ui_font != self.ui_font || code_font != self.code_font {
            let previous_ui = self.ui_font.clone();
            let previous_code = self.code_font.clone();
            self.ui_font = ui_font;
            self.code_font = code_font;
            match self.apply_fonts(&ctx) {
                Ok(()) => self.persist_settings(),
                Err(error) => {
                    self.ui_font = previous_ui;
                    self.code_font = previous_code;
                    let _ = self.apply_fonts(&ctx);
                    self.error = Some(format!("Could not load font: {error}"));
                }
            }
        }
        if import_font {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("OpenType font", &["ttf", "otf"])
                .pick_file()
            {
                match crate::fonts::import(&path) {
                    Ok(font) => {
                        self.custom_fonts = crate::fonts::list_imported();
                        self.ui_font = Some(font.key);
                        match self.apply_fonts(&ctx) {
                            Ok(()) => {
                                self.persist_settings();
                                self.status_msg = format!("Imported {}", font.label);
                                self.error = None;
                            }
                            Err(error) => {
                                self.ui_font = None;
                                let _ = self.apply_fonts(&ctx);
                                self.error = Some(format!("Could not load font: {error}"));
                            }
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }

        let preferences_changed = history_enabled != self.history_enabled
            || audit_enabled != self.audit_enabled
            || review_edits_before_save != self.review_edits_before_save
            || update_check_enabled != self.update_check_enabled
            || editor_font_size != self.editor_font_size
            || editor_wrap_lines != self.editor_wrap_lines
            || autocomplete_enabled != self.autocomplete_enabled
            || ghost_suggestions_enabled != self.ghost_suggestions_enabled
            || result_memory_budget_mb as usize * 1024 * 1024 != self.result_memory_budget;
        if preferences_changed {
            self.history_enabled = history_enabled;
            self.audit_enabled = audit_enabled;
            self.review_edits_before_save = review_edits_before_save;
            self.update_check_enabled = update_check_enabled;
            self.editor_font_size = editor_font_size;
            self.editor_wrap_lines = editor_wrap_lines;
            self.autocomplete_enabled = autocomplete_enabled;
            self.ghost_suggestions_enabled = ghost_suggestions_enabled;
            self.result_memory_budget = result_memory_budget_mb as usize * 1024 * 1024;
            self.enforce_result_memory_budget();
            self.persist_settings();
        }
    }
}
