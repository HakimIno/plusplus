//! Schema grid rendering and interaction.

use crate::app::{Action, TabView};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

// ─── Schema editor tab helpers ────────────────────────────────────────────────

/// Common column types per database, offered in the Type dropdown. The current value is
/// always shown even if it isn't in this list (e.g. an exotic type on an existing column).
pub(in crate::app) fn db_type_options(kind: dbcore::DbKind) -> &'static [&'static str] {
    use dbcore::DbKind;
    match kind {
        DbKind::Postgres => &[
            "bigint",
            "bigserial",
            "bit",
            "bit varying",
            "bool",
            "boolean",
            "box",
            "bytea",
            "char(1)",
            "char",
            "character varying(255)",
            "cidr",
            "circle",
            "date",
            "decimal",
            "double precision",
            "float4",
            "float8",
            "inet",
            "integer",
            "int2",
            "int4",
            "int8",
            "interval",
            "json",
            "jsonb",
            "line",
            "lseg",
            "macaddr",
            "macaddr8",
            "money",
            "numeric",
            "oid",
            "path",
            "pg_lsn",
            "point",
            "polygon",
            "real",
            "serial",
            "serial2",
            "serial4",
            "serial8",
            "smallint",
            "smallserial",
            "text",
            "time",
            "time with time zone",
            "timestamp",
            "timestamp with time zone",
            "timestamptz",
            "timetz",
            "tsquery",
            "tsvector",
            "uuid",
            "varchar(255)",
            "varchar",
            "xml",
            "integer[]",
            "text[]",
            "uuid[]",
        ],
        DbKind::MySql | DbKind::MariaDb => &[
            "BIGINT",
            "BINARY(255)",
            "BIT",
            "BLOB",
            "BOOLEAN",
            "CHAR(255)",
            "DATE",
            "DATETIME",
            "DECIMAL(10,2)",
            "DOUBLE",
            "ENUM('value1','value2')",
            "FLOAT",
            "GEOMETRY",
            "GEOMETRYCOLLECTION",
            "INT",
            "JSON",
            "LINESTRING",
            "LONGBLOB",
            "LONGTEXT",
            "MEDIUMBLOB",
            "MEDIUMINT",
            "MEDIUMTEXT",
            "MULTILINESTRING",
            "MULTIPOINT",
            "MULTIPOLYGON",
            "NUMERIC(10,2)",
            "POINT",
            "POLYGON",
            "REAL",
            "SET('value1','value2')",
            "SMALLINT",
            "TEXT",
            "TIME",
            "TIMESTAMP",
            "TINYBLOB",
            "TINYINT",
            "TINYTEXT",
            "VARBINARY(255)",
            "VARCHAR(255)",
            "YEAR",
        ],
        DbKind::SqlServer => &[
            "BIGINT",
            "BINARY(50)",
            "BIT",
            "CHAR(255)",
            "DATE",
            "DATETIME",
            "DATETIME2",
            "DATETIMEOFFSET",
            "DECIMAL(18,2)",
            "FLOAT",
            "GEOGRAPHY",
            "GEOMETRY",
            "HIERARCHYID",
            "IMAGE",
            "INT",
            "MONEY",
            "NCHAR(255)",
            "NTEXT",
            "NUMERIC(18,2)",
            "NVARCHAR(255)",
            "NVARCHAR(MAX)",
            "REAL",
            "ROWVERSION",
            "SMALLDATETIME",
            "SMALLINT",
            "SMALLMONEY",
            "SQL_VARIANT",
            "TEXT",
            "TIME",
            "TINYINT",
            "UNIQUEIDENTIFIER",
            "VARBINARY(255)",
            "VARBINARY(MAX)",
            "VARCHAR(255)",
            "VARCHAR(MAX)",
            "XML",
        ],
        DbKind::Sqlite | DbKind::DuckDb => &[
            "BIGINT",
            "BLOB",
            "BOOLEAN",
            "CHAR(255)",
            "CLOB",
            "DATE",
            "DATETIME",
            "DECIMAL(10,2)",
            "DOUBLE",
            "FLOAT",
            "INT",
            "INTEGER",
            "JSON",
            "NONE",
            "NUMERIC",
            "REAL",
            "SMALLINT",
            "TEXT",
            "TIME",
            "TIMESTAMP",
            "VARCHAR(255)",
        ],
        DbKind::Cassandra | DbKind::ScyllaDb => &[
            "text",
            "ascii",
            "int",
            "bigint",
            "smallint",
            "tinyint",
            "varint",
            "decimal",
            "float",
            "double",
            "boolean",
            "uuid",
            "timeuuid",
            "timestamp",
            "date",
            "time",
            "duration",
            "inet",
            "blob",
            "counter",
            "list<text>",
            "set<text>",
            "map<text, text>",
            "frozen<list<text>>",
            "tuple<text, int>",
        ],
    }
}

pub(super) fn schema_grid_keyboard(
    ui: &mut egui::Ui,
    editor: &mut crate::schema::SchemaEditor,
    table_section: Option<TabView>,
) {
    use crate::schema::{SchemaGridSelection, SchemaTab};

    if ui.ctx().memory(|memory| memory.focused().is_some()) || egui::Popup::is_any_open(ui.ctx()) {
        return;
    }

    let default_tab = if table_section == Some(TabView::Indexes) {
        SchemaTab::Indexes
    } else {
        SchemaTab::Columns
    };
    let visible = |tab| match table_section {
        Some(TabView::Indexes) => tab == SchemaTab::Indexes,
        Some(TabView::Structure) => tab == SchemaTab::Columns,
        _ => false,
    };
    if editor
        .grid_selection
        .is_some_and(|selection| !visible(selection.tab))
    {
        editor.grid_selection = None;
    }

    let (up, down, delete, edit, clear) = ui.ctx().input_mut(|input| {
        (
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::F2),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if !(up || down || delete || edit || clear) {
        return;
    }

    if clear {
        editor.grid_selection = None;
        return;
    }

    let row_count = |tab| match tab {
        SchemaTab::Columns => editor.columns.len(),
        SchemaTab::Indexes => editor.indexes.len(),
        SchemaTab::ForeignKeys => 0,
    };
    if up || down {
        let tab = editor
            .grid_selection
            .map_or(default_tab, |selection| selection.tab);
        let len = row_count(tab);
        if len > 0 {
            let row = match editor.grid_selection {
                Some(selection) if up => selection.row.saturating_sub(1),
                Some(selection) if down => (selection.row + 1).min(len - 1),
                _ => 0,
            };
            editor.grid_selection = Some(SchemaGridSelection { tab, row });
        }
    }

    if delete {
        if let Some(selection) = editor.grid_selection {
            let remaining = match selection.tab {
                SchemaTab::Columns if selection.row < editor.columns.len() => {
                    if editor.columns[selection.row].is_existing {
                        editor.columns[selection.row].drop = !editor.columns[selection.row].drop;
                    } else {
                        editor.columns.remove(selection.row);
                    }
                    editor.columns.len()
                }
                SchemaTab::Indexes if selection.row < editor.indexes.len() => {
                    if editor.indexes[selection.row].is_existing {
                        editor.indexes[selection.row].drop = !editor.indexes[selection.row].drop;
                    } else {
                        editor.indexes.remove(selection.row);
                    }
                    editor.indexes.len()
                }
                _ => 0,
            };
            editor.grid_selection = (remaining > 0).then_some(SchemaGridSelection {
                tab: selection.tab,
                row: selection.row.min(remaining.saturating_sub(1)),
            });
        }
    }

    if edit && editor.grid_selection.is_some() {
        editor.focus_selected_cell = true;
    }
    ui.ctx().request_repaint();
}

fn schema_grid_header(ui: &mut egui::Ui, label: &str) {
    components::paint_table_header_cell(ui);
    ui.with_layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(label)
                        .font(egui::TextStyle::Heading.resolve(ui.style()))
                        .color(palette::TEXT()),
                )
                .selectable(false)
                .truncate(),
            );
        },
    );
}

fn schema_grid_row_tint(ui: &mut egui::Ui, drop: bool, is_new: bool) {
    let fill = if drop {
        palette::DANGER().linear_multiply(0.10)
    } else if is_new {
        // Green marks a pending addition, the same way the data grid marks a new row, so
        // what was just added is easy to check before Apply.
        palette::SUCCESS().linear_multiply(0.12)
    } else {
        return;
    };
    ui.painter().rect_filled(
        ui.available_rect_before_wrap(),
        egui::CornerRadius::ZERO,
        fill,
    );
}

fn schema_grid_text(
    ui: &mut egui::Ui,
    enabled: bool,
    value: &mut String,
    hint: &str,
) -> egui::Response {
    let edit_id = ui.id().with("schema_grid_text");
    let focused = enabled && ui.memory(|memory| memory.focused() == Some(edit_id));
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let vertical_padding = ((ui.available_height() - text_height) / 2.0)
        .max(0.0)
        .round() as i8;
    ui.add_enabled_ui(enabled, |ui| {
        let response = ui.add(
            egui::TextEdit::singleline(value)
                .id(edit_id)
                .hint_text(hint)
                .font(egui::TextStyle::Body)
                .vertical_align(egui::Align::Center)
                .frame(
                    egui::Frame::new()
                        .fill(if focused {
                            palette::CODE_BG()
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::symmetric(4, vertical_padding)),
                )
                .desired_width(f32::INFINITY),
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                response.rect.shrink(0.5),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.0_f32, palette::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
        response
    })
    .inner
}

fn schema_grid_bool(
    ui: &mut egui::Ui,
    enabled: bool,
    value: &mut bool,
    hover: &str,
) -> egui::Response {
    ui.with_layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            let color = if *value {
                palette::TEXT()
            } else {
                palette::TEXT_WEAK()
            };
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(
                        egui::RichText::new(if *value { "YES" } else { "NO" })
                            .font(egui::TextStyle::Body.resolve(ui.style()))
                            .color(color),
                    )
                    .frame(false)
                    .min_size(egui::vec2(ui.available_width(), 21.0)),
                )
                .on_hover_text(hover);
            if response.clicked() {
                *value = !*value;
            }
            response
        },
    )
    .inner
}

fn schema_grid_type_editor(
    ui: &mut egui::Ui,
    enabled: bool,
    value: &mut String,
    db_kind: dbcore::DbKind,
    row_index: usize,
    editing_row: &mut Option<usize>,
) -> egui::Response {
    let width = ui.available_width().max(60.0);
    let input_id = ui.id().with(("structure_column_type_edit", row_index));
    let editing = enabled && *editing_row == Some(row_index);
    if editing {
        let text_height = ui.text_style_height(&egui::TextStyle::Body);
        let vertical_padding = ((ui.available_height() - text_height) / 2.0)
            .max(0.0)
            .round() as i8;
        let response = ui.add(
            egui::TextEdit::singleline(value)
                .id(input_id)
                .frame(
                    egui::Frame::new()
                        .fill(palette::CODE_BG())
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::symmetric(4, vertical_padding)),
                )
                .desired_width(f32::INFINITY),
        );
        ui.painter().rect_stroke(
            response.rect.shrink(0.5),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0_f32, palette::ACCENT()),
            egui::StrokeKind::Inside,
        );
        let finish_editing = response.double_clicked()
            || response.lost_focus()
            || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if finish_editing {
            *editing_row = None;
            response.surrender_focus();
            ui.ctx().request_repaint();
        } else if !response.has_focus() {
            response.request_focus();
        }
        return response;
    }

    let (button_rect, button) = ui.allocate_exact_size(
        egui::vec2(width, 21.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    ui.painter().text(
        egui::pos2(button_rect.left() + 4.0, button_rect.center().y),
        egui::Align2::LEFT_CENTER,
        value.as_str(),
        egui::TextStyle::Body.resolve(ui.style()),
        if enabled {
            palette::TEXT()
        } else {
            palette::TEXT_FAINT()
        },
    );
    let chevron_rect = egui::Rect::from_center_size(
        egui::pos2(button.rect.right() - 10.0, button.rect.center().y),
        egui::vec2(11.0, 11.0),
    );
    egui::Image::new(icons::chevron_down())
        .fit_to_exact_size(egui::vec2(11.0, 11.0))
        .tint(palette::TEXT_WEAK())
        .paint_at(ui, chevron_rect);

    let popup_id = ui.id().with(("structure_column_type", row_index));
    if button.double_clicked() && enabled {
        *editing_row = Some(row_index);
        egui::Popup::close_id(ui.ctx(), popup_id);
        ui.ctx().request_repaint();
        return button;
    }

    egui::Popup::from_toggle_button_response(&button)
        .id(popup_id)
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[egui::RectAlign::TOP_START])
        .width(button.rect.width().max(180.0))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width((button.rect.width() - 20.0).max(160.0));
            egui::ScrollArea::vertical()
                .id_salt(("structure_column_types", row_index))
                .max_height(320.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for data_type in db_type_options(db_kind) {
                        let data_type = data_type.to_ascii_lowercase();
                        let selected = value.eq_ignore_ascii_case(&data_type);
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 28.0),
                            egui::Sense::click(),
                        );
                        let fill = if selected {
                            Some(palette::SELECTION())
                        } else if response.hovered() {
                            Some(palette::SURFACE_HOVER())
                        } else {
                            None
                        };
                        if let Some(fill) = fill {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::vec2(2.0, 1.0)),
                                egui::CornerRadius::same(6),
                                fill,
                            );
                        }
                        ui.painter().with_clip_rect(rect.shrink(2.0)).text(
                            egui::pos2(rect.left() + 10.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &data_type,
                            egui::TextStyle::Body.resolve(ui.style()),
                            palette::TEXT(),
                        );
                        if response.clicked() {
                            *value = data_type;
                            ui.close();
                        }
                    }
                });
        });
    button
}

fn schema_grid_select_on_click(
    response: &egui::Response,
    selection: &mut Option<crate::schema::SchemaGridSelection>,
    tab: crate::schema::SchemaTab,
    row: usize,
) {
    if response.clicked() {
        *selection = Some(crate::schema::SchemaGridSelection { tab, row });
    }
}

fn schema_grid_metadata(ui: &mut egui::Ui, value: &str, empty: &str) -> egui::Response {
    let value = value.trim();
    let (text, color) = if value.is_empty() {
        (empty, palette::TEXT_FAINT())
    } else {
        (value, palette::TEXT_WEAK())
    };
    ui.add(
        egui::Label::new(egui::RichText::new(text).color(color))
            .truncate()
            .sense(egui::Sense::click()),
    )
    .on_hover_text(text)
}

fn schema_column_foreign_key(fks: &[crate::schema::FkDraft], column_name: &str) -> String {
    fks.iter()
        .filter(|fk| {
            !fk.drop
                && fk
                    .columns_raw
                    .split(',')
                    .any(|column| column.trim() == column_name)
        })
        .map(|fk| {
            let table = fk
                .ref_schema
                .as_deref()
                .filter(|schema| !schema.is_empty())
                .map_or_else(
                    || fk.ref_table.clone(),
                    |schema| format!("{schema}.{}", fk.ref_table),
                );
            format!("{table}({})", fk.ref_columns_raw)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Editable Structure table for an existing database table. Inputs intentionally have no card
/// chrome: the table grid provides the alignment and a focused cell supplies its own affordance.
pub(super) struct SchemaStructureGridState<'a> {
    pub(super) editing_type_row: &'a mut Option<usize>,
    pub(super) selection: &'a mut Option<crate::schema::SchemaGridSelection>,
    pub(super) focus_selected_cell: &'a mut bool,
}

pub(super) fn schema_structure_grid(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    columns: &mut [crate::schema::ColumnDraft],
    fks: &[crate::schema::FkDraft],
    db_kind: dbcore::DbKind,
    column_filter: &str,
    state: SchemaStructureGridState<'_>,
) {
    use crate::schema::{SchemaGridSelection, SchemaTab};
    use egui_extras::{Column, TableBuilder};

    let SchemaStructureGridState {
        editing_type_row,
        selection,
        focus_selected_cell,
    } = state;
    let row_height = 24.0;
    let query = column_filter.trim().to_lowercase();
    TableBuilder::new(ui)
        .id_salt("editable_structure_columns")
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .auto_shrink([false, true])
        .column(Column::exact(34.0))
        .column(Column::initial(180.0).at_least(110.0).clip(true))
        .column(Column::initial(150.0).at_least(100.0).clip(true))
        .column(Column::initial(90.0).at_least(72.0).clip(true))
        .column(Column::initial(140.0).at_least(90.0).clip(true))
        .column(Column::initial(210.0).at_least(120.0).clip(true))
        .column(Column::initial(220.0).at_least(130.0).clip(true))
        .column(Column::remainder().at_least(140.0).clip(true))
        .header(24.0, |mut header| {
            for label in [
                "#",
                "column_name",
                "data_type",
                "is_nullable",
                "check",
                "column_default",
                "foreign_key",
                "comment",
            ] {
                header.col(|ui| schema_grid_header(ui, label));
            }
        })
        .body(|mut body| {
            for (row_index, column) in columns.iter_mut().enumerate().filter(|(_, column)| {
                query.is_empty() || column.name.to_lowercase().contains(&query)
            }) {
                body.row(row_height, |mut row| {
                    let is_new = !column.is_existing;
                    let selected = selection.is_some_and(|selected| {
                        selected.tab == SchemaTab::Columns && selected.row == row_index
                    });
                    row.set_selected(selected);
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new((row_index + 1).to_string())
                                    .size(11.0)
                                    .monospace()
                                    .color(palette::TEXT_FAINT()),
                            );
                        });
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let response =
                            schema_grid_text(ui, !column.drop, &mut column.name, "column_name");
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                        if selected && *focus_selected_cell && !column.drop {
                            response.request_focus();
                            *focus_selected_cell = false;
                        }
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let type_changed =
                            column.original_type.as_deref().is_some_and(|original| {
                                !original.eq_ignore_ascii_case(column.data_type.trim())
                            });
                        if type_changed && !column.drop {
                            ui.painter().rect_filled(
                                ui.available_rect_before_wrap(),
                                egui::CornerRadius::ZERO,
                                palette::SUCCESS().linear_multiply(0.16),
                            );
                        }
                        let response = schema_grid_type_editor(
                            ui,
                            !column.drop,
                            &mut column.data_type,
                            db_kind,
                            row_index,
                            editing_type_row,
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let response = schema_grid_bool(
                            ui,
                            !column.drop,
                            &mut column.nullable,
                            "Click to change nullability",
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let response = if db_kind == dbcore::DbKind::SqlServer {
                            schema_grid_text(ui, !column.drop, &mut column.check, "NULL")
                        } else {
                            schema_grid_metadata(ui, &column.check, "NULL")
                        };
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let response =
                            schema_grid_text(ui, !column.drop, &mut column.default, "NULL");
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let foreign_key = schema_column_foreign_key(fks, &column.name);
                        let empty = foreign_key.is_empty();
                        let label = if empty { "EMPTY" } else { &foreign_key };
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 21.0),
                            if column.drop {
                                egui::Sense::hover()
                            } else {
                                egui::Sense::click()
                            },
                        );
                        let color = if response.hovered() {
                            palette::TEXT()
                        } else {
                            palette::TEXT_WEAK()
                        };
                        ui.painter().text(
                            egui::pos2(rect.left() + 4.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::TextStyle::Body.resolve(ui.style()),
                            color,
                        );
                        egui::Image::new(icons::chevron_right())
                            .fit_to_exact_size(egui::vec2(13.0, 13.0))
                            .tint(color)
                            .paint_at(
                                ui,
                                egui::Rect::from_center_size(
                                    egui::pos2(rect.right() - 10.0, rect.center().y),
                                    egui::vec2(13.0, 13.0),
                                ),
                            );
                        let response = response.on_hover_text(if empty {
                            format!("Create a foreign key on {}", column.name)
                        } else {
                            format!("Edit the foreign key on {}", column.name)
                        });
                        if response.clicked() && !column.drop {
                            actions.push(Action::OpenForeignKeysForColumn(column.name.clone()));
                        }
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, column.drop, is_new);
                        let response = if db_kind == dbcore::DbKind::SqlServer {
                            schema_grid_text(ui, !column.drop, &mut column.comment, "NULL")
                        } else {
                            schema_grid_metadata(ui, &column.comment, "NULL")
                        };
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    if row.response().clicked() {
                        *selection = Some(SchemaGridSelection {
                            tab: SchemaTab::Columns,
                            row: row_index,
                        });
                    }
                });
            }
        });
}

/// Editable column grid for a table that doesn't exist yet. It speaks the same dense-grid
/// language as the Structure view of an existing table (so creating and altering feel like one
/// tool), plus the two things a new table needs that an existing one doesn't: a primary-key
/// cell and a way to remove a row.
pub(super) fn schema_new_table_grid(
    ui: &mut egui::Ui,
    columns: &mut Vec<crate::schema::ColumnDraft>,
    db_kind: dbcore::DbKind,
    column_filter: &str,
    state: SchemaStructureGridState<'_>,
) {
    use crate::schema::{SchemaGridSelection, SchemaTab};
    use egui_extras::{Column, TableBuilder};

    let SchemaStructureGridState {
        editing_type_row,
        selection,
        focus_selected_cell,
    } = state;
    let query = column_filter.trim().to_lowercase();
    let mut remove: Option<usize> = None;
    TableBuilder::new(ui)
        .id_salt("new_table_columns")
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .auto_shrink([false, true])
        .column(Column::exact(34.0))
        .column(Column::initial(200.0).at_least(110.0).clip(true))
        .column(Column::initial(170.0).at_least(100.0).clip(true))
        .column(Column::initial(110.0).at_least(80.0).clip(true))
        .column(Column::initial(110.0).at_least(80.0).clip(true))
        .column(Column::remainder().at_least(140.0).clip(true))
        .column(Column::exact(34.0))
        .header(24.0, |mut header| {
            for label in [
                "#",
                "column_name",
                "data_type",
                "primary_key",
                "is_nullable",
                "column_default",
                "",
            ] {
                header.col(|ui| schema_grid_header(ui, label));
            }
        })
        .body(|mut body| {
            for (row_index, column) in columns.iter_mut().enumerate().filter(|(_, column)| {
                query.is_empty() || column.name.to_lowercase().contains(&query)
            }) {
                body.row(24.0, |mut row| {
                    let selected = selection.is_some_and(|selected| {
                        selected.tab == SchemaTab::Columns && selected.row == row_index
                    });
                    row.set_selected(selected);
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new((row_index + 1).to_string())
                                    .size(11.0)
                                    .monospace()
                                    .color(palette::TEXT_FAINT()),
                            );
                        });
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        let response = schema_grid_text(ui, true, &mut column.name, "column_name");
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                        if selected && *focus_selected_cell {
                            response.request_focus();
                            *focus_selected_cell = false;
                        }
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        let response = schema_grid_type_editor(
                            ui,
                            true,
                            &mut column.data_type,
                            db_kind,
                            row_index,
                            editing_type_row,
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        let response = schema_grid_bool(
                            ui,
                            true,
                            &mut column.primary_key,
                            "Click to make this column part of the primary key",
                        );
                        // A key column can't be NULL.
                        if response.clicked() && column.primary_key {
                            column.nullable = false;
                        }
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        let response = schema_grid_bool(
                            ui,
                            !column.primary_key,
                            &mut column.nullable,
                            "Click to change nullability",
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        let response = schema_grid_text(ui, true, &mut column.default, "NULL");
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Columns,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, false, !column.is_existing);
                        ui.with_layout(
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                let trash = egui::Image::new(icons::trash())
                                    .fit_to_exact_size(egui::vec2(13.0, 13.0))
                                    .tint(palette::TEXT_FAINT());
                                if ui
                                    .add(egui::Button::image(trash).frame(false))
                                    .on_hover_text("Remove column")
                                    .clicked()
                                {
                                    remove = Some(row_index);
                                }
                            },
                        );
                    });
                    if row.response().clicked() {
                        *selection = Some(SchemaGridSelection {
                            tab: SchemaTab::Columns,
                            row: row_index,
                        });
                    }
                });
            }
        });
    if let Some(row) = remove {
        columns.remove(row);
        *selection = None;
    }
}

pub(super) fn schema_indexes_grid(
    ui: &mut egui::Ui,
    indexes: &mut [crate::schema::IndexDraft],
    selection: &mut Option<crate::schema::SchemaGridSelection>,
    focus_selected_cell: &mut bool,
) {
    use crate::schema::{SchemaGridSelection, SchemaTab};
    use egui_extras::{Column, TableBuilder};

    TableBuilder::new(ui)
        .id_salt("editable_structure_indexes")
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .auto_shrink([false, true])
        .column(Column::exact(34.0))
        .column(Column::initial(300.0).at_least(120.0).clip(true))
        .column(Column::initial(120.0).at_least(80.0).clip(true))
        .column(Column::remainder().at_least(180.0).clip(true))
        .header(24.0, |mut header| {
            for label in ["#", "index_name", "is_unique", "column_name"] {
                header.col(|ui| schema_grid_header(ui, label));
            }
        })
        .body(|mut body| {
            for (row_index, index) in indexes.iter_mut().enumerate() {
                body.row(24.0, |mut row| {
                    let is_new = !index.is_existing;
                    let selected = selection.is_some_and(|selected| {
                        selected.tab == SchemaTab::Indexes && selected.row == row_index
                    });
                    row.set_selected(selected);
                    row.col(|ui| {
                        schema_grid_row_tint(ui, index.drop, is_new);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new((row_index + 1).to_string())
                                    .size(11.0)
                                    .monospace()
                                    .color(palette::TEXT_FAINT()),
                            );
                        });
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, index.drop, is_new);
                        let response =
                            schema_grid_text(ui, !index.drop, &mut index.name, "index_name");
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Indexes,
                            row_index,
                        );
                        if selected && *focus_selected_cell && !index.drop {
                            response.request_focus();
                            *focus_selected_cell = false;
                        }
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, index.drop, is_new);
                        let response = schema_grid_bool(
                            ui,
                            !index.drop,
                            &mut index.unique,
                            "Click to change uniqueness",
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Indexes,
                            row_index,
                        );
                    });
                    row.col(|ui| {
                        schema_grid_row_tint(ui, index.drop, is_new);
                        let response = schema_grid_text(
                            ui,
                            !index.drop,
                            &mut index.columns_raw,
                            "column1, column2",
                        );
                        schema_grid_select_on_click(
                            &response,
                            selection,
                            SchemaTab::Indexes,
                            row_index,
                        );
                    });
                    if row.response().clicked() {
                        *selection = Some(SchemaGridSelection {
                            tab: SchemaTab::Indexes,
                            row: row_index,
                        });
                    }
                });
            }
        });
}

pub(super) fn schema_columns_tab(
    ui: &mut egui::Ui,
    columns: &mut Vec<crate::schema::ColumnDraft>,
    mode: crate::schema::SchemaEditorMode,
    db_kind: dbcore::DbKind,
) {
    use crate::schema::SchemaEditorMode;

    const NAME_W: f32 = 150.0;
    const TYPE_W: f32 = 132.0;
    const NULL_W: f32 = 34.0;
    const PK_W: f32 = 30.0;
    const DEFAULT_W: f32 = 108.0;
    const ACTION_W: f32 = 24.0;

    let mut to_remove: Option<usize> = None;

    ui.spacing_mut().item_spacing.x = 4.0;
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        schema_column_header(ui, "Name", NAME_W);
        schema_column_header(ui, "Type", TYPE_W);
        schema_column_header(ui, "Null", NULL_W);
        schema_column_header(ui, "Key", PK_W);
        schema_column_header(ui, "Default", DEFAULT_W);
        ui.allocate_exact_size(egui::vec2(ACTION_W, 1.0), egui::Sense::hover());
    });
    ui.add_space(3.0);

    for (i, col) in columns.iter_mut().enumerate() {
        let row_color = if col.drop {
            palette::DANGER().linear_multiply(0.12)
        } else if col.is_existing {
            palette::SURFACE().linear_multiply(0.70)
        } else {
            palette::ACCENT().linear_multiply(0.10)
        };

        let frame = egui::Frame::new()
            .fill(row_color)
            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
            .corner_radius(egui::CornerRadius::same(style::radius::SM))
            .inner_margin(egui::Margin::symmetric(4, 3));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!col.drop, |ui| {
                    components::text_input(ui, &mut col.name, "column_name", NAME_W);
                });
                ui.add_enabled_ui(!col.drop, |ui| {
                    ui.spacing_mut().interact_size.y = style::CONTROL_H;
                    egui::ComboBox::from_id_salt(("schema_col_type", i))
                        .height(320.0)
                        .selected_text(if col.data_type.is_empty() {
                            "TEXT"
                        } else {
                            &col.data_type
                        })
                        .width(TYPE_W)
                        .show_ui(ui, |ui| {
                            components::text_input(
                                ui,
                                &mut col.data_type,
                                "Manual data type…",
                                210.0,
                            );
                            ui.separator();
                            for ty in db_type_options(db_kind) {
                                ui.selectable_value(&mut col.data_type, ty.to_string(), *ty);
                            }
                        });
                });
                ui.allocate_ui_with_layout(
                    egui::vec2(NULL_W, style::CONTROL_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        components::accent_checkbox(ui, !col.drop, &mut col.nullable, None);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(PK_W, style::CONTROL_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        components::accent_checkbox(ui, !col.drop, &mut col.primary_key, None);
                    },
                );
                ui.add_enabled_ui(!col.drop, |ui| {
                    components::text_input(ui, &mut col.default, "default...", DEFAULT_W);
                });

                let removable =
                    (col.is_existing || mode == SchemaEditorMode::Edit || i > 0) && !col.drop;
                let remove_hover = if col.is_existing {
                    "Mark column for deletion"
                } else {
                    "Remove column"
                };
                let keep_hover = "Keep this column";

                ui.allocate_ui_with_layout(
                    egui::vec2(ACTION_W, style::CONTROL_H),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if col.drop {
                            if components::Btn::new("Keep")
                                .tooltip(keep_hover)
                                .show(ui)
                                .clicked()
                            {
                                col.drop = false;
                            }
                        } else {
                            let img = egui::Image::new(icons::trash())
                                .fit_to_exact_size(egui::vec2(13.0, 13.0))
                                .tint(palette::DANGER());
                            let resp = ui
                                .add_enabled(
                                    removable,
                                    egui::Button::image(img)
                                        .frame(false)
                                        .min_size(egui::vec2(20.0, 20.0)),
                                )
                                .on_hover_text(remove_hover);
                            if resp.clicked() {
                                if col.is_existing {
                                    col.drop = true;
                                } else {
                                    to_remove = Some(i);
                                }
                            }
                        }
                    },
                );
            });
        });
        ui.add_space(4.0);
    }

    if columns.is_empty() {
        let frame = egui::Frame::new()
            .fill(palette::SURFACE().linear_multiply(0.45))
            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
            .corner_radius(egui::CornerRadius::same(style::radius::SM))
            .inner_margin(egui::Margin::same(12));
        frame.show(ui, |ui| {
            ui.label(
                egui::RichText::new("No columns yet")
                    .color(palette::TEXT_FAINT())
                    .size(12.0),
            );
        });
    }

    if let Some(i) = to_remove {
        columns.remove(i);
    }

    ui.add_space(6.0);
    if components::button(ui, icons::plus(), "Add Column", true).clicked() {
        columns.push(crate::schema::ColumnDraft::new_empty());
    }
}

fn schema_column_header(ui: &mut egui::Ui, label: &str, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 17.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(10.5)
                    .strong()
                    .color(palette::TEXT_FAINT()),
            );
        },
    );
}

pub(super) fn schema_indexes_tab(ui: &mut egui::Ui, indexes: &mut Vec<crate::schema::IndexDraft>) {
    let mut to_remove: Option<usize> = None;
    ui.spacing_mut().item_spacing.y = 0.0;

    for (i, idx) in indexes.iter_mut().enumerate() {
        let row_color = if idx.drop {
            Some(palette::DANGER().linear_multiply(0.12))
        } else if !idx.is_existing {
            Some(palette::ACCENT().linear_multiply(0.10))
        } else {
            None
        };

        let frame = egui::Frame::new()
            .fill(row_color.unwrap_or(egui::Color32::TRANSPARENT))
            .inner_margin(egui::Margin::symmetric(4, 3));

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                components::text_input_enabled(ui, !idx.drop, &mut idx.name, "index_name", 150.0);
                components::text_input_enabled(
                    ui,
                    !idx.drop,
                    &mut idx.columns_raw,
                    "col1, col2",
                    160.0,
                );
                ui.add_space(2.0);
                components::accent_checkbox(ui, !idx.drop, &mut idx.unique, Some("Unique"));

                if idx.is_existing {
                    let (label, hover) = if idx.drop {
                        ("Restore", "Keep this index")
                    } else {
                        ("Drop", "Mark index for removal")
                    };
                    if components::Btn::new(label)
                        .show(ui)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        idx.drop = !idx.drop;
                    }
                } else if components::Btn::ghost_icon(icons::close())
                    .tooltip("Remove index")
                    .show(ui)
                    .clicked()
                {
                    to_remove = Some(i);
                }
            });
        });
    }

    if let Some(i) = to_remove {
        indexes.remove(i);
    }

    ui.add_space(4.0);
    if components::Btn::new("Add Index")
        .icon(icons::plus())
        .show(ui)
        .clicked()
    {
        indexes.push(crate::schema::IndexDraft::new_empty());
    }
}

pub(super) fn schema_fk_tab(ui: &mut egui::Ui, fks: &mut Vec<crate::schema::FkDraft>) {
    use dbcore::FkAction;

    let mut to_remove: Option<usize> = None;

    for (i, fk) in fks.iter_mut().enumerate() {
        let row_color = if fk.drop {
            Some(palette::DANGER().linear_multiply(0.12))
        } else if !fk.is_existing {
            Some(palette::ACCENT().linear_multiply(0.10))
        } else {
            None
        };

        let frame = if let Some(c) = row_color {
            egui::Frame::new()
                .fill(c)
                .inner_margin(egui::Margin::symmetric(4, 2))
        } else {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 2))
        };

        frame.show(ui, |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Constraint:");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.constraint_name,
                        "fk_name (optional)",
                        160.0,
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Columns:");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.columns_raw,
                        "col1, col2",
                        130.0,
                    );
                    icons::show_weak(ui, icons::chevron_right(), 14.0);
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.ref_table,
                        "ref_table",
                        110.0,
                    );
                    ui.label("(");
                    components::text_input_enabled(
                        ui,
                        !fk.drop,
                        &mut fk.ref_columns_raw,
                        "ref_col",
                        90.0,
                    );
                    ui.label(")");
                });
                ui.horizontal(|ui| {
                    ui.label("On Delete:");
                    egui::ComboBox::from_id_salt(format!("fk_action_{i}"))
                        .selected_text(fk.on_delete.label())
                        .show_ui(ui, |ui| {
                            for action in FkAction::ALL {
                                ui.selectable_value(&mut fk.on_delete, *action, action.label());
                            }
                        });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if fk.is_existing {
                            let (label, hover) = if fk.drop {
                                ("Restore", "Keep this FK")
                            } else {
                                ("Drop", "Remove FK constraint")
                            };
                            if components::Btn::new(label)
                                .show(ui)
                                .on_hover_text(hover)
                                .clicked()
                            {
                                fk.drop = !fk.drop;
                            }
                        } else if components::Btn::ghost_icon(icons::close())
                            .tooltip("Remove FK")
                            .show(ui)
                            .clicked()
                        {
                            to_remove = Some(i);
                        }
                    });
                });
                ui.add_space(2.0);
            });
        });
        ui.separator();
    }

    if let Some(i) = to_remove {
        fks.remove(i);
    }

    ui.add_space(4.0);
    if components::Btn::new("Add Foreign Key")
        .icon(icons::plus())
        .show(ui)
        .clicked()
    {
        fks.push(crate::schema::FkDraft::new_empty());
    }
}
