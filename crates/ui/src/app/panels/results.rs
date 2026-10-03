//! Results rendering and interaction.

use super::pager::BarDensity;
use super::query_plan::plan_viewer;
use super::structure::structure_view;
use crate::app::{result_status, Action, DbGuiApp, QueryEditorPlacement, QueryTab, TabView};
use crate::components;
use crate::filter;
use crate::filter::FilterEvent;
use crate::grid::results_grid;
use crate::icons;
use crate::style;
use crate::style::palette;

/// The "+ Row" / "+ Column" / "+ Index" button beside the view modes. When the bar is short of
/// room it keeps just the plus, with the full label in the tooltip.
fn add_button(ui: &mut egui::Ui, density: BarDensity, label: &str, enabled: bool) -> bool {
    if density.labelled_buttons() {
        components::button(ui, icons::plus(), label, enabled).clicked()
    } else {
        components::soft_icon_button(
            ui,
            icons::plus(),
            &format!("Add {}", label.to_lowercase()),
            enabled,
        )
        .clicked()
    }
}

impl DbGuiApp {
    /// One closable, scrollable tab per statement returned by Run All.
    pub(in crate::app) fn batch_result_bar(&mut self, root: &mut egui::Ui) {
        let idx = self.active_query_tab;
        let count = self.tabs[idx].batch_results.len();
        if self.tabs[idx].kind != crate::components::QueryTabKind::Query || count <= 1 {
            return;
        }
        let selected = self.tabs[idx].active_batch_result.min(count - 1);
        let tab_id = self.tabs[idx].id;
        let mut choice = None;
        let mut close = None;
        // The 29-point tabs stand on the bar's bottom separator, like the query tab strip:
        // all the vertical margin goes on top, so the selected tab joins the grid below
        // instead of floating against the bar's upper edge.
        egui::Panel::top(egui::Id::new(("batch_result_bar", self.tabs[idx].id)))
            .resizable(false)
            .exact_size(35.0)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 6,
                        bottom: 0,
                    })
                    .fill(palette::PANEL()),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt(("batch_result_scroll", tab_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for index in 0..count {
                                ui.push_id(("batch_result", index), |ui| {
                                    let response = components::result_tab_item(
                                        ui,
                                        &format!("Query {}", index + 1),
                                        index == selected,
                                        self.tabs[idx].batch_result_has_error(index),
                                    );
                                    if response.close {
                                        close = Some(index);
                                    } else if response.clicked {
                                        choice = Some(index);
                                    }
                                });
                            }
                        });
                    });
            });
        if let Some(index) = close {
            self.tabs[idx].close_batch_result(index);
            self.touch_result(idx);
        } else if let Some(index) = choice.filter(|index| *index != selected) {
            self.tabs[idx].activate_batch_result(index);
            self.touch_result(idx);
        }
    }

    /// The TablePlus-style filter strip directly above the grid. Only shown when toggled on
    /// and a result with columns is loaded. Edits mutate `self.filter` directly; Apply or
    /// Clear rebuilds the view.
    pub(in crate::app) fn filter_bar(&mut self, root: &mut egui::Ui) {
        let idx = self.active_query_tab;
        // The filter applies to data rows; it has no meaning over another result view or
        // while the schema editor occupies the central panel.
        if !self.tabs[idx].filter.visible
            || self.tabs[idx].view != TabView::Data
            || self.tabs[idx].schema_editor.is_some()
        {
            return;
        }
        let col_names: Vec<String> = match &self.tabs[idx].result {
            Some(res) if res.column_count() > 0 => {
                res.columns.iter().map(|c| c.name.clone()).collect()
            }
            _ => return,
        };

        let mut event: Option<FilterEvent> = None;
        egui::Panel::top(egui::Id::new(("filter_bar", self.tabs[idx].id)))
            .resizable(false)
            .frame(
                style::workspace_frame(palette::PANEL())
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                let scope = if self.result_filter_runs_on_database(idx) {
                    "Filter scope: entire table"
                } else {
                    "Filter scope: loaded rows only"
                };
                ui.label(
                    egui::RichText::new(scope)
                        .small()
                        .color(palette::TEXT_WEAK()),
                );
                event = filter::ui(ui, &mut self.tabs[idx].filter, &col_names);
            });

        match event {
            Some(FilterEvent::Apply) => self.apply_result_filter(idx, false),
            Some(FilterEvent::Clear) => self.apply_result_filter(idx, true),
            None => {}
        }
    }

    /// Contextual result switch. Query tabs dock Data / Message / Chart beneath the result;
    /// table and view tabs expose their editable Structure and Indexes by the data surface.
    pub(in crate::app) fn view_mode_bar(
        &mut self,
        root: &mut egui::Ui,
        placement: QueryEditorPlacement,
        force_top: bool,
        actions: &mut Vec<Action>,
    ) {
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let query_result_tabs = self.tabs[idx].kind == crate::components::QueryTabKind::Query;
        let editable_table = self.tabs[idx].kind == crate::components::QueryTabKind::Table;
        let table_or_view = matches!(
            self.tabs[idx].kind,
            crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
        );
        if !query_result_tabs && !table_or_view {
            self.tabs[idx].view = TabView::Data;
            return;
        }
        let table_info = self.structure_table(idx).cloned();
        if editable_table && matches!(self.tabs[idx].view, TabView::Structure | TabView::Indexes) {
            let schema_view = self.tabs[idx].view;
            if self.tabs[idx].schema_editor.is_none() && !self.tabs[idx].table_metadata_pending {
                if let Some(info) = table_info.as_ref() {
                    if !info.columns.is_empty() {
                        let kind = self
                            .active()
                            .map(|active| active.db.kind())
                            .unwrap_or(dbcore::DbKind::Sqlite);
                        self.tabs[idx].schema_editor = Some(crate::schema::ObjectEditor::Table(
                            crate::schema::SchemaEditor::edit_table(info, kind),
                        ));
                    }
                }
            }
            if let Some(crate::schema::ObjectEditor::Table(editor)) =
                self.tabs[idx].schema_editor.as_mut()
            {
                editor.active_tab = if schema_view == TabView::Indexes {
                    crate::schema::SchemaTab::Indexes
                } else {
                    crate::schema::SchemaTab::Columns
                };
            }
        }
        // A split renders this bar twice in one frame. Its panel id must belong to the tab;
        // otherwise both bars create the same child Ui and their segmented buttons share click
        // state, making Data/Structure/Indexes switch in both panes together.
        let panel_id = egui::Id::new(("view_mode_bar", tab_id));
        let panel = match (query_result_tabs, placement) {
            // Query result modes belong beneath the data surface, matching the statement tabs
            // above it. Table/view modes keep following their data-first editor placement.
            (true, _) | (false, QueryEditorPlacement::Bottom) => egui::Panel::bottom(panel_id),
            (false, QueryEditorPlacement::Top) => egui::Panel::top(panel_id),
        };
        panel
            .resizable(false)
            .exact_size(38.0)
            .frame(
                style::workspace_frame(palette::PANEL())
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show_separator_line(true)
            .show_inside(root, |ui| {
                let bar_width = ui.available_width();
                let density = BarDensity::for_width(bar_width);
                ui.horizontal(|ui| {
                    if query_result_tabs {
                        let modes = [TabView::Data, TabView::Message, TabView::Chart];
                        let selected = modes
                            .iter()
                            .position(|mode| *mode == self.tabs[idx].view)
                            .unwrap_or(0);
                        let choice = components::segmented_sized(
                            ui,
                            &[
                                (icons::table(), "Data"),
                                (icons::code(), "Message"),
                                (icons::diagram(), "Chart"),
                            ],
                            selected,
                            270.0,
                            false,
                        );
                        self.tabs[idx].view = modes[choice];
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.result_filter_button(ui, actions);
                        });
                        return;
                    }
                    if !editable_table {
                        let modes = [TabView::Data, TabView::Structure];
                        let selected = modes
                            .iter()
                            .position(|mode| *mode == self.tabs[idx].view)
                            .unwrap_or(0);
                        let choice = components::segmented_sized(
                            ui,
                            &[(icons::table(), "Data"), (icons::column(), "Structure")],
                            selected,
                            200.0,
                            false,
                        );
                        self.tabs[idx].view = modes[choice];
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.result_filter_button(ui, actions);
                        });
                        return;
                    }
                    let editing = self.tabs[idx].schema_editor.is_some();
                    let modes = [TabView::Data, TabView::Structure, TabView::Indexes];
                    let selected = modes
                        .iter()
                        .position(|mode| *mode == self.tabs[idx].view)
                        .unwrap_or(0);
                    let choice = components::segmented_sized(
                        ui,
                        &[
                            (icons::table(), "Data"),
                            (icons::column(), "Structure"),
                            (icons::index(), "Indexes"),
                        ],
                        selected,
                        // The DDL button only exists on Structure.
                        density.segment_width(
                            bar_width,
                            300.0,
                            if self.tabs[idx].view == TabView::Structure {
                                50.0
                            } else {
                                0.0
                            },
                        ),
                        false,
                    );
                    if choice != selected {
                        self.tabs[idx].view = modes[choice];
                        if choice == 0 {
                            if editing {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::CancelSchema),
                                });
                            }
                        } else if !editing {
                            if let Some(info) = table_info.clone() {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::OpenEditTable(info)),
                                });
                            } else {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::LoadTableMetadata),
                                });
                            }
                        } else if let Some(crate::schema::ObjectEditor::Table(editor)) =
                            self.tabs[idx].schema_editor.as_mut()
                        {
                            editor.active_tab = if choice == 1 {
                                crate::schema::SchemaTab::Columns
                            } else {
                                crate::schema::SchemaTab::Indexes
                            };
                        }
                    }
                    match self.tabs[idx].view {
                        TabView::Data => {
                            ui.add_space(6.0);
                            let can_add_row = self.tabs[idx].edits.editable();
                            if add_button(ui, density, "Row", can_add_row) {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::AddDataRow),
                                });
                            }
                        }
                        TabView::Structure => {
                            ui.add_space(6.0);
                            if add_button(ui, density, "Column", editing) {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::AddSchemaColumn),
                                });
                            }
                        }
                        TabView::Indexes => {
                            ui.add_space(6.0);
                            if add_button(ui, density, "Index", editing) {
                                actions.push(Action::ForTab {
                                    tab_id,
                                    action: Box::new(Action::AddSchemaIndex),
                                });
                            }
                        }
                        TabView::Message | TabView::Chart => {}
                    }
                    if self.tabs[idx].view == TabView::Structure {
                        ui.add_space(6.0);
                        if let (Some(info), Some(kind)) = (
                            table_info.as_ref(),
                            self.active().map(|active| active.db.kind()),
                        ) {
                            let ddl_button = components::Btn::new("DDL")
                                .tooltip("Show the table creation DDL")
                                .show(ui);
                            let statements = crate::schema::build_table_definition_ddl(info, kind);
                            let ddl = statements.join("\n\n");
                            let popup_frame = egui::Frame::popup(ui.style())
                                .fill(palette::PANEL())
                                .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
                                .corner_radius(egui::CornerRadius::same(14))
                                .inner_margin(egui::Margin::same(10));
                            let popup = egui::Popup::from_toggle_button_response(&ddl_button)
                                .id(ddl_button.id.with("table_ddl"))
                                .align(egui::RectAlign::TOP_END)
                                .align_alternatives(&[egui::RectAlign::TOP_START])
                                .gap(9.0)
                                .width(720.0)
                                .frame(popup_frame)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                    ui.set_min_width(680.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} — {} DDL",
                                            info.name,
                                            kind.label()
                                        ))
                                        .strong()
                                        .color(palette::TEXT()),
                                    );
                                    ui.add_space(6.0);
                                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                                    egui::ScrollArea::both()
                                        .id_salt("table_ddl_preview")
                                        .max_height(440.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            let job = crate::highlight::highlight_sql_cached(
                                                ui.ctx(),
                                                &ddl,
                                                font,
                                            );
                                            ui.add(egui::Label::new(job).selectable(true));
                                        });
                                });
                            if let Some(response) = popup {
                                let rect = response.response.rect;
                                let anchor_x = ddl_button
                                    .rect
                                    .center()
                                    .x
                                    .clamp(rect.left() + 10.0, rect.right() - 10.0);
                                let left = egui::pos2(anchor_x - 8.0, rect.bottom() - 1.0);
                                let right = egui::pos2(anchor_x + 8.0, rect.bottom() - 1.0);
                                let tip = egui::pos2(anchor_x, rect.bottom() + 8.0);
                                let painter = ui.ctx().layer_painter(response.response.layer_id);
                                painter.add(egui::Shape::convex_polygon(
                                    vec![left, right, tip],
                                    palette::PANEL(),
                                    egui::Stroke::NONE,
                                ));
                                let stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG());
                                painter.line_segment([left, tip], stroke);
                                painter.line_segment([tip, right], stroke);
                            }
                        }
                    }
                    // Paging describes the table result, so its range and navigation stay visible
                    // while inspecting Structure or Indexes as well as Data.
                    self.pager(ui, density, actions);
                });
            });
    }

    pub(in crate::app) fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        let idx = self.active_query_tab;
        let result_frame = if self.show_query_console
            && self.tabs[idx].schema_editor.is_none()
            && matches!(
                self.tabs[idx].kind,
                crate::components::QueryTabKind::Query
                    | crate::components::QueryTabKind::Function
                    | crate::components::QueryTabKind::Procedure
                    | crate::components::QueryTabKind::Trigger
            ) {
            // The SQL workspace bar directly above is this result card's header.
            style::workspace_frame(palette::PANEL())
                // A normal frame would draw its top stroke across the join. Keep the
                // content inset at five points (the old four plus its one-point stroke).
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::same(5))
                .outer_margin(egui::Margin {
                    left: style::WORKSPACE_GUTTER,
                    right: style::WORKSPACE_GUTTER,
                    top: 0,
                    bottom: style::WORKSPACE_GUTTER_Y,
                })
                .corner_radius(egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: style::radius::LG,
                    se: style::radius::LG,
                })
        } else {
            style::workspace_frame(palette::PANEL())
        };
        // A portable table editor temporarily replaces its Diagram canvas, then returns to it.
        if self.tabs[idx].schema_editor.is_some() {
            egui::CentralPanel::default()
                .frame(result_frame)
                .show_inside(root, |ui| {
                    self.schema_editor_view(ui, actions);
                });
            return;
        }
        let schema_mode = matches!(self.tabs[idx].view, TabView::Structure | TabView::Indexes);
        let table_metadata_incomplete = self
            .structure_table(idx)
            .is_none_or(|table| table.columns.is_empty());
        if schema_mode
            && self.tabs[idx].schema_editor.is_none()
            && (self.tabs[idx].table_metadata_pending || table_metadata_incomplete)
        {
            let connection_metadata_pending = self.tabs[idx]
                .conn_id
                .as_ref()
                .is_some_and(|conn_id| self.connection_jobs.contains(conn_id));
            let loading = self.tabs[idx].table_metadata_pending || connection_metadata_pending;
            egui::CentralPanel::default()
                .frame(result_frame)
                .show_inside(root, |ui| {
                    if loading {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new("Loading table structure…")
                                    .color(palette::TEXT_WEAK()),
                            );
                        });
                    } else {
                        components::empty_state(
                            ui,
                            icons::mood_sad_dizzy(),
                            "Table structure unavailable",
                            "Its columns couldn't be read from the database.",
                        );
                    }
                });
            return;
        }
        // A Diagram tab owns the whole central panel (the editor and result bars were
        // already skipped in `draw`). Slim vertical margins keep the header band tight
        // between the tab strip and the canvas.
        if self.tabs[idx].kind == crate::components::QueryTabKind::Activity {
            egui::CentralPanel::default()
                .frame(style::workspace_frame(palette::PANEL()))
                .show_inside(root, |ui| {
                    self.activity_view(ui, actions);
                });
            return;
        }
        if self.tabs[idx].kind == crate::components::QueryTabKind::Diagram {
            egui::CentralPanel::default()
                .frame(
                    style::workspace_frame(palette::PANEL())
                        .inner_margin(egui::Margin::symmetric(8, 2)),
                )
                .show_inside(root, |ui| {
                    self.erd_view(ui, actions);
                });
            return;
        }
        // Structure mode replaces the whole grid with the table's introspected definition.
        // Missing or name-only metadata was handled by the loading/unavailable state above.
        if self.tabs[idx].view == TabView::Structure {
            if let Some(info) = self.structure_table(idx).cloned() {
                egui::CentralPanel::default()
                    .frame(result_frame)
                    .show_inside(root, |ui| {
                        structure_view(ui, &info);
                    });
                return;
            }
        }
        if self.tabs[idx].kind == crate::components::QueryTabKind::Query {
            match self.tabs[idx].view {
                TabView::Message => {
                    let query_error = self.tabs[idx].query_error.as_deref();
                    if query_error.is_none() && self.tabs[idx].result.is_none() {
                        egui::CentralPanel::default()
                            .frame(result_frame)
                            .show_inside(root, crate::pet::show);
                        return;
                    }
                    let message = query_error.map(str::to_owned).unwrap_or_else(|| {
                        self.tabs[idx]
                            .result
                            .as_ref()
                            .map(result_status)
                            .unwrap_or_else(|| "Run a query to see execution details".to_string())
                    });
                    let color = if query_error.is_some() {
                        palette::DANGER()
                    } else {
                        palette::TEXT_WEAK()
                    };
                    egui::CentralPanel::default()
                        .frame(result_frame.inner_margin(egui::Margin::same(12)))
                        .show_inside(root, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(message).monospace().color(color),
                                )
                                .wrap()
                                .selectable(true),
                            );
                        });
                    return;
                }
                TabView::Chart => {
                    egui::CentralPanel::default()
                        .frame(result_frame)
                        .show_inside(root, |ui| {
                            let tab = &mut self.tabs[idx];
                            if tab.result.is_none() && tab.query_error.is_none() {
                                crate::pet::show(ui);
                            } else if let Some(result) = tab.result.as_ref() {
                                let response =
                                    crate::chart::show(ui, result, &tab.row_order, &mut tab.chart);
                                if response.export_requested {
                                    actions.push(Action::ExportChart);
                                }
                            }
                        });
                    return;
                }
                TabView::Data | TabView::Structure | TabView::Indexes => {}
            }
        }
        if self.tabs[idx].plan_result && self.tabs[idx].view == TabView::Data {
            let plan_kind = self.tab_db_kind(idx);
            egui::CentralPanel::default()
                .frame(result_frame)
                .show_inside(root, |ui| {
                    if let Some(result) = self.tabs[idx].result.as_ref() {
                        plan_viewer(ui, result, plan_kind);
                    } else {
                        crate::pet::show(ui);
                    }
                });
            return;
        }
        let editable = self.tabs[idx].edits.editable() && !self.is_tab_querying(self.tabs[idx].id);
        // Per-column FK labels for the grid's link/"Follow →" affordance (owned, so it doesn't
        // hold a borrow across the mutable tab access below).
        let fk_cols = self.fk_column_labels(idx);
        let status_msg = &self.status_msg;
        let emoji = &self.emoji;
        let tab_id = self.tabs[idx].id;
        let kind = self.tabs[idx].kind;
        let query_error = self.tabs[idx].query_error.clone();
        let loading = self.is_tab_querying(tab_id);
        let can_load_more = !loading
            && matches!(
                self.tabs[idx].kind,
                crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
            )
            && !self.tabs[idx].page_exhausted
            && self.tabs[idx].sort.is_none()
            && (self.tabs[idx].edits.source.is_some()
                || self.tabs[idx].edits.pending_source.is_some());
        let QueryTab {
            result,
            row_order,
            sort,
            selection,
            edits,
            pending_scroll,
            ..
        } = &mut self.tabs[idx];
        let sort = *sort;
        egui::CentralPanel::default()
            .frame(result_frame)
            .show_inside(root, |ui| {
                if query_error.is_some() {
                    crate::pet::show(ui);
                    return;
                }
                match result.as_ref() {
                    Some(result) if result.column_count() > 0 => {
                        let resp = results_grid(
                            ui,
                            result,
                            row_order,
                            sort,
                            selection,
                            edits,
                            editable,
                            tab_id,
                            pending_scroll.take(),
                            emoji,
                            &fk_cols,
                        );
                        if resp.near_end && can_load_more {
                            actions.push(Action::LoadMoreRows);
                        }
                        if let Some(cmd) = resp.sort {
                            actions.push(match cmd {
                                crate::grid::SortCmd::Asc(col) => {
                                    Action::SetSort { col, asc: true }
                                }
                                crate::grid::SortCmd::Desc(col) => {
                                    Action::SetSort { col, asc: false }
                                }
                                crate::grid::SortCmd::Clear => Action::ClearSort,
                            });
                        }
                        if let Some(col) = resp.filter_column {
                            actions.push(Action::FilterColumn { tab_id, col });
                        }
                        if let Some(click) = resp.selected {
                            selection.apply_click(click);
                        }
                        // Right-click "Copy as …": a row right-clicked while outside the selection
                        // becomes the sole target first, then the whole selection is copied.
                        if let Some((disp, fmt)) = resp.copy {
                            if !selection.contains(disp) {
                                selection.select_one(disp);
                            }
                            actions.push(Action::CopyRows(fmt));
                        }
                        // Set NULL / Set Empty / Duplicate Row target the selection the same way.
                        if let Some((disp, col, to)) = resp.set_cells {
                            if !selection.contains(disp) {
                                selection.select_one(disp);
                            }
                            actions.push(Action::SetCells { col, to });
                        }
                        if let Some(disp) = resp.duplicate {
                            if !selection.contains(disp) {
                                selection.select_one(disp);
                            }
                            actions.push(Action::DuplicateRows);
                        }
                        // "Follow →" on a foreign-key cell: open the referenced table, filtered.
                        if let Some((row, col)) = resp.follow_fk {
                            actions.push(Action::FollowForeignKey { row, col });
                        }
                        if let Some((column, type_name, value)) = resp.view_value {
                            if let Some(viewer) =
                                crate::value_viewer::ValueViewer::new(&column, &type_name, &value)
                            {
                                actions.push(Action::OpenValueViewer(viewer));
                            }
                        }
                        if let Some((row, col)) = resp.replace_blob {
                            actions.push(Action::ReplaceBlobFromFile { row, col });
                        }
                        use crate::edit::{
                            begin_cell_edit, disp_to_raw, original_value, selection_fan_out,
                            settle_active,
                        };
                        if let Some(fill) = resp.fill {
                            settle_active(edits, result);
                            if let Some(src_raw) =
                                disp_to_raw(row_order, edits.new_rows, fill.from_disp)
                            {
                                let source = edits
                                    .staged(src_raw, fill.col)
                                    .cloned()
                                    .or_else(|| original_value(result, src_raw, fill.col));
                                if let Some(value) = source {
                                    // One undo group so the whole fill-drag takes a single Cmd/Ctrl+Z.
                                    edits.begin_undo_group();
                                    for disp in fill.from_disp.min(fill.to_disp)
                                        ..=fill.from_disp.max(fill.to_disp)
                                    {
                                        if disp == fill.from_disp {
                                            continue;
                                        }
                                        if let Some(raw) =
                                            disp_to_raw(row_order, edits.new_rows, disp)
                                        {
                                            if edits.deleted.contains(&raw) {
                                                continue;
                                            }
                                            if let Some(orig) =
                                                original_value(result, raw, fill.col)
                                            {
                                                edits.stage(raw, fill.col, value.clone(), &orig);
                                            }
                                        }
                                    }
                                    edits.end_undo_group();
                                    selection.select_one(fill.from_disp);
                                    selection.range_to(fill.to_disp);
                                    selection.set_cursor(fill.to_disp, fill.col);
                                    *pending_scroll = Some(fill.to_disp);
                                }
                            }
                        }
                        if let Some(advance) = resp.commit_edit {
                            settle_active(edits, result);
                            // The commit landed → move the cursor and keep editing there. Up/Down
                            // retain the column; Tab/Shift+Tab retain the row. Bool/binary cells are
                            // skipped as editors, though the cursor still parks on them.
                            if edits.active.is_none() {
                                if let Some(dir) = advance {
                                    let (dr, dc) = match dir {
                                        crate::edit::CursorDir::Left => (0, -1),
                                        crate::edit::CursorDir::Right => (0, 1),
                                        crate::edit::CursorDir::Up => (-1, 0),
                                        crate::edit::CursorDir::Down => (1, 0),
                                    };
                                    let len = row_order.len() + edits.new_rows;
                                    if selection.move_cursor(
                                        dr,
                                        dc,
                                        len,
                                        result.column_count(),
                                        false,
                                    ) {
                                        if let Some((nd, nc)) = selection.cursor() {
                                            *pending_scroll = Some(nd);
                                            if let Some(raw) =
                                                disp_to_raw(row_order, edits.new_rows, nd)
                                            {
                                                let bytes = original_value(result, raw, nc)
                                                    .is_some_and(|v| {
                                                        matches!(v, dbcore::Value::Bytes(_))
                                                    });
                                                if edits.col_kind(nc)
                                                    != crate::edit::EditorKind::Bool
                                                    && !bytes
                                                {
                                                    // Tab keeps a multi-row selection, so
                                                    // the next column edits all of it too.
                                                    let fan_out = selection_fan_out(
                                                        selection,
                                                        row_order,
                                                        edits.new_rows,
                                                        result,
                                                        nd,
                                                        nc,
                                                    );
                                                    begin_cell_edit(edits, result, raw, nc);
                                                    edits.set_fan_out(fan_out);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if resp.cancel_edit {
                            edits.cancel_active();
                        }
                        if let Some((disp, c)) = resp.begin_edit {
                            if let Some(raw) = disp_to_raw(row_order, edits.new_rows, disp) {
                                let fan_out = selection_fan_out(
                                    selection,
                                    row_order,
                                    edits.new_rows,
                                    result,
                                    disp,
                                    c,
                                );
                                begin_cell_edit(edits, result, raw, c);
                                edits.set_fan_out(fan_out);
                                // The cursor tracks the editor so Tab-advance moves relative to it.
                                selection.set_cursor(disp, c);
                            }
                        }
                        // A boolean cell flips in place rather than opening an editor. If another
                        // cell's editor is still open (e.g. the user clicked straight from it onto this
                        // bool), settle that first so its typed value isn't silently dropped.
                        if let Some((disp, c)) = resp.toggle {
                            if let Some(raw) = disp_to_raw(row_order, edits.new_rows, disp) {
                                if edits
                                    .active
                                    .as_ref()
                                    .is_some_and(|a| (a.row, a.col) != (raw, c))
                                {
                                    settle_active(edits, result);
                                }
                                if let Some(orig) = original_value(result, raw, c) {
                                    edits.toggle_bool(raw, c, &orig);
                                }
                                selection.set_cursor(disp, c);
                            }
                        }
                        // Double-clicking empty table space appends a new (insert) row, selects it,
                        // and opens an editor on the first text-editable column right away.
                        if resp.add_row {
                            settle_active(edits, result);
                            let new_id = edits.add_new_row();
                            let disp = row_order.len() + edits.new_rows - 1;
                            selection.select_one(disp);
                            let first_col = (0..result.column_count())
                                .find(|&c| edits.col_kind(c) != crate::edit::EditorKind::Bool);
                            if let Some(c) = first_col {
                                edits.begin(
                                    new_id,
                                    c,
                                    &dbcore::Value::Null,
                                    crate::edit::EditOrigin::Grid,
                                );
                                selection.set_cursor(disp, c);
                            }
                        }
                    }
                    Some(_) => {
                        components::empty_state(ui, icons::table(), "No columns", status_msg);
                    }
                    None if loading => {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new("Loading data…").color(palette::TEXT_WEAK()),
                            );
                        });
                    }
                    None => match kind {
                        crate::components::QueryTabKind::Query => {
                            components::empty_illustration(ui);
                        }
                        crate::components::QueryTabKind::Function
                        | crate::components::QueryTabKind::Procedure
                        | crate::components::QueryTabKind::Trigger => components::empty_state(
                            ui,
                            kind.icon(),
                            "No output",
                            "This definition has not been run",
                        ),
                        crate::components::QueryTabKind::Table
                        | crate::components::QueryTabKind::View
                        | crate::components::QueryTabKind::Diagram
                        | crate::components::QueryTabKind::Activity => {
                            components::empty_illustration(ui);
                        }
                    },
                }
            });
    }
}
