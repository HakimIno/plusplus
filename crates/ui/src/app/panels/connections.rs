//! Connections rendering and interaction.

use crate::app::{Action, ConnField, ConnTestState, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

fn field_test_status(state: &ConnTestState, field: ConnField) -> Option<bool> {
    match state {
        ConnTestState::Success => Some(true),
        ConnTestState::Failed { fields, .. } if fields.contains(&field) => Some(false),
        _ => None,
    }
}

fn with_field_status<R>(
    ui: &mut egui::Ui,
    status: Option<bool>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        if let Some(ok) = status {
            let (stroke_color, fill_color) = if ok {
                (
                    egui::Color32::from_rgb(58, 178, 108),
                    egui::Color32::from_rgba_unmultiplied(58, 178, 108, 42),
                )
            } else {
                let danger = palette::DANGER();
                (
                    danger,
                    egui::Color32::from_rgba_unmultiplied(danger.r(), danger.g(), danger.b(), 48),
                )
            };
            let stroke = egui::Stroke::new(1.5_f32, stroke_color);
            let visuals = ui.visuals_mut();
            visuals.extreme_bg_color = fill_color;
            visuals.widgets.inactive.bg_fill = fill_color;
            visuals.widgets.inactive.bg_stroke = stroke;
            visuals.widgets.hovered.bg_fill = fill_color;
            visuals.widgets.hovered.bg_stroke = stroke;
            visuals.widgets.active.bg_fill = fill_color;
            visuals.widgets.active.bg_stroke = stroke;
        }
        add(ui)
    })
    .inner
}

fn status_text_input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    status: Option<bool>,
) -> egui::Response {
    with_field_status(ui, status, |ui| {
        components::text_input(ui, text, hint, width)
    })
}

fn connection_form_label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(96.0, style::CONTROL_H), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            text,
            egui::TextStyle::Body.resolve(ui.style()),
            palette::TEXT(),
        );
    }
    response
}

pub(super) fn connection_color_to_egui(color: dbcore::ConnectionColor) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

fn egui_to_connection_color(color: egui::Color32) -> dbcore::ConnectionColor {
    dbcore::ConnectionColor::new(color.r(), color.g(), color.b())
}

pub(super) fn mix_color(
    base: egui::Color32,
    accent: egui::Color32,
    accent_weight: f32,
) -> egui::Color32 {
    let accent_weight = accent_weight.clamp(0.0, 1.0);
    let base_weight = 1.0 - accent_weight;
    let mix = |base: u8, accent: u8| {
        (base as f32 * base_weight + accent as f32 * accent_weight).round() as u8
    };
    egui::Color32::from_rgb(
        mix(base.r(), accent.r()),
        mix(base.g(), accent.g()),
        mix(base.b(), accent.b()),
    )
}

impl DbGuiApp {
    pub(in crate::app) fn connection_tabs(
        &mut self,
        root: &mut egui::Ui,
        actions: &mut Vec<Action>,
    ) {
        egui::Panel::left("connection_tabs")
            .resizable(false)
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .fill(palette::PANEL()),
            )
            .show_separator_line(false)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                let list_h = ui.available_height();

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), list_h),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("active_connection_tabs")
                            .show(ui, |ui| {
                                let bound_id = self.tabs[self.active_query_tab].conn_id.clone();
                                let mut rects = Vec::with_capacity(self.connections.len());
                                let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
                                for (idx, conn) in self.connections.iter().enumerate() {
                                    let active_conn = self
                                        .active_connections
                                        .iter()
                                        .find(|a| a.config_id == conn.id);
                                    let live = active_conn.is_some();
                                    // Highlight the connection the active tab is bound to.
                                    let selected = bound_id.as_deref() == Some(conn.id.as_str());
                                    let drag_float_y = match (&self.connection_drag, pointer_y) {
                                        (Some(drag), Some(py)) if drag.id == conn.id => {
                                            Some(py - drag.grab_y)
                                        }
                                        _ => None,
                                    };
                                    let resp = components::connection_tab_item(
                                        ui,
                                        &conn.name,
                                        conn.kind,
                                        selected,
                                        live,
                                        drag_float_y,
                                    )
                                    .on_hover_text(format!(
                                        "{}\nSafety: {} — {}",
                                        conn.target_summary(),
                                        conn.safety_profile.label(),
                                        conn.safety_profile.description()
                                    ));
                                    if resp.drag_started() {
                                        self.connection_drag = Some(crate::app::ConnectionDrag {
                                            id: conn.id.clone(),
                                            grab_y: pointer_y.unwrap_or(resp.rect.top())
                                                - resp.rect.top(),
                                        });
                                    }
                                    if resp.clicked() {
                                        if live {
                                            actions.push(Action::BindConnection(idx));
                                        } else {
                                            actions.push(Action::Connect(idx));
                                        }
                                    }
                                    let databases: Vec<String> = active_conn
                                        .map(|a| a.databases.clone())
                                        .unwrap_or_default();
                                    let current_db = conn.database.clone();
                                    resp.context_menu(|ui| {
                                        ui.set_min_width(180.0);
                                        let connect_label =
                                            if live { "Reconnect" } else { "Connect" };
                                        if components::button(
                                            ui,
                                            icons::connect(),
                                            connect_label,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            actions.push(Action::Connect(idx));
                                            ui.close();
                                        }
                                        if live && !databases.is_empty() {
                                            components::menu_button(
                                                ui,
                                                icons::database(),
                                                if conn.kind.is_cql() {
                                                    "Switch Keyspace"
                                                } else {
                                                    "Switch Database"
                                                },
                                                |ui| {
                                                    ui.set_min_width(160.0);
                                                    egui::ScrollArea::vertical()
                                                        .max_height(220.0)
                                                        .show(ui, |ui| {
                                                            for db in &databases {
                                                                let is_current = *db == current_db;
                                                                let tint = ui
                                                                    .visuals()
                                                                    .widgets
                                                                    .inactive
                                                                    .fg_stroke
                                                                    .color;
                                                                let db_img = egui::Image::new(
                                                                    icons::database(),
                                                                )
                                                                .fit_to_exact_size(egui::vec2(
                                                                    14.0, 14.0,
                                                                ))
                                                                .tint(tint);
                                                                let label = if is_current {
                                                                    format!("✓  {db}")
                                                                } else {
                                                                    db.clone()
                                                                };
                                                                let btn =
                                                                    egui::Button::image_and_text(
                                                                        db_img, label,
                                                                    )
                                                                    .min_size(egui::vec2(
                                                                        ui.available_width(),
                                                                        0.0,
                                                                    ));
                                                                if ui
                                                                    .add_enabled(!is_current, btn)
                                                                    .clicked()
                                                                {
                                                                    actions.push(
                                                                        Action::SwitchDatabase {
                                                                            conn_idx: idx,
                                                                            database: db.clone(),
                                                                        },
                                                                    );
                                                                    ui.close();
                                                                }
                                                            }
                                                        });
                                                },
                                            );
                                        }
                                        if live {
                                            ui.separator();
                                            if components::button(
                                                ui,
                                                icons::database_export(),
                                                "Backup Database…",
                                                true,
                                            )
                                            .clicked()
                                            {
                                                actions.push(Action::OpenBackup {
                                                    conn_idx: idx,
                                                    restore: false,
                                                });
                                                ui.close();
                                            }
                                            if components::button(
                                                ui,
                                                icons::database_import(),
                                                "Restore Database…",
                                                !conn.is_read_only(),
                                            )
                                            .on_disabled_hover_text("This connection is read-only")
                                            .clicked()
                                            {
                                                actions.push(Action::OpenBackup {
                                                    conn_idx: idx,
                                                    restore: true,
                                                });
                                                ui.close();
                                            }
                                            ui.separator();
                                        }
                                        if components::button(ui, icons::edit(), "Edit…", true)
                                            .clicked()
                                        {
                                            actions.push(Action::EditConnection(idx));
                                            ui.close();
                                        }
                                        if live
                                            && components::button(
                                                ui,
                                                icons::disconnect(),
                                                "Disconnect",
                                                true,
                                            )
                                            .clicked()
                                        {
                                            actions.push(Action::DisconnectConn(idx));
                                            ui.close();
                                        }
                                        if components::button(ui, icons::trash(), "Delete", true)
                                            .clicked()
                                        {
                                            actions.push(Action::DeleteConnection(idx));
                                            ui.close();
                                        }
                                    });
                                    rects.push(resp.rect);
                                    ui.add_space(2.0);
                                }

                                self.handle_connection_drag(ui, &rects, actions);

                                if self.connections.is_empty() {
                                    ui.vertical_centered(|ui| {
                                        icons::show_native(ui, icons::database(), 16.0);
                                    });
                                }
                            });
                    },
                );
            });
    }

    /// While a saved connection is dragged, live-reorder it into the vertical slot under
    /// the pointer. The persisted connection list order follows the visible order.
    fn handle_connection_drag(
        &mut self,
        ui: &egui::Ui,
        rects: &[egui::Rect],
        actions: &mut Vec<Action>,
    ) {
        let Some(drag) = self.connection_drag.clone() else {
            return;
        };
        if !ui.input(|i| i.pointer.primary_down()) {
            self.connection_drag = None;
            return;
        }
        let Some(from) = self.connections.iter().position(|c| c.id == drag.id) else {
            self.connection_drag = None;
            return;
        };
        if from >= rects.len() {
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let float_center = pointer.y - drag.grab_y + rects[from].height() * 0.5;
        let to = rects
            .iter()
            .enumerate()
            .filter(|(i, r)| *i != from && float_center > r.center().y)
            .count();
        if to != from {
            actions.push(Action::MoveConnection { from, to });
        }
    }

    pub(in crate::app) fn connection_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.selecting_provider {
            "Choose a database"
        } else if editor.is_new {
            "New Connection"
        } else {
            "Edit Connection"
        };
        let mut open = true;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                let test_state = editor.test_state.clone();
                let mut form_changed = false;

                if editor.selecting_provider {
                    ui.set_min_width(536.0);

                    ui.add_space(10.0);

                    egui::Grid::new("connection_provider_grid")
                        .num_columns(4)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (index, kind) in [
                                dbcore::DbKind::Postgres,
                                dbcore::DbKind::MySql,
                                dbcore::DbKind::MariaDb,
                                dbcore::DbKind::SqlServer,
                                dbcore::DbKind::Sqlite,
                                dbcore::DbKind::DuckDb,
                                dbcore::DbKind::Cassandra,
                                dbcore::DbKind::ScyllaDb,
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if components::db_kind_card(ui, kind).clicked() {
                                    editor.config.kind = kind;
                                    editor.config.port = kind.default_port();
                                    editor.test_state = ConnTestState::Untested;
                                    editor.selecting_provider = false;
                                }
                                if (index + 1) % 4 == 0 {
                                    ui.end_row();
                                }
                            }
                        });

                    components::dialog_footer(ui, |ui| {
                        if components::button(ui, icons::close(), "Cancel", true).clicked() {
                            actions.push(Action::CancelDialog);
                        }
                    });
                    return;
                }

                ui.set_min_width(600.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(icons::db_kind_icon(editor.config.kind))
                            .fit_to_exact_size(egui::Vec2::splat(28.0))
                            .tint(icons::db_kind_icon_tint()),
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(editor.config.kind.label())
                                .size(13.0)
                                .color(palette::TEXT()),
                        );
                        ui.label(
                            egui::RichText::new("Connection details")
                                .size(10.5)
                                .color(palette::TEXT_FAINT()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if components::button(ui, icons::chevron_left(), "Change", true).clicked() {
                            editor.selecting_provider = true;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.label(
                    egui::RichText::new("GENERAL")
                        .size(10.0)
                        .color(palette::TEXT_FAINT()),
                );
                ui.add_space(3.0);
                egui::Frame::new()
                    .fill(palette::SURFACE())
                    .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        egui::Grid::new("conn_general")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                connection_form_label(ui, "Name");
                                form_changed |= status_text_input(
                                    ui,
                                    &mut editor.config.name,
                                    "",
                                    440.0,
                                    field_test_status(&test_state, ConnField::Name),
                                )
                                .changed();
                                ui.end_row();
                            });
                    });

                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("CONNECTION")
                        .size(10.0)
                        .color(palette::TEXT_FAINT()),
                );
                ui.add_space(3.0);
                egui::Frame::new()
                    .fill(palette::SURFACE())
                    .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        egui::Grid::new("conn_form")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                let field_w = 440.0;

                                if editor.show_advanced {
                                    connection_form_label(ui, "Title bar color");
                                    ui.horizontal(|ui| {
                                        let mut color = editor
                                            .config
                                            .title_bar_color
                                            .map(connection_color_to_egui)
                                            .unwrap_or_else(palette::ACCENT);
                                        if egui::color_picker::color_edit_button_srgba(
                                            ui,
                                            &mut color,
                                            egui::color_picker::Alpha::Opaque,
                                        )
                                        .changed()
                                        {
                                            editor.config.title_bar_color =
                                                Some(egui_to_connection_color(color));
                                            form_changed = true;
                                        }
                                        if editor.config.title_bar_color.is_none() {
                                            ui.label("Default");
                                        }
                                        if ui.button("Clear").clicked()
                                            && editor.config.title_bar_color.take().is_some()
                                        {
                                            form_changed = true;
                                        }
                                    });
                                    ui.end_row();

                                    connection_form_label(ui, "Safety profile");
                                    let previous_profile = editor.config.safety_profile;
                                    egui::ComboBox::from_id_salt("safety_profile")
                                        .selected_text(editor.config.safety_profile.label())
                                        .width(field_w)
                                        .show_ui(ui, |ui| {
                                            for profile in dbcore::SafetyProfile::ALL {
                                                ui.selectable_value(
                                                    &mut editor.config.safety_profile,
                                                    profile,
                                                    profile.label(),
                                                )
                                                .on_hover_text(profile.description());
                                            }
                                        });
                                    if editor.config.safety_profile != previous_profile {
                                        editor
                                            .config
                                            .set_safety_profile(editor.config.safety_profile);
                                        form_changed = true;
                                    }
                                    ui.end_row();

                                    connection_form_label(ui, "");
                                    ui.vertical(|ui| {
                                        ui.set_max_width(field_w);
                                        ui.label(
                                            egui::RichText::new(
                                                editor.config.safety_profile.description(),
                                            )
                                            .size(11.0)
                                            .color(palette::TEXT_WEAK()),
                                        );
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            ui.label(egui::RichText::new("Protection").size(11.0));
                                            let guardian_on = editor.config.is_production();
                                            ui.label(
                                                egui::RichText::new(if guardian_on {
                                                    "Guardian: On"
                                                } else {
                                                    "Guardian: Off"
                                                })
                                                .size(11.0)
                                                .color(if guardian_on {
                                                    palette::SUCCESS()
                                                } else {
                                                    palette::TEXT_FAINT()
                                                }),
                                            );
                                            ui.label(
                                                egui::RichText::new("·")
                                                    .color(palette::TEXT_FAINT()),
                                            );
                                            let read_only_on = editor.config.is_read_only();
                                            ui.label(
                                                egui::RichText::new(if read_only_on {
                                                    "Read-only: On"
                                                } else {
                                                    "Read-only: Off"
                                                })
                                                .size(11.0)
                                                .color(if read_only_on {
                                                    palette::SUCCESS()
                                                } else {
                                                    palette::TEXT_FAINT()
                                                }),
                                            );
                                        });
                                    });
                                    ui.end_row();

                                    if editor.config.safety_profile == dbcore::SafetyProfile::Custom
                                    {
                                        connection_form_label(ui, "Production");
                                        form_changed |= ui
                                    .checkbox(
                                        &mut editor.config.production,
                                        "Confirm destructive queries",
                                    )
                                    .on_hover_text(
                                        "UPDATE, DELETE, DROP, TRUNCATE, ALTER, and MERGE must \
                                     be confirmed in a dialog before they run",
                                    )
                                    .changed();
                                        ui.end_row();

                                        connection_form_label(ui, "Read-only");
                                        form_changed |= ui
                                    .checkbox(&mut editor.config.read_only, "Block all writes")
                                    .on_hover_text(
                                        "Only reads (SELECT, SHOW, EXPLAIN, …) are allowed to \
                                     run; in-grid editing and schema changes are refused. \
                                     Where the database supports it the session itself is \
                                     opened read-only, so even writes hidden inside \
                                     functions are rejected by the server. Takes effect on \
                                     the next connect.",
                                    )
                                    .changed();
                                        ui.end_row();
                                    }
                                }

                                if editor.config.kind.is_server() {
                                    connection_form_label(ui, "Host");
                                    ui.horizontal(|ui| {
                                        form_changed |= status_text_input(
                                            ui,
                                            &mut editor.config.host,
                                            "",
                                            292.0,
                                            field_test_status(&test_state, ConnField::Host),
                                        )
                                        .changed();
                                        ui.add_space(8.0);
                                        ui.label("Port");
                                        form_changed |= with_field_status(
                                            ui,
                                            field_test_status(&test_state, ConnField::Port),
                                            |ui| {
                                                ui.add_sized(
                                                    egui::vec2(84.0, style::CONTROL_H),
                                                    egui::DragValue::new(&mut editor.config.port),
                                                )
                                            },
                                        )
                                        .changed();
                                    });
                                    ui.end_row();

                                    connection_form_label(ui, "User");
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.user,
                                        "",
                                        field_w,
                                        field_test_status(&test_state, ConnField::User),
                                    )
                                    .changed();
                                    ui.end_row();

                                    connection_form_label(ui, "Password");
                                    form_changed |= with_field_status(
                                        ui,
                                        field_test_status(&test_state, ConnField::Password),
                                        |ui| {
                                            components::password_input(
                                                ui,
                                                &mut editor.password,
                                                "",
                                                field_w,
                                            )
                                        },
                                    )
                                    .changed();
                                    ui.end_row();

                                    // CQL groups tables into keyspaces, not databases; label the
                                    // field with the term the user expects to type there.
                                    connection_form_label(
                                        ui,
                                        if editor.config.kind.is_cql() {
                                            "Keyspace"
                                        } else {
                                            "Database"
                                        },
                                    );
                                    form_changed |= status_text_input(
                                        ui,
                                        &mut editor.config.database,
                                        "",
                                        field_w,
                                        field_test_status(&test_state, ConnField::Database),
                                    )
                                    .changed();
                                    ui.end_row();

                                    if editor.show_advanced {
                                        connection_form_label(ui, "SSL mode");
                                        let previous_ssl = editor.config.ssl_mode;
                                        egui::ComboBox::from_id_salt("ssl_mode")
                                            .selected_text(editor.config.ssl_mode.label())
                                            .show_ui(ui, |ui| {
                                                for mode in dbcore::SslMode::ALL {
                                                    ui.selectable_value(
                                                        &mut editor.config.ssl_mode,
                                                        mode,
                                                        mode.label(),
                                                    );
                                                }
                                            });
                                        form_changed |= editor.config.ssl_mode != previous_ssl;
                                        ui.end_row();

                                        // Flag the modes that don't verify the server's identity, so the
                                        // weaker choices read as a deliberate trade-off rather than a default.
                                        if let Some(warning) =
                                            editor.config.ssl_mode.security_warning()
                                        {
                                            connection_form_label(ui, "");
                                            ui.horizontal_wrapped(|ui| {
                                                icons::show_colored(
                                                    ui,
                                                    icons::warning(),
                                                    12.0,
                                                    palette::WARNING(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(warning)
                                                        .size(11.0)
                                                        .color(palette::WARNING()),
                                                );
                                            });
                                            ui.end_row();
                                        }

                                        if editor.config.ssl_mode.verifies_certificate() {
                                            connection_form_label(ui, "CA certificate");
                                            ui.horizontal(|ui| {
                                                form_changed |= status_text_input(
                                                    ui,
                                                    &mut editor.config.ssl_ca_cert,
                                                    "System trust store",
                                                    field_w,
                                                    None,
                                                )
                                                .changed();
                                                if ui.button("Browse…").clicked() {
                                                    actions.push(Action::BrowseSslCaCert);
                                                }
                                            });
                                            ui.end_row();
                                        }

                                        if editor.config.kind.supports_client_cert()
                                            && editor.config.ssl_mode != dbcore::SslMode::Disable
                                        {
                                            connection_form_label(ui, "Client certificate");
                                            ui.horizontal(|ui| {
                                                form_changed |= status_text_input(
                                                    ui,
                                                    &mut editor.config.ssl_client_cert,
                                                    "None",
                                                    field_w,
                                                    None,
                                                )
                                                .changed();
                                                if ui.button("Browse…").clicked() {
                                                    actions.push(Action::BrowseSslClientCert);
                                                }
                                            });
                                            ui.end_row();

                                            connection_form_label(ui, "Client key");
                                            ui.horizontal(|ui| {
                                                form_changed |= status_text_input(
                                                    ui,
                                                    &mut editor.config.ssl_client_key,
                                                    "None",
                                                    field_w,
                                                    None,
                                                )
                                                .changed();
                                                if ui.button("Browse…").clicked() {
                                                    actions.push(Action::BrowseSslClientKey);
                                                }
                                            });
                                            ui.end_row();
                                        }

                                        connection_form_label(ui, "SSH tunnel");
                                        form_changed |= ui
                                            .checkbox(
                                                &mut editor.config.ssh_enabled,
                                                "Connect through a bastion host",
                                            )
                                            .on_hover_text(
                                                "Host and port above are then resolved from the \
                                     bastion, not from this machine",
                                            )
                                            .changed();
                                        ui.end_row();

                                        if editor.config.ssh_enabled {
                                            connection_form_label(ui, "SSH host");
                                            form_changed |= status_text_input(
                                                ui,
                                                &mut editor.config.ssh_host,
                                                "bastion.example.com",
                                                field_w,
                                                None,
                                            )
                                            .changed();
                                            ui.end_row();

                                            connection_form_label(ui, "SSH port");
                                            form_changed |= ui
                                                .add_sized(
                                                    egui::vec2(80.0, style::CONTROL_H),
                                                    egui::DragValue::new(
                                                        &mut editor.config.ssh_port,
                                                    ),
                                                )
                                                .changed();
                                            ui.end_row();

                                            connection_form_label(ui, "SSH user");
                                            form_changed |= status_text_input(
                                                ui,
                                                &mut editor.config.ssh_user,
                                                "",
                                                field_w,
                                                None,
                                            )
                                            .changed();
                                            ui.end_row();

                                            connection_form_label(ui, "SSH key");
                                            ui.horizontal(|ui| {
                                                form_changed |= status_text_input(
                                                    ui,
                                                    &mut editor.config.ssh_key_path,
                                                    "None — use password",
                                                    field_w,
                                                    None,
                                                )
                                                .changed();
                                                if ui.button("Browse…").clicked() {
                                                    actions.push(Action::BrowseSshKey);
                                                }
                                            });
                                            ui.end_row();

                                            connection_form_label(
                                                ui,
                                                if editor.config.ssh_key_path.trim().is_empty() {
                                                    "SSH password"
                                                } else {
                                                    "Key passphrase"
                                                },
                                            );
                                            form_changed |= components::password_input(
                                                ui,
                                                &mut editor.ssh_password,
                                                "",
                                                field_w,
                                            )
                                            .changed();
                                            ui.end_row();
                                        }
                                    }
                                } else {
                                    connection_form_label(ui, "File");
                                    ui.horizontal(|ui| {
                                        let (path, hint) =
                                            if editor.config.kind == dbcore::DbKind::DuckDb {
                                                (
                                                    &mut editor.config.duckdb_path,
                                                    "/path/to/analytics.duckdb",
                                                )
                                            } else {
                                                (
                                                    &mut editor.config.sqlite_path,
                                                    "/path/to/database.sqlite",
                                                )
                                            };
                                        form_changed |= status_text_input(
                                            ui,
                                            path,
                                            hint,
                                            350.0,
                                            field_test_status(&test_state, ConnField::SqlitePath),
                                        )
                                        .changed();
                                        if ui.button("Browse…").clicked() {
                                            actions.push(Action::BrowseSqlitePath);
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = if editor.show_advanced {
                        "Hide advanced"
                    } else {
                        "Advanced settings"
                    };
                    if components::button(ui, icons::settings(), label, true).clicked() {
                        editor.show_advanced = !editor.show_advanced;
                    }
                    if !editor.show_advanced {
                        ui.label(
                            egui::RichText::new("Appearance, safety, SSL & SSH")
                                .size(10.5)
                                .color(palette::TEXT_FAINT()),
                        );
                    }
                });
                ui.add_space(2.0);
                match &editor.test_state {
                    ConnTestState::Testing(_) => {
                        ui.horizontal(|ui| {
                            ui.add(components::spinner(style::CONTROL_H));
                            ui.label("Testing connection…");
                        });
                    }
                    ConnTestState::Success => {
                        ui.colored_label(
                            egui::Color32::from_rgb(58, 178, 108),
                            "Connection test succeeded",
                        );
                    }
                    ConnTestState::Failed { message, .. } => {
                        ui.colored_label(palette::DANGER(), message);
                    }
                    ConnTestState::Untested => {}
                }
                if form_changed && !matches!(editor.test_state, ConnTestState::Testing(_)) {
                    editor.test_state = ConnTestState::Untested;
                }
                components::dialog_footer(ui, |ui| {
                    let testing = matches!(editor.test_state, ConnTestState::Testing(_));
                    // Footer paints right-to-left: first widget is rightmost (Save).
                    if components::button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SaveConnection);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDialog);
                    }
                    if components::button(ui, icons::connect(), "Test", !testing).clicked() {
                        actions.push(Action::TestConnection);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelDialog);
        }
    }
}
