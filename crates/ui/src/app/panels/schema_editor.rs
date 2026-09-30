//! Schema editor rendering and interaction.

use super::schema_grid::schema_columns_tab;
use super::schema_grid::schema_fk_tab;
use super::schema_grid::schema_grid_keyboard;
use super::schema_grid::schema_indexes_grid;
use super::schema_grid::schema_indexes_tab;
use super::schema_grid::schema_structure_grid;
use super::schema_grid::SchemaStructureGridState;
use crate::app::{Action, DbGuiApp, TabView};
use crate::components;
use crate::icons;
use crate::style::palette;

/// The header (title + Apply / Cancel buttons) shared by every object editor. Returns
/// nothing; the buttons push actions directly.
fn object_editor_header(ui: &mut egui::Ui, actions: &mut Vec<Action>, title: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(16.0)
                .strong()
                .color(palette::TEXT()),
        );
        // Action buttons on the right of the header, where the eye lands first.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::primary_button(ui, icons::play(), "Apply", true)
                .on_hover_text("Apply the generated DDL as a single transaction")
                .clicked()
            {
                actions.push(Action::GenerateSchema);
            }
            ui.add_space(6.0);
            if components::button(ui, icons::close(), "Cancel", true).clicked() {
                actions.push(Action::CancelSchema);
            }
        });
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(10.0);
}

/// Compact metadata bar used when an existing table is edited in-place. Keeping this separate
/// from the create-table header makes Structure/Indexes read like data grids, not dialog forms.
fn embedded_table_editor_header(
    ui: &mut egui::Ui,
    editor: &mut crate::schema::SchemaEditor,
    table_section: Option<TabView>,
) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Name")
                .size(11.0)
                .strong()
                .color(palette::TEXT_WEAK()),
        );
        components::text_input(ui, &mut editor.table_name, "table_name", 240.0);

        let primary_columns = editor
            .columns
            .iter()
            .filter(|column| column.primary_key && !column.drop)
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        if !primary_columns.is_empty() || editor.db_kind == dbcore::DbKind::SqlServer {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("Primary")
                    .size(11.0)
                    .strong()
                    .color(palette::TEXT_WEAK()),
            );
        }
        if editor.db_kind == dbcore::DbKind::SqlServer {
            let primary_text = if primary_columns.is_empty() {
                "No primary key".to_string()
            } else {
                primary_columns.join(", ")
            };
            egui::ComboBox::from_id_salt("structure_primary_key")
                .width(220.0)
                .selected_text(primary_text)
                .icon(components::combo_chevron_icon)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    ui.set_min_width(220.0);
                    for column in editor.columns.iter_mut().filter(|column| !column.drop) {
                        let name = column.name.clone();
                        if components::menu_checkbox(ui, &mut column.primary_key, &name).changed()
                            && column.primary_key
                        {
                            // SQL Server primary-key columns must be NOT NULL.
                            column.nullable = false;
                        }
                    }
                });
        } else {
            for column in primary_columns {
                components::type_badge(ui, &column, palette::ACCENT());
            }
        }

        if table_section == Some(TabView::Structure) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search_width = 280.0_f32.min(ui.available_width());
                components::icon_text_input(
                    ui,
                    &mut editor.column_filter,
                    "Search for column…",
                    icons::search(),
                    search_width,
                );
            });
        }
    });
    ui.add_space(8.0);
    ui.separator();
}

/// Render the table create/edit form (columns, indexes, foreign keys) into the central panel.
fn table_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::SchemaEditor,
    table_section: Option<TabView>,
) {
    use crate::schema::{SchemaEditorMode, SchemaTab};
    let title = match (table_section, editor.mode) {
        (Some(TabView::Structure), _) => format!("Structure — {}", editor.table_name),
        (Some(TabView::Indexes), _) => format!("Indexes — {}", editor.table_name),
        (_, SchemaEditorMode::New) => "Create Table".to_string(),
        (_, SchemaEditorMode::Edit) => format!("Structure — {}", editor.table_name),
        (_, SchemaEditorMode::DesignNew) => "Add Table to ER Design".to_string(),
        (_, SchemaEditorMode::DesignEdit) => format!("Edit ER Table — {}", editor.table_name),
    };
    let design_mode = matches!(
        editor.mode,
        SchemaEditorMode::DesignNew | SchemaEditorMode::DesignEdit
    );
    if design_mode {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(16.0)
                    .strong()
                    .color(palette::TEXT()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::primary_button(ui, icons::save(), "Save to Design", true).clicked() {
                    actions.push(Action::SaveErdTable);
                }
                ui.add_space(6.0);
                if components::button(ui, icons::close(), "Cancel", true).clicked() {
                    actions.push(Action::CancelSchema);
                }
            });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);
    } else if table_section.is_some() {
        embedded_table_editor_header(ui, editor, table_section);
    } else {
        object_editor_header(ui, actions, &title);
    }

    // Live EditTable keeps the database object's name fixed; portable designs allow renames.
    if table_section.is_none() {
        ui.horizontal(|ui| {
            ui.label("Table name:");
            components::text_input_enabled(
                ui,
                editor.mode != SchemaEditorMode::Edit,
                &mut editor.table_name,
                "my_table",
                200.0,
            );
            if !editor.schema_name.is_empty() || editor.mode != SchemaEditorMode::Edit {
                ui.label("Schema:");
                components::text_input(ui, &mut editor.schema_name, "public", 120.0);
            }
        });
        ui.add_space(10.0);
    }

    if table_section.is_none() {
        // Create-table and ER-design flows keep their local selector. Existing table tabs expose
        // Structure and Indexes in the persistent result bar instead of nesting another switch.
        let (tabs, tab_labels, tab_width): (&[SchemaTab], &[(_, _)], f32) =
            if editor.db_kind.is_cql() {
                (
                    &[SchemaTab::Columns, SchemaTab::Indexes],
                    &[(icons::column(), "Columns"), (icons::index(), "Indexes")],
                    240.0,
                )
            } else {
                (
                    &[
                        SchemaTab::Columns,
                        SchemaTab::Indexes,
                        SchemaTab::ForeignKeys,
                    ],
                    &[
                        (icons::column(), "Columns"),
                        (icons::index(), "Indexes"),
                        (icons::key(), "Foreign Keys"),
                    ],
                    340.0,
                )
            };
        let selected = tabs
            .iter()
            .position(|tab| *tab == editor.active_tab)
            .unwrap_or(0);
        let choice = components::segmented_sized(ui, tab_labels, selected, tab_width, false);
        editor.active_tab = tabs[choice];
        ui.add_space(8.0);
    }

    if table_section.is_some() {
        schema_grid_keyboard(ui, editor, table_section);
    }

    let viewport_width = ui.available_width();
    let schema_scroll = if table_section.is_some() {
        egui::ScrollArea::both()
    } else {
        egui::ScrollArea::vertical()
    };

    schema_scroll
        .id_salt("schema_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // TableBuilder sizes columns against its parent. Give live Structure/Indexes grids
            // their natural width so a narrow panel overflows into this horizontal ScrollArea
            // instead of clipping the columns on the right.
            let grid_min_width: f32 = match table_section {
                Some(TabView::Structure) => 1_164.0,
                Some(TabView::Indexes) => 634.0,
                _ => 0.0,
            };
            if grid_min_width > 0.0 {
                ui.set_min_width(grid_min_width.max(viewport_width));
            }

            match editor.active_tab {
                SchemaTab::Columns => {
                    if table_section == Some(TabView::Structure) {
                        schema_structure_grid(
                            ui,
                            actions,
                            &mut editor.columns,
                            &editor.fks,
                            editor.db_kind,
                            &editor.column_filter,
                            SchemaStructureGridState {
                                editing_type_row: &mut editor.editing_type_row,
                                selection: &mut editor.grid_selection,
                                focus_selected_cell: &mut editor.focus_selected_cell,
                            },
                        );
                    } else {
                        schema_columns_tab(ui, &mut editor.columns, editor.mode, editor.db_kind);
                    }
                }
                SchemaTab::Indexes if table_section == Some(TabView::Indexes) => {
                    schema_indexes_grid(
                        ui,
                        &mut editor.indexes,
                        &mut editor.grid_selection,
                        &mut editor.focus_selected_cell,
                    )
                }
                SchemaTab::Indexes => schema_indexes_tab(ui, &mut editor.indexes),
                SchemaTab::ForeignKeys if table_section.is_some() => {}
                SchemaTab::ForeignKeys => schema_fk_tab(ui, &mut editor.fks),
            }
        });
}

/// Render the view create/edit form: name/schema, an optional materialized toggle (Postgres),
/// and the defining `SELECT` as a multi-line editor.
fn view_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::ViewEditor,
) {
    use crate::schema::ObjectMode;
    let title = match editor.mode {
        ObjectMode::Create => "Create View".to_string(),
        ObjectMode::Edit => format!("Edit View — {}", editor.name),
    };
    object_editor_header(ui, actions, &title);

    ui.horizontal(|ui| {
        ui.label("View name:");
        components::text_input(ui, &mut editor.name, "my_view", 200.0);
        if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
            ui.label("Schema:");
            components::text_input(ui, &mut editor.schema_name, "public", 120.0);
        }
        // Materialized views are Postgres-only.
        if editor.db_kind == dbcore::DbKind::Postgres {
            components::accent_checkbox(ui, true, &mut editor.materialized, Some("Materialized"));
        }
    });
    ui.add_space(6.0);

    ui.label(
        egui::RichText::new("Defining query (the SELECT after AS)")
            .color(palette::TEXT_WEAK())
            .size(12.0),
    );
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .id_salt("view_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut editor.select_body)
                    .code_editor()
                    .desired_rows(16)
                    .desired_width(f32::INFINITY)
                    .hint_text("SELECT ..."),
            );
        });
}

/// A placeholder body for a trigger, tailored to the dialect's procedural style.
fn trigger_body_hint(kind: dbcore::DbKind) -> &'static str {
    match kind {
        dbcore::DbKind::Postgres => "BEGIN\n  -- NEW / OLD available\n  RETURN NEW;\nEND;",
        dbcore::DbKind::Sqlite => "INSERT INTO audit(msg) VALUES ('changed');",
        dbcore::DbKind::SqlServer => {
            "BEGIN\n  SET NOCOUNT ON;\n  -- inserted / deleted tables\nEND"
        }
        _ => "SET NEW.col = ...;  -- or a BEGIN ... END block",
    }
}

/// Render the dialect-adaptive trigger create/edit form. Controls a dialect can't express are
/// hidden (e.g. row/statement level off Postgres, WHEN off MySQL/SQL Server), so the same
/// editor serves all four backends.
fn trigger_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::TriggerEditor,
) {
    use crate::schema::ObjectMode;
    use dbcore::{DbKind, TriggerEvent, TriggerLevel, TriggerTiming};

    let title = match editor.mode {
        ObjectMode::Create => "Create Trigger".to_string(),
        ObjectMode::Edit => format!("Edit Trigger — {}", editor.name),
    };
    object_editor_header(ui, actions, &title);
    let kind = editor.db_kind;

    egui::ScrollArea::vertical()
        .id_salt("trigger_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name:");
                components::text_input(ui, &mut editor.name, "my_trigger", 180.0);
                if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
                    ui.label("Schema:");
                    components::text_input(ui, &mut editor.schema_name, "public", 110.0);
                }
            });
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Table:");
                let selected = if editor.table.is_empty() {
                    "select…".to_string()
                } else {
                    editor.table.clone()
                };
                let current = editor
                    .tables
                    .iter()
                    .position(|table| table == &editor.table);
                if let Some(Some(table)) = components::searchable_combo_box(
                    ui,
                    "trig_table",
                    &selected,
                    180.0,
                    &editor.tables,
                    current,
                    None,
                ) {
                    editor.table.clone_from(&editor.tables[table]);
                }
            });
            ui.add_space(6.0);

            // Timing — the available options depend on the dialect.
            let timings: &[TriggerTiming] = match kind {
                DbKind::MySql | DbKind::MariaDb => &[TriggerTiming::Before, TriggerTiming::After],
                DbKind::SqlServer => &[TriggerTiming::After, TriggerTiming::InsteadOf],
                _ => TriggerTiming::ALL,
            };
            ui.horizontal(|ui| {
                ui.label("Timing:");
                for &t in timings {
                    ui.selectable_value(&mut editor.timing, t, t.label());
                }
            });
            ui.add_space(4.0);

            // Events — MySQL/SQLite fire on one (radio); Postgres/SQL Server allow several.
            let single = matches!(kind, DbKind::MySql | DbKind::MariaDb | DbKind::Sqlite);
            ui.horizontal(|ui| {
                ui.label("Events:");
                for &e in TriggerEvent::ALL {
                    let mut on = editor.has_event(e);
                    if single {
                        if ui.selectable_label(on, e.label()).clicked() {
                            editor.events = vec![e];
                        }
                    } else if components::accent_checkbox(ui, true, &mut on, Some(e.label()))
                        .changed()
                    {
                        editor.set_event(e, on);
                    }
                }
            });
            if single {
                ui.label(
                    egui::RichText::new("This dialect fires on a single event.")
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
            }
            ui.add_space(4.0);

            // Row vs statement — only Postgres lets you choose; the others are fixed.
            if kind == DbKind::Postgres {
                ui.horizontal(|ui| {
                    ui.label("For each:");
                    ui.selectable_value(&mut editor.level, TriggerLevel::Row, "ROW");
                    ui.selectable_value(&mut editor.level, TriggerLevel::Statement, "STATEMENT");
                });
                ui.add_space(4.0);
            }

            // WHEN guard — Postgres & SQLite only.
            if matches!(kind, DbKind::Postgres | DbKind::Sqlite) {
                ui.horizontal(|ui| {
                    ui.label("When:");
                    components::text_input(
                        ui,
                        &mut editor.when_condition,
                        "optional: NEW.col > 0",
                        320.0,
                    );
                });
                ui.add_space(6.0);
            }

            // Body — Postgres can execute an existing function instead of an inline body.
            if kind == DbKind::Postgres {
                ui.horizontal(|ui| {
                    components::accent_checkbox(
                        ui,
                        true,
                        &mut editor.pg_existing_function,
                        Some("Execute existing function"),
                    );
                });
                if editor.pg_existing_function {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Function to execute")
                            .color(palette::TEXT_WEAK())
                            .size(12.0),
                    );
                    components::text_input(ui, &mut editor.body, "my_trigger_fn", 280.0);
                    return;
                }
                ui.label(
                    egui::RichText::new(
                        "PL/pgSQL function body (a RETURNS trigger function is generated)",
                    )
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
                );
            } else {
                ui.label(
                    egui::RichText::new("Trigger body")
                        .color(palette::TEXT_WEAK())
                        .size(12.0),
                );
            }
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut editor.body)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text(trigger_body_hint(kind)),
            );
        });
}

/// A placeholder routine body, tailored to the dialect and routine kind.
fn routine_body_hint(kind: dbcore::DbKind, is_function: bool) -> &'static str {
    use dbcore::DbKind;
    match (kind, is_function) {
        (DbKind::Postgres, _) => "BEGIN\n  RETURN ...;\nEND;",
        (DbKind::SqlServer, true) => "BEGIN\n  RETURN ...;\nEND",
        (DbKind::SqlServer, false) => "BEGIN\n  SELECT ...;\nEND",
        (_, true) => "RETURN ...;  -- or a BEGIN ... END block",
        (_, false) => "BEGIN\n  ...\nEND",
    }
}

/// Render the function/procedure create/edit form: a parameter grid plus return type,
/// language (Postgres), and body. Dialect-adaptive — the mode column is hidden for MySQL
/// functions, the language picker shows only on Postgres.
fn routine_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::RoutineEditor,
) {
    use crate::schema::{ObjectMode, ParamDraft};
    use dbcore::{DbKind, ParamMode, RoutineKind};

    let title = match editor.mode {
        ObjectMode::Create if editor.kind == RoutineKind::Function => "Create Function".to_string(),
        ObjectMode::Create => "Create Procedure".to_string(),
        ObjectMode::Edit => format!("Edit {} — {}", editor.kind.label(), editor.name),
    };
    object_editor_header(ui, actions, &title);
    let kind = editor.db_kind;
    let is_fn = editor.kind == RoutineKind::Function;

    egui::ScrollArea::vertical()
        .id_salt("routine_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Function/Procedure switch (create mode only; the kind is fixed once it exists).
            if editor.mode == ObjectMode::Create {
                ui.horizontal(|ui| {
                    ui.label("Kind:");
                    ui.selectable_value(&mut editor.kind, RoutineKind::Function, "Function");
                    ui.selectable_value(&mut editor.kind, RoutineKind::Procedure, "Procedure");
                });
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                ui.label("Name:");
                components::text_input(ui, &mut editor.name, "my_routine", 180.0);
                if !editor.schema_name.is_empty() || editor.mode == ObjectMode::Create {
                    ui.label("Schema:");
                    components::text_input(ui, &mut editor.schema_name, "public", 110.0);
                }
            });
            ui.add_space(4.0);

            // Return type (functions) and language (Postgres).
            if is_fn || kind == DbKind::Postgres {
                ui.horizontal(|ui| {
                    if is_fn {
                        ui.label("Returns:");
                        components::text_input(ui, &mut editor.return_type, "integer", 150.0);
                    }
                    if kind == DbKind::Postgres {
                        ui.label("Language:");
                        egui::ComboBox::from_id_salt("routine_lang")
                            .selected_text(editor.language.clone())
                            .show_ui(ui, |ui| {
                                for l in ["plpgsql", "sql"] {
                                    ui.selectable_value(&mut editor.language, l.to_string(), l);
                                }
                            });
                    }
                });
                ui.add_space(6.0);
            }

            // Parameters grid.
            ui.label(
                egui::RichText::new("Parameters")
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
            );
            ui.add_space(2.0);
            // MySQL/MariaDB functions take no parameter mode.
            let show_mode = !(matches!(kind, DbKind::MySql | DbKind::MariaDb) && is_fn);
            let mut remove: Option<usize> = None;
            for (i, p) in editor.params.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    components::text_input(ui, &mut p.name, "name", 110.0);
                    components::text_input(ui, &mut p.data_type, "type", 120.0);
                    if show_mode {
                        egui::ComboBox::from_id_salt(("pmode", i))
                            .selected_text(p.mode.label())
                            .width(82.0)
                            .show_ui(ui, |ui| {
                                for m in ParamMode::ALL {
                                    ui.selectable_value(&mut p.mode, *m, m.label());
                                }
                            });
                    }
                    components::text_input(ui, &mut p.default, "default", 100.0);
                    if components::button(ui, icons::trash(), "", true).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                editor.params.remove(i);
            }
            if components::button(ui, icons::plus(), "Add parameter", true).clicked() {
                editor.params.push(ParamDraft::new_empty());
            }
            ui.add_space(6.0);

            ui.label(
                egui::RichText::new("Body")
                    .color(palette::TEXT_WEAK())
                    .size(12.0),
            );
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut editor.body)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text(routine_body_hint(kind, is_fn)),
            );
        });
}

impl DbGuiApp {
    // ─── Schema Editor dialog ─────────────────────────────────────────────────

    /// The schema editor (Create/Edit Table), rendered inline in the central panel —
    /// it takes the grid's place like the Data/Structure views rather than floating as
    /// a dialog. Applying DDL on a production connection opens Guardian review.
    pub(super) fn schema_editor_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let editing_existing_table = matches!(
            self.tabs[idx].schema_editor.as_ref(),
            Some(crate::schema::ObjectEditor::Table(editor))
                if editor.mode == crate::schema::SchemaEditorMode::Edit
        );
        // An existing table has one schema-editing surface: the dense Structure/Indexes grids.
        // Normalize stale tabs that still carry the former Data-form state so the legacy
        // Columns/Indexes/Foreign Keys editor can never reappear for a live table.
        if editing_existing_table
            && !matches!(self.tabs[idx].view, TabView::Structure | TabView::Indexes)
        {
            self.tabs[idx].view = TabView::Structure;
        }
        let table_section = editing_existing_table.then_some(self.tabs[idx].view);
        // The designer owns the whole tab; a slim margin keeps the form off the panel edge.
        let rect = ui
            .available_rect_before_wrap()
            .shrink2(egui::vec2(10.0, 2.0));
        ui.push_id(("schema_editor", tab_id), |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                match self.tabs[idx].schema_editor.as_mut() {
                    Some(crate::schema::ObjectEditor::Table(editor)) => {
                        if let Some(section) = table_section {
                            editor.active_tab = if section == TabView::Indexes {
                                crate::schema::SchemaTab::Indexes
                            } else {
                                crate::schema::SchemaTab::Columns
                            };
                        }
                        table_editor_view(ui, actions, editor, table_section)
                    }
                    Some(crate::schema::ObjectEditor::View(editor)) => {
                        view_editor_view(ui, actions, editor)
                    }
                    Some(crate::schema::ObjectEditor::Trigger(editor)) => {
                        trigger_editor_view(ui, actions, editor)
                    }
                    Some(crate::schema::ObjectEditor::Routine(editor)) => {
                        routine_editor_view(ui, actions, editor)
                    }
                    None => {}
                }
            });
        });
    }
}
