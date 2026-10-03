//! Schema editor rendering and interaction.

use super::schema_grid::schema_columns_tab;
use super::schema_grid::schema_fk_tab;
use super::schema_grid::schema_grid_keyboard;
use super::schema_grid::schema_indexes_grid;
use super::schema_grid::schema_indexes_tab;
use super::schema_grid::schema_new_table_grid;
use super::schema_grid::schema_structure_grid;
use super::schema_grid::SchemaStructureGridState;
use crate::app::{Action, DbGuiApp, TabView};
use crate::components;
use crate::icons;
use crate::style::{self, palette};

/// The header shared by the object editors: an optional title over a rule. There are no
/// Apply / Cancel buttons — Cmd/Ctrl+S applies and Esc leaves (see `layout.rs`).
fn object_editor_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    if !title.is_empty() {
        ui.label(
            egui::RichText::new(title)
                .size(16.0)
                .strong()
                .color(palette::TEXT()),
        );
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);
    }
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

/// The name field of a new object. A brand-new editor opens with this field focused and its
/// suggested name selected: type to replace it, or just carry on. `select_on_open` is cleared
/// once that has happened, so a later edit of the text is never re-selected.
fn object_name_input(
    ui: &mut egui::Ui,
    id_salt: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    select_on_open: &mut bool,
) {
    let id = ui.id().with(id_salt);
    if std::mem::take(select_on_open) {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            )));
        state.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
    }
    ui.add_sized(
        egui::vec2(width, style::CONTROL_H),
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .vertical_align(egui::Align::Center)
            .margin(egui::Margin::symmetric(6, 0)),
    );
}

/// Double-clicking the blank space under a grid's last row. The zone starts where the content
/// ends, so it can never sit over a row or a cell and steal their clicks.
fn empty_space_double_clicked(ui: &mut egui::Ui, id: &'static str) -> bool {
    let content = ui.min_rect();
    let visible = ui.clip_rect();
    let zone = egui::Rect::from_min_max(
        egui::pos2(content.left(), content.bottom()),
        egui::pos2(visible.right(), visible.bottom()),
    );
    if zone.height() < 4.0 || zone.width() < 4.0 {
        return false;
    }
    let response = ui.interact(zone, ui.id().with(id), egui::Sense::click());
    response.double_clicked()
}

/// Whether a new object on this backend is placed in a named schema. SQLite and MySQL/MariaDB
/// have none to type, so offering the field (with a `public` hint) would only mislead.
fn has_schemas(kind: dbcore::DbKind) -> bool {
    matches!(
        kind,
        dbcore::DbKind::Postgres | dbcore::DbKind::SqlServer | dbcore::DbKind::DuckDb
    )
}

/// New Table: one compact bar (name, schema, search, Cancel / Apply), a Columns / Indexes /
/// Foreign Keys switch, and the dense grid for whichever is showing — the same surface as an
/// existing table's Structure and Indexes views, so nothing here is a dialog-style form.
fn new_table_grid_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::SchemaEditor,
) {
    use crate::schema::SchemaTab;

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Name")
                .size(11.0)
                .strong()
                .color(palette::TEXT_WEAK()),
        );
        object_name_input(
            ui,
            "new_table_name",
            &mut editor.table_name,
            "table_name",
            220.0,
            &mut editor.select_name_on_open,
        );
        if !editor.schema_name.is_empty() || has_schemas(editor.db_kind) {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Schema")
                    .size(11.0)
                    .strong()
                    .color(palette::TEXT_WEAK()),
            );
            components::text_input(ui, &mut editor.schema_name, "public", 110.0);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if editor.active_tab == SchemaTab::Columns {
                let width = 240.0_f32.min(ui.available_width());
                components::icon_text_input(
                    ui,
                    &mut editor.column_filter,
                    "Search for column…",
                    icons::search(),
                    width,
                );
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let tabs = [
            SchemaTab::Columns,
            SchemaTab::Indexes,
            SchemaTab::ForeignKeys,
        ];
        let selected = tabs
            .iter()
            .position(|tab| *tab == editor.active_tab)
            .unwrap_or(0);
        let choice = components::segmented_sized(
            ui,
            &[
                (icons::column(), "Columns"),
                (icons::index(), "Indexes"),
                (icons::key(), "Foreign Keys"),
            ],
            selected,
            340.0,
            false,
        );
        editor.active_tab = tabs[choice];
        ui.add_space(6.0);
        match editor.active_tab {
            SchemaTab::Columns => {
                if components::button(ui, icons::plus(), "Column", true).clicked() {
                    actions.push(Action::AddSchemaColumn);
                }
            }
            SchemaTab::Indexes => {
                if components::button(ui, icons::plus(), "Index", true).clicked() {
                    actions.push(Action::AddSchemaIndex);
                }
            }
            SchemaTab::ForeignKeys => {}
        }
    });
    ui.add_space(6.0);
    ui.separator();

    // Arrow keys / Delete / Enter act on whichever grid is showing.
    schema_grid_keyboard(
        ui,
        editor,
        Some(if editor.active_tab == SchemaTab::Indexes {
            TabView::Indexes
        } else {
            TabView::Structure
        }),
    );

    let viewport_width = ui.available_width();
    egui::ScrollArea::both()
        .id_salt("new_table_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(760.0_f32.max(viewport_width));
            match editor.active_tab {
                SchemaTab::Columns => {
                    schema_new_table_grid(
                        ui,
                        &mut editor.columns,
                        editor.db_kind,
                        &editor.column_filter,
                        SchemaStructureGridState {
                            editing_type_row: &mut editor.editing_type_row,
                            selection: &mut editor.grid_selection,
                            focus_selected_cell: &mut editor.focus_selected_cell,
                        },
                    );
                    if empty_space_double_clicked(ui, "new_column_zone") {
                        actions.push(Action::AddSchemaColumn);
                    }
                }
                SchemaTab::Indexes => {
                    schema_indexes_grid(
                        ui,
                        &mut editor.indexes,
                        &mut editor.grid_selection,
                        &mut editor.focus_selected_cell,
                    );
                    if empty_space_double_clicked(ui, "new_index_zone") {
                        actions.push(Action::AddSchemaIndex);
                    }
                }
                SchemaTab::ForeignKeys => schema_fk_tab(ui, &mut editor.fks),
            }
        });
}

/// Render the table create/edit form (columns, indexes, foreign keys) into the central panel.
fn table_editor_view(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    editor: &mut crate::schema::SchemaEditor,
    table_section: Option<TabView>,
) {
    use crate::schema::{SchemaEditorMode, SchemaTab};
    // A table that doesn't exist yet gets the same dense grid as the Structure view of one
    // that does. CQL tables keep the legacy form, whose partition/clustering model the grid
    // doesn't express.
    if table_section.is_none() && editor.mode == SchemaEditorMode::New && !editor.db_kind.is_cql() {
        new_table_grid_view(ui, actions, editor);
        return;
    }
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
        object_editor_header(ui, &title);
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

/// View create/edit: one compact bar (name, schema, materialized, Cancel / Apply) over a
/// full-height SQL editor with the app's own highlighting and font, so writing a view feels
/// like writing a query rather than filling in a dialog.
///
/// With `own_editor` false only the bar is drawn: the caller supplies the SQL editor below it.
fn view_editor_view(
    ui: &mut egui::Ui,
    editor: &mut crate::schema::ViewEditor,
    font_size: f32,
    own_editor: bool,
) {
    use crate::schema::ObjectMode;
    let creating = editor.mode == ObjectMode::Create;

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
        ui.label(
            egui::RichText::new("Name")
                .size(11.0)
                .strong()
                .color(palette::TEXT_WEAK()),
        );
        object_name_input(
            ui,
            "view_editor_name",
            &mut editor.name,
            "view_name",
            240.0,
            &mut editor.select_name_on_open,
        );
        if !editor.schema_name.is_empty() || (creating && has_schemas(editor.db_kind)) {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Schema")
                    .size(11.0)
                    .strong()
                    .color(palette::TEXT_WEAK()),
            );
            components::text_input(ui, &mut editor.schema_name, "public", 110.0);
        }
        // Materialized views are Postgres-only.
        if editor.db_kind == dbcore::DbKind::Postgres {
            ui.add_space(8.0);
            components::accent_checkbox(ui, true, &mut editor.materialized, Some("Materialized"));
        }
    });
    ui.add_space(6.0);
    ui.separator();
    if !own_editor {
        return;
    }

    // The defining query: line numbers beside a frameless, highlighted editor that fills the
    // rest of the tab.
    let mut font = egui::TextStyle::Monospace.resolve(ui.style());
    font.size = font_size;
    let row_height = ui.fonts_mut(|f| f.row_height(&font));
    let rows = ((ui.available_height() / row_height).floor() as usize).max(6);
    let lines = editor.select_body.split('\n').count().max(1);
    let digits = lines.to_string().len().max(2);
    let gutter_width = digits as f32 * font_size * 0.62 + 14.0;
    let numbers = (1..=lines)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    egui::ScrollArea::vertical()
        .id_salt("view_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(gutter_width, row_height * lines as f32),
                    egui::Layout::top_down(egui::Align::Max),
                    |ui| {
                        ui.add_space(0.0);
                        ui.label(
                            egui::RichText::new(numbers)
                                .font(font.clone())
                                .color(palette::TEXT_FAINT()),
                        );
                    },
                );
                ui.add_space(10.0);
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job =
                        crate::highlight::highlight_sql_folded(buf.as_str(), font.clone(), &[]);
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };
                ui.add(
                    egui::TextEdit::multiline(&mut editor.select_body)
                        .code_editor()
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_rows(rows)
                        .desired_width(f32::INFINITY)
                        .hint_text("SELECT …")
                        .layouter(&mut layouter),
                );
            });
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

/// A small label for the compact object-editor bars (Name, Table, Timing…). It takes the full
/// height of the controls beside it, so it sits on their centre line rather than above it.
fn bar_caption(ui: &mut egui::Ui, text: &str) {
    let color = palette::TEXT_WEAK();
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(11.0),
        color,
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 4.0, style::CONTROL_H),
        egui::Sense::hover(),
    );
    // Painted on the row's centre line: a label laid out by `add_sized` hugs the top instead.
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// `INSTEAD OF` → `Instead of`: the SQL keywords the dialects use, shown the way the rest of
/// the interface writes its words.
fn sentence_case(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A segmented choice among `labels`, returning the clicked index if it changed. Every segment
/// is as wide as the longest label needs, so the text never touches an edge.
fn bar_choice(ui: &mut egui::Ui, labels: &[String], selected: usize) -> Option<usize> {
    let items: Vec<_> = labels
        .iter()
        .map(|label| (icons::play(), label.as_str()))
        .collect();
    let widest = labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    label.clone(),
                    egui::FontId::proportional(12.0),
                    palette::TEXT(),
                )
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    let width = (widest + 30.0) * labels.len() as f32;
    let choice = components::segmented_sized(ui, &items, selected, width, false);
    (choice != selected).then_some(choice)
}

/// Trigger create/edit: the same compact bar as a view — name, table, timing, event, and the
/// dialect's extras — over the body. Controls a dialect can't express are hidden (row/statement
/// level off Postgres, WHEN off MySQL/SQL Server), so one form serves every driver.
///
/// Returns whether the caller should show the body editor (Postgres' "execute an existing
/// function" takes just a name, which the bar already holds). With `own_body` the plain text
/// box is drawn here, for editing an existing trigger; a new one is written in the SQL editor.
fn trigger_editor_view(
    ui: &mut egui::Ui,
    editor: &mut crate::schema::TriggerEditor,
    own_body: bool,
) -> bool {
    use crate::schema::ObjectMode;
    use dbcore::{DbKind, TriggerEvent, TriggerLevel, TriggerTiming};

    let kind = editor.db_kind;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
        bar_caption(ui, "Name");
        components::text_input(ui, &mut editor.name, "my_trigger", 200.0);
        if !editor.schema_name.is_empty()
            || (editor.mode == ObjectMode::Create && has_schemas(editor.db_kind))
        {
            ui.add_space(8.0);
            bar_caption(ui, "Schema");
            components::text_input(ui, &mut editor.schema_name, "public", 110.0);
        }
        ui.add_space(8.0);
        bar_caption(ui, "Table");
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
        // Row vs statement — only Postgres lets you choose; the others are fixed.
        if kind == DbKind::Postgres {
            ui.add_space(8.0);
            bar_caption(ui, "For each");
            let selected = usize::from(editor.level == TriggerLevel::Statement);
            if let Some(choice) = bar_choice(ui, &["Row".into(), "Statement".into()], selected) {
                editor.level = if choice == 0 {
                    TriggerLevel::Row
                } else {
                    TriggerLevel::Statement
                };
            }
        }
    });
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
        // Timing — the available options depend on the dialect.
        let timings: &[TriggerTiming] = match kind {
            DbKind::MySql | DbKind::MariaDb => &[TriggerTiming::Before, TriggerTiming::After],
            DbKind::SqlServer => &[TriggerTiming::After, TriggerTiming::InsteadOf],
            _ => TriggerTiming::ALL,
        };
        bar_caption(ui, "Timing");
        let labels: Vec<String> = timings.iter().map(|t| sentence_case(t.label())).collect();
        let selected = timings.iter().position(|t| *t == editor.timing).unwrap_or(0);
        if let Some(choice) = bar_choice(ui, &labels, selected) {
            editor.timing = timings[choice];
        }

        // Events — MySQL/SQLite fire on one (segmented); Postgres/SQL Server allow several.
        ui.add_space(8.0);
        bar_caption(ui, "Event");
        let single = matches!(kind, DbKind::MySql | DbKind::MariaDb | DbKind::Sqlite);
        if single {
            let labels: Vec<String> = TriggerEvent::ALL
                .iter()
                .map(|e| sentence_case(e.label()))
                .collect();
            let selected = TriggerEvent::ALL
                .iter()
                .position(|e| editor.has_event(*e))
                .unwrap_or(0);
            if let Some(choice) = bar_choice(ui, &labels, selected) {
                editor.events = vec![TriggerEvent::ALL[choice]];
            }
        } else {
            for &event in TriggerEvent::ALL {
                let mut on = editor.has_event(event);
                if components::accent_checkbox(ui, true, &mut on, Some(&sentence_case(event.label())))
                    .changed()
                {
                    editor.set_event(event, on);
                }
            }
        }

        // WHEN guard — Postgres & SQLite only.
        if matches!(kind, DbKind::Postgres | DbKind::Sqlite) {
            ui.add_space(8.0);
            bar_caption(ui, "When");
            let width = ui.available_width().clamp(120.0, 260.0);
            components::text_input(ui, &mut editor.when_condition, "NEW.col > 0", width);
        }
    });

    // Postgres can execute an existing function instead of an inline body.
    let mut wants_body = true;
    if kind == DbKind::Postgres {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
            let before = editor.pg_existing_function;
            components::accent_checkbox(
                ui,
                true,
                &mut editor.pg_existing_function,
                Some("Execute existing function"),
            );
            if before != editor.pg_existing_function {
                // The body holds an inline body or a function name, never both.
                editor.body.clear();
            }
            if editor.pg_existing_function {
                ui.add_space(8.0);
                components::text_input(ui, &mut editor.body, "my_trigger_fn", 240.0);
            }
        });
        wants_body = !editor.pg_existing_function;
    }
    ui.add_space(6.0);
    ui.separator();

    if own_body && wants_body {
        ui.add_space(6.0);
        bar_caption(ui, "Body");
        ui.add_space(2.0);
        ui.add(
            egui::TextEdit::multiline(&mut editor.body)
                .code_editor()
                .desired_rows(12)
                .desired_width(f32::INFINITY)
                .hint_text(trigger_body_hint(kind)),
        );
    }
    wants_body
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

/// Function/procedure create/edit: the same compact bar as a view or trigger — kind, name,
/// return type, language — then a short parameter list, over the body. Dialect-adaptive: the
/// mode column is hidden for MySQL functions, the language picker shows only on Postgres.
///
/// With `own_body` the plain body box is drawn here, for editing an existing routine; a new
/// one is written in the SQL editor the caller draws below.
fn routine_editor_view(
    ui: &mut egui::Ui,
    editor: &mut crate::schema::RoutineEditor,
    own_body: bool,
) {
    use crate::schema::{ObjectMode, ParamDraft};
    use dbcore::{DbKind, ParamMode, RoutineKind};

    let kind = editor.db_kind;
    let creating = editor.mode == ObjectMode::Create;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
        // The kind is fixed once the routine exists.
        if creating {
            let selected = usize::from(editor.kind == RoutineKind::Procedure);
            if let Some(choice) = bar_choice(ui, &["Function".into(), "Procedure".into()], selected) {
                editor.kind = if choice == 0 {
                    RoutineKind::Function
                } else {
                    RoutineKind::Procedure
                };
            }
            ui.add_space(8.0);
        }
        bar_caption(ui, "Name");
        components::text_input(ui, &mut editor.name, "my_routine", 200.0);
        if !editor.schema_name.is_empty() || creating {
            ui.add_space(8.0);
            bar_caption(ui, "Schema");
            components::text_input(ui, &mut editor.schema_name, "public", 110.0);
        }
        // Return type (functions) and language (Postgres).
        if editor.kind == RoutineKind::Function {
            ui.add_space(8.0);
            bar_caption(ui, "Returns");
            components::text_input(ui, &mut editor.return_type, "integer", 140.0);
        }
        if kind == DbKind::Postgres {
            ui.add_space(8.0);
            bar_caption(ui, "Language");
            egui::ComboBox::from_id_salt("routine_lang")
                .selected_text(editor.language.clone())
                .icon(components::combo_chevron_icon)
                .show_ui(ui, |ui| {
                    for l in ["plpgsql", "sql"] {
                        ui.selectable_value(&mut editor.language, l.to_string(), l);
                    }
                });
        }
    });
    let is_fn = editor.kind == RoutineKind::Function;

    // Parameters: one compact row each, scrolling past a few so the body keeps the room.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
        bar_caption(ui, "Parameters");
        if components::button(ui, icons::plus(), "Add", true).clicked() {
            editor.params.push(ParamDraft::new_empty());
        }
    });
    if !editor.params.is_empty() {
        // MySQL/MariaDB functions take no parameter mode.
        let show_mode = !(matches!(kind, DbKind::MySql | DbKind::MariaDb) && is_fn);
        let mut remove: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("routine_params_scroll")
            .max_height(132.0)
            .auto_shrink([true, true])
            .show(ui, |ui| {
                for (i, p) in editor.params.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
        // The row is as tall as its controls from the first item, so every caption sits on
        // their centre line instead of riding high until the taller widgets arrive.
        ui.set_min_height(style::CONTROL_H);
                        components::text_input(ui, &mut p.name, "name", 140.0);
                        components::text_input(ui, &mut p.data_type, "type", 140.0);
                        if show_mode {
                            egui::ComboBox::from_id_salt(("pmode", i))
                                .selected_text(p.mode.label())
                                .width(82.0)
                                .icon(components::combo_chevron_icon)
                                .show_ui(ui, |ui| {
                                    for m in ParamMode::ALL {
                                        ui.selectable_value(&mut p.mode, *m, m.label());
                                    }
                                });
                        }
                        components::text_input(ui, &mut p.default, "default", 120.0);
                        if components::button(ui, icons::trash(), "", true).clicked() {
                            remove = Some(i);
                        }
                    });
                }
            });
        if let Some(i) = remove {
            editor.params.remove(i);
        }
    }
    ui.add_space(6.0);
    ui.separator();

    if own_body {
        ui.add_space(6.0);
        bar_caption(ui, "Body");
        ui.add_space(2.0);
        ui.add(
            egui::TextEdit::multiline(&mut editor.body)
                .code_editor()
                .desired_rows(12)
                .desired_width(f32::INFINITY)
                .hint_text(routine_body_hint(kind, is_fn)),
        );
    }
}

impl DbGuiApp {
    // ─── Schema Editor dialog ─────────────────────────────────────────────────

    /// The schema editor (Create/Edit Table), rendered inline in the central panel —
    /// it takes the grid's place like the Data/Structure views rather than floating as
    /// a dialog. Applying DDL on a production connection opens Guardian review.
    pub(super) fn schema_editor_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        self.sync_draft_title(idx);
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
        // A new view, trigger or routine is written in the same SQL editor as a query tab — highlighting,
        // folds, autocomplete, ghost text, hover, diagnostics, find — over the tab's own `sql`,
        // which `open_draft_tab` seeds from the editor. The editor's body follows it.
        if self.draft_uses_sql_editor(idx) {
            let rect = ui
                .available_rect_before_wrap()
                .shrink2(egui::vec2(10.0, 2.0));
            let font_size = self.editor_font_size;
            ui.push_id(("schema_editor", tab_id), |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    use crate::schema::ObjectEditor;
                    let wants_body = match self.tabs[idx].schema_editor.as_mut() {
                        Some(ObjectEditor::View(editor)) => {
                            view_editor_view(ui, editor, font_size, false);
                            true
                        }
                        Some(ObjectEditor::Trigger(editor)) => {
                            trigger_editor_view(ui, editor, false)
                        }
                        Some(ObjectEditor::Routine(editor)) => {
                            routine_editor_view(ui, editor, false);
                            true
                        }
                        _ => true,
                    };
                    if !wants_body {
                        return;
                    }
                    self.sql_editor_body(ui, idx);
                    let tab = &mut self.tabs[idx];
                    let body = match tab.schema_editor.as_mut() {
                        Some(ObjectEditor::View(editor)) => Some(&mut editor.select_body),
                        Some(ObjectEditor::Trigger(editor)) => Some(&mut editor.body),
                        Some(ObjectEditor::Routine(editor)) => Some(&mut editor.body),
                        _ => None,
                    };
                    if let Some(body) = body.filter(|body| **body != tab.sql) {
                        body.clone_from(&tab.sql);
                    }
                });
            });
            return;
        }
        let table_section = editing_existing_table.then_some(self.tabs[idx].view);
        let font_size = self.editor_font_size;
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
                        view_editor_view(ui, editor, font_size, true)
                    }
                    Some(crate::schema::ObjectEditor::Trigger(editor)) => {
                        trigger_editor_view(ui, editor, true);
                    }
                    Some(crate::schema::ObjectEditor::Routine(editor)) => {
                        routine_editor_view(ui, editor, true)
                    }
                    None => {}
                }
            });
        });
    }
}
