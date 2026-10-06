//! Structure rendering and interaction.

use crate::components;
use crate::icons;
use crate::style::palette;

/// The Structure view of a table tab: its introspected columns, indexes, and foreign keys
/// as read-only grids, styled after the results grid (TablePlus's "Structure" mode).
pub(super) fn structure_view(ui: &mut egui::Ui, info: &dbcore::TableInfo) {
    use egui_extras::{Column, TableBuilder};

    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
    let header = |ui: &mut egui::Ui, title: &str| {
        components::paint_table_header_cell(ui);
        ui.add(
            egui::Label::new(
                egui::RichText::new(title)
                    .font(egui::TextStyle::Heading.resolve(ui.style()))
                    .color(palette::TEXT()),
            )
            .selectable(false),
        );
    };

    egui::ScrollArea::vertical()
        .id_salt("structure_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            components::section_header(ui, "Columns");
            ui.add_space(2.0);
            TableBuilder::new(ui)
                .id_salt("structure_columns")
                .striped(true)
                .resizable(true)
                .vscroll(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .auto_shrink([false, true])
                .column(Column::exact(30.0)) // row-number gutter
                .column(Column::initial(220.0).at_least(60.0).clip(true))
                .column(Column::initial(160.0).at_least(60.0).clip(true))
                .column(Column::initial(90.0).at_least(60.0).clip(true))
                .column(Column::remainder().at_least(60.0).clip(true))
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        components::paint_table_header_cell(ui);
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("#")
                                .color(palette::TEXT_FAINT())
                                .font(egui::TextStyle::Small.resolve(ui.style())),
                        );
                    });
                    for title in ["column_name", "data_type", "nullable", "key"] {
                        h.col(|ui| header(ui, title));
                    }
                })
                .body(|mut body| {
                    for (i, col) in info.columns.iter().enumerate() {
                        body.row(row_height, |mut row| {
                            row.col(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(4.0);
                                        ui.weak(egui::RichText::new(format!("{}", i + 1)).font(
                                            egui::FontId::new(10.5, egui::FontFamily::Monospace),
                                        ));
                                    },
                                );
                            });
                            row.col(|ui| {
                                if col.primary_key {
                                    icons::show_colored(ui, icons::key(), 13.0, palette::ACCENT());
                                    ui.add_space(2.0);
                                }
                                ui.label(&col.name);
                            });
                            row.col(|ui| {
                                ui.label(&col.data_type);
                            });
                            row.col(|ui| {
                                if col.nullable {
                                    ui.label("YES");
                                } else {
                                    ui.colored_label(palette::TEXT_WEAK(), "NO");
                                }
                            });
                            row.col(|ui| {
                                if col.primary_key {
                                    ui.colored_label(palette::ACCENT(), "PRIMARY");
                                }
                            });
                        });
                    }
                });

            if !info.indexes.is_empty() {
                ui.add_space(12.0);
                components::section_header(ui, "Indexes");
                ui.add_space(2.0);
                TableBuilder::new(ui)
                    .id_salt("structure_indexes")
                    .striped(true)
                    .resizable(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .auto_shrink([false, true])
                    .column(Column::exact(30.0))
                    .column(Column::initial(260.0).at_least(60.0).clip(true))
                    .column(Column::initial(90.0).at_least(60.0).clip(true))
                    .column(Column::remainder().at_least(60.0).clip(true))
                    .header(24.0, |mut h| {
                        h.col(|ui| {
                            components::paint_table_header_cell(ui);
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("#")
                                    .color(palette::TEXT_FAINT())
                                    .font(egui::TextStyle::Small.resolve(ui.style())),
                            );
                        });
                        for title in ["index_name", "unique", "columns"] {
                            h.col(|ui| header(ui, title));
                        }
                    })
                    .body(|mut body| {
                        for (i, idx) in info.indexes.iter().enumerate() {
                            body.row(row_height, |mut row| {
                                row.col(|ui| {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_space(4.0);
                                            ui.weak(
                                                egui::RichText::new(format!("{}", i + 1)).font(
                                                    egui::FontId::new(
                                                        10.5,
                                                        egui::FontFamily::Monospace,
                                                    ),
                                                ),
                                            );
                                        },
                                    );
                                });
                                row.col(|ui| {
                                    icons::show_weak(ui, icons::index(), 13.0);
                                    ui.add_space(2.0);
                                    ui.label(&idx.name);
                                });
                                row.col(|ui| {
                                    if idx.unique {
                                        ui.colored_label(palette::ACCENT(), "UNIQUE");
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(idx.columns.join(", "));
                                });
                            });
                        }
                    });
            }

            if !info.foreign_keys.is_empty() {
                ui.add_space(12.0);
                components::section_header(ui, "Foreign keys");
                ui.add_space(2.0);
                TableBuilder::new(ui)
                    .id_salt("structure_fks")
                    .striped(true)
                    .resizable(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .auto_shrink([false, true])
                    .column(Column::exact(30.0))
                    .column(Column::initial(220.0).at_least(60.0).clip(true))
                    .column(Column::initial(140.0).at_least(60.0).clip(true))
                    .column(Column::initial(220.0).at_least(60.0).clip(true))
                    .column(Column::initial(100.0).at_least(60.0).clip(true))
                    .column(Column::remainder().at_least(60.0).clip(true))
                    .header(24.0, |mut h| {
                        h.col(|ui| {
                            components::paint_table_header_cell(ui);
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("#")
                                    .color(palette::TEXT_FAINT())
                                    .font(egui::TextStyle::Small.resolve(ui.style())),
                            );
                        });
                        for title in [
                            "constraint_name",
                            "columns",
                            "references",
                            "on_delete",
                            "on_update",
                        ] {
                            h.col(|ui| header(ui, title));
                        }
                    })
                    .body(|mut body| {
                        for (i, fk) in info.foreign_keys.iter().enumerate() {
                            body.row(row_height, |mut row| {
                                row.col(|ui| {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_space(4.0);
                                            ui.weak(
                                                egui::RichText::new(format!("{}", i + 1)).font(
                                                    egui::FontId::new(
                                                        10.5,
                                                        egui::FontFamily::Monospace,
                                                    ),
                                                ),
                                            );
                                        },
                                    );
                                });
                                row.col(|ui| {
                                    icons::show_weak(ui, icons::connect(), 13.0);
                                    ui.add_space(2.0);
                                    if fk.name.is_empty() {
                                        ui.colored_label(palette::TEXT_FAINT(), "(unnamed)");
                                    } else {
                                        ui.label(&fk.name);
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(fk.columns.join(", "));
                                });
                                row.col(|ui| {
                                    // Qualify the target with its schema only when it lives
                                    // outside this table's own schema.
                                    let target = match (&fk.ref_schema, &info.schema) {
                                        (Some(rs), Some(s)) if rs != s => {
                                            format!("{rs}.{}", fk.ref_table)
                                        }
                                        _ => fk.ref_table.clone(),
                                    };
                                    ui.label(format!("{target} ({})", fk.ref_columns.join(", ")));
                                });
                                row.col(|ui| {
                                    ui.label(&fk.on_delete);
                                });
                                row.col(|ui| {
                                    ui.label(&fk.on_update);
                                });
                            });
                        }
                    });
            }
        });
}
