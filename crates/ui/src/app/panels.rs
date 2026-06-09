use super::{Action, Busy, DbGuiApp};
use crate::filter::{self, FilterEvent};
use crate::grid::results_grid;
use crate::icons;
use crate::style::{self, palette};
use crate::theme::ThemeId;

impl DbGuiApp {
    pub(super) fn top_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("top_bar")
            .resizable(false)
            .exact_size(34.0)
            .show_inside(root, |ui| {
                ui.add_space(3.0);
                ui.horizontal_centered(|ui| {
                    ui.add_space(6.0);
                    let connected = self.active().is_some();

                    if super::widgets::toolbar_icon_button(ui, icons::plus(), "New connection")
                        .clicked()
                    {
                        actions.push(Action::NewConnection);
                    }
                    ui.add_space(4.0);

                    if super::widgets::toolbar_icon_button(ui, icons::disconnect(), "Disconnect")
                        .clicked()
                        && connected
                    {
                        actions.push(Action::Disconnect);
                    }
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    let has_result = self.result.is_some();
                    if super::widgets::toolbar_icon_button(
                        ui,
                        icons::filter(),
                        "Show / hide filter",
                    )
                    .clicked()
                        && has_result
                    {
                        self.filter.visible = !self.filter.visible;
                    }
                    ui.add_space(4.0);

                    icons::show_weak(ui, icons::table(), 18.0).on_hover_text("Tables");
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    icons::show_weak(ui, icons::key(), 18.0).on_hover_text("Keys / permissions");
                    ui.add_space(6.0);

                    icons::show_weak(ui, icons::database(), 18.0).on_hover_text("Database");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("SQL")
                                .size(12.0)
                                .strong()
                                .color(palette::TEXT_WEAK()),
                        );
                        ui.add_space(8.0);
                        self.theme_picker(ui);

                        match self.busy {
                            Busy::Connecting => {
                                ui.add_space(2.0);
                                ui.colored_label(palette::TEXT_WEAK(), "connecting…");
                                ui.spinner();
                            }
                            Busy::Querying => {
                                ui.add_space(2.0);
                                ui.colored_label(palette::TEXT_WEAK(), "running…");
                                ui.spinner();
                            }
                            Busy::Idle => {}
                        }
                    });
                });
            });
    }

    /// A small combo box for choosing the colour theme. Switching applies immediately and
    /// the choice is remembered across launches.
    fn theme_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.theme;
        egui::ComboBox::from_id_salt("theme_picker")
            .selected_text(self.theme.label())
            .show_ui(ui, |ui| {
                for id in ThemeId::ALL {
                    ui.selectable_value(&mut chosen, id, id.label());
                }
            });
        if chosen != self.theme {
            self.set_theme(ui.ctx(), chosen);
        }
    }

    /// Thin bar between the grid and the SQL console: row count / selection / errors.
    pub(super) fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show_inside(root, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                if let Some(err) = &self.error {
                    icons::show_colored(ui, icons::warning(), 15.0, palette::DANGER());
                    ui.colored_label(palette::DANGER(), err);
                } else {
                    icons::show_weak(ui, icons::table(), 14.0);
                    ui.colored_label(palette::TEXT_WEAK(), &self.status_msg);
                    if let Some(res) = &self.result {
                        if self.filter.is_active() && self.row_order.len() != res.row_count() {
                            ui.colored_label(palette::TEXT_FAINT(), "·");
                            icons::show_colored(ui, icons::filter(), 13.0, palette::ACCENT());
                            ui.colored_label(
                                palette::ACCENT(),
                                format!("{} of {} rows", self.row_order.len(), res.row_count()),
                            );
                        }
                    }
                    if let (Some(sel), true) = (self.selected_row, self.result.is_some()) {
                        ui.colored_label(palette::TEXT_FAINT(), "·");
                        ui.colored_label(palette::TEXT_WEAK(), format!("row {}", sel + 1));
                    }
                }
            });
            ui.add_space(3.0);
        });
    }

    /// SQL editor at the very bottom, with syntax highlighting and a Run button.
    pub(super) fn query_console(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::bottom("query_console")
            .resizable(true)
            .default_size(150.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::section_header(ui, "Query");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_run = self.active().is_some() && self.busy == Busy::Idle;
                        if icons::primary_button(ui, icons::play(), "Run", can_run)
                            .on_hover_text("Cmd/Ctrl+Enter")
                            .clicked()
                        {
                            actions.push(Action::RunQuery);
                        }
                    });
                });
                ui.add_space(6.0);

                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = crate::highlight::highlight_sql(buf.as_str(), font.clone());
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };

                egui::ScrollArea::vertical()
                    .id_salt("sql_scroll")
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.sql)
                                .code_editor()
                                .desired_rows(5)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .hint_text("Write SQL here, then press Run (Cmd/Ctrl+Enter)"),
                        );
                    });
                ui.add_space(4.0);
            });
    }

    /// Right-hand Details panel: the selected row's columns and values.
    pub(super) fn right_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::right("details_panel")
            .resizable(true)
            .default_size(260.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                style::section_header(ui, "Details");
                ui.separator();

                let selected = match (self.result.as_ref(), self.selected_row) {
                    (Some(res), Some(disp)) if disp < self.row_order.len() => {
                        Some((res, self.row_order[disp]))
                    }
                    _ => None,
                };

                let Some((res, row_idx)) = selected else {
                    style::empty_state(
                        ui,
                        icons::table(),
                        "No row selected",
                        "Click a row to inspect it",
                    );
                    return;
                };

                egui::ScrollArea::vertical()
                    .id_salt("details_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("details_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                for (c, col) in res.columns.iter().enumerate() {
                                    ui.weak(&col.name);
                                    let value = &res.rows[row_idx][c];
                                    if value.is_null() {
                                        ui.weak("NULL");
                                    } else {
                                        ui.add(egui::Label::new(value.display()).wrap());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
    }

    pub(super) fn connection_tabs(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("connection_tabs")
            .resizable(false)
            .exact_size(68.0)
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
                                for (idx, conn) in self.connections.iter().enumerate() {
                                    let active_idx = self
                                        .active_connections
                                        .iter()
                                        .position(|active| active.config_id == conn.id);
                                    let selected =
                                        active_idx.is_some_and(|i| self.active_tab == Some(i));
                                    let resp = super::widgets::connection_tab_item(
                                        ui,
                                        &conn.name,
                                        selected,
                                        active_idx.is_some(),
                                    )
                                    .on_hover_text(conn.target_summary());
                                    if resp.clicked() {
                                        if let Some(active_idx) = active_idx {
                                            actions.push(Action::SelectActive(active_idx));
                                        } else {
                                            actions.push(Action::Connect(idx));
                                        }
                                    }
                                    resp.context_menu(|ui| {
                                        if icons::button(ui, icons::connect(), "Connect", true)
                                            .clicked()
                                        {
                                            actions.push(Action::Connect(idx));
                                            ui.close();
                                        }
                                        if icons::button(ui, icons::edit(), "Edit…", true).clicked()
                                        {
                                            actions.push(Action::EditConnection(idx));
                                            ui.close();
                                        }
                                        if let Some(active_idx) = active_idx {
                                            if icons::button(ui, icons::close(), "Close tab", true)
                                                .clicked()
                                            {
                                                actions.push(Action::CloseActive(active_idx));
                                                ui.close();
                                            }
                                        }
                                        if icons::button(ui, icons::trash(), "Delete", true)
                                            .clicked()
                                        {
                                            actions.push(Action::DeleteConnection(idx));
                                            ui.close();
                                        }
                                    });
                                    ui.add_space(2.0);
                                }

                                if self.connections.is_empty() {
                                    ui.vertical_centered(|ui| {
                                        icons::show_weak(ui, icons::database(), 18.0);
                                    });
                                }
                            });
                    },
                );
            });
    }

    pub(super) fn left_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(280.0)
            .min_size(200.0)
            .max_size(360.0)
            .show_inside(root, |ui| {
                ui.add_space(8.0);
                style::section_header(ui, "Schema");
                ui.horizontal(|ui| {
                    icons::show_weak(ui, icons::filter(), 15.0);
                    let w = ui.available_width();
                    style::text_input(ui, &mut self.schema_filter, "filter tables…", w);
                });
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("schema_scroll")
                    .show(ui, |ui| {
                        // Keep tree content within the panel — long names must not widen it.
                        ui.set_width(ui.available_width());
                        self.schema_tree(ui, actions);
                    });
            });
    }

    fn schema_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            ui.add_space(4.0);
            ui.colored_label(
                palette::TEXT_FAINT(),
                "Connect to a database to browse its schema.",
            );
            return;
        };

        ui.horizontal(|ui| {
            icons::show(ui, icons::database(), icons::SIZE);
            style::truncated_label(
                ui,
                &active.schema.database_name,
                None,
                false,
                egui::Sense::hover(),
            );
        });
        ui.add_space(2.0);

        let filter = self.schema_filter.to_lowercase();
        for table in &active.schema.tables {
            if !filter.is_empty() && !table.name.to_lowercase().contains(&filter) {
                continue;
            }
            let id = ui.make_persistent_id(("tbl", table.name.as_str()));
            let (_toggle, header, _body) =
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                )
                .show_header(ui, |ui| {
                    icons::show_weak(ui, icons::table(), 15.0);
                    ui.add_space(2.0);
                    style::truncated_label(ui, &table.name, None, false, egui::Sense::click())
                })
                .body(|ui| {
                    for col in &table.columns {
                        ui.horizontal(|ui| {
                            let glyph = if col.primary_key {
                                icons::key()
                            } else {
                                icons::column()
                            };
                            icons::show_weak(ui, glyph, 13.0);
                            ui.add_space(2.0);
                            style::truncated_label(
                                ui,
                                &col.name,
                                None,
                                false,
                                egui::Sense::hover(),
                            );
                            let nn = if col.nullable { "" } else { " · not null" };
                            let meta = format!("{}{nn}", col.data_type);
                            style::truncated_label(
                                ui,
                                &meta,
                                Some(&meta),
                                true,
                                egui::Sense::hover(),
                            );
                        });
                    }
                    if !table.indexes.is_empty() {
                        ui.add_space(3.0);
                        for idx in &table.indexes {
                            ui.horizontal(|ui| {
                                icons::show_weak(ui, icons::index(), 13.0);
                                ui.add_space(2.0);
                                let u = if idx.unique { "unique " } else { "" };
                                let detail =
                                    format!("{u}{} ({})", idx.name, idx.columns.join(", "));
                                style::truncated_label(
                                    ui,
                                    &detail,
                                    Some(&detail),
                                    true,
                                    egui::Sense::hover(),
                                );
                            });
                        }
                    }
                });

            let resp = header.inner.on_hover_text("Click to preview rows");
            if resp.clicked() {
                actions.push(Action::OpenTable(
                    active.db.kind().preview_query(&table.qualified(), 100),
                ));
            }
        }
    }

    /// The TablePlus-style filter strip directly above the grid. Only shown when toggled on
    /// and a result with columns is loaded. Edits mutate `self.filter` directly; an Apply or
    /// Clear rebuilds the view.
    pub(super) fn filter_bar(&mut self, root: &mut egui::Ui) {
        if !self.filter.visible {
            return;
        }
        let col_names: Vec<String> = match &self.result {
            Some(res) if res.column_count() > 0 => {
                res.columns.iter().map(|c| c.name.clone()).collect()
            }
            _ => return,
        };

        let mut event: Option<FilterEvent> = None;
        egui::Panel::top("filter_bar")
            .resizable(false)
            .show_inside(root, |ui| {
                event = filter::ui(ui, &mut self.filter, &col_names);
            });

        match event {
            Some(FilterEvent::Apply) => self.recompute_view(),
            Some(FilterEvent::Clear) => {
                self.filter.reset();
                self.recompute_view();
            }
            None => {}
        }
    }

    pub(super) fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::CentralPanel::default().show_inside(root, |ui| match &self.result {
            Some(result) if result.column_count() > 0 => {
                let resp = results_grid(ui, result, &self.row_order, self.sort, self.selected_row);
                if let Some(col) = resp.sort {
                    actions.push(Action::SortBy(col));
                }
                if let Some(row) = resp.selected {
                    self.selected_row = Some(row);
                }
            }
            Some(_) => {
                style::empty_state(ui, icons::table(), "No columns", &self.status_msg);
            }
            None => {
                style::empty_state(
                    ui,
                    icons::play(),
                    "No results yet",
                    "Write a query below and press Run (Cmd/Ctrl+Enter)",
                );
            }
        });
    }

    pub(super) fn connection_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.is_new {
            "New Connection"
        } else {
            "Edit Connection"
        };
        let mut open = true;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("conn_form")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        let field_w = 240.0;

                        ui.label("Name");
                        style::text_input(ui, &mut editor.config.name, "", field_w);
                        ui.end_row();

                        ui.label("Type");
                        let previous_kind = editor.config.kind;
                        egui::ComboBox::from_id_salt("kind")
                            .selected_text(editor.config.kind.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::Postgres,
                                    "PostgreSQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::MySql,
                                    "MySQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::MariaDb,
                                    "MariaDB",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::SqlServer,
                                    "SQL Server",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    dbcore::DbKind::Sqlite,
                                    "SQLite",
                                );
                            });
                        if editor.config.kind != previous_kind {
                            editor.config.port = editor.config.kind.default_port();
                        }
                        ui.end_row();

                        if editor.config.kind.is_server() {
                            ui.label("Host");
                            style::text_input(ui, &mut editor.config.host, "", field_w);
                            ui.end_row();

                            ui.label("Port");
                            ui.add_sized(
                                egui::vec2(80.0, style::CONTROL_H),
                                egui::DragValue::new(&mut editor.config.port),
                            );
                            ui.end_row();

                            ui.label("User");
                            style::text_input(ui, &mut editor.config.user, "", field_w);
                            ui.end_row();

                            ui.label("Password");
                            ui.add_sized(
                                egui::vec2(field_w, style::CONTROL_H),
                                egui::TextEdit::singleline(&mut editor.password)
                                    .password(true)
                                    .vertical_align(egui::Align::Center)
                                    .margin(egui::Margin::symmetric(6, 0)),
                            );
                            ui.end_row();

                            ui.label("Database");
                            style::text_input(ui, &mut editor.config.database, "", field_w);
                            ui.end_row();
                        } else {
                            ui.label("File");
                            ui.horizontal(|ui| {
                                style::text_input(
                                    ui,
                                    &mut editor.config.sqlite_path,
                                    "/path/to/database.sqlite",
                                    field_w,
                                );
                                if ui.button("Browse…").clicked() {
                                    actions.push(Action::BrowseSqlitePath);
                                }
                            });
                            ui.end_row();
                        }
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if icons::button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SaveConnection);
                    }
                    if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDialog);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelDialog);
        }
    }
}
