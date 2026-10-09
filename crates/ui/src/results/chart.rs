//! Query-result charting and SVG/PNG export.
//!
//! The chart deliberately consumes the same materialized rows and display order as the grid:
//! filters and sorts therefore carry across when the user switches from Data to Chart. Rendering
//! uses the same data rules for the on-screen painter and exported images.

use dbcore::{QueryResult, Value};
use std::{collections::BTreeMap, rc::Rc};

use crate::components;
use crate::icons;
use crate::style::{self, palette};

const MAX_POINTS: usize = 2_000;
const MAX_BARS: usize = 120;
const POPOVER_WIDTH: f32 = 244.0;
const STYLE_WIDTH: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartExportFormat {
    Svg,
    Png,
}

impl ChartExportFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ChartPalette {
    #[default]
    Ember,
    Theme,
    Ocean,
    Sunset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CurveStyle {
    Linear,
    #[default]
    Smooth,
    Step,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FillStyle {
    #[default]
    Gradient,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChartKind {
    #[default]
    Line,
    Area,
    Bar,
    StackedBar,
    Scatter,
    Donut,
}

impl ChartKind {
    const ALL: [Self; 6] = [
        Self::Line,
        Self::Area,
        Self::Bar,
        Self::StackedBar,
        Self::Scatter,
        Self::Donut,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Area => "Area",
            Self::Bar => "Bar",
            Self::StackedBar => "Stacked",
            Self::Scatter => "Scatter",
            Self::Donut => "Donut",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Self::Line => "Line chart",
            Self::Area => "Area chart",
            Self::Bar => "Bar chart",
            Self::StackedBar => "Stacked bar",
            Self::Scatter => "Scatter plot",
            Self::Donut => "Donut chart",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChartState {
    pub(crate) kind: ChartKind,
    /// `None` means a generated 1-based row number.
    pub(crate) x_column: Option<usize>,
    pub(crate) series: Vec<usize>,
    title: String,
    show_legend: bool,
    show_grid: bool,
    show_values: bool,
    start_at_zero: bool,
    show_summary: bool,
    show_points: bool,
    full_numbers: bool,
    line_width: f32,
    point_radius: f32,
    fill_opacity: f32,
    bar_width: f32,
    donut_hole: f32,
    x_title: String,
    y_title: String,
    palette: ChartPalette,
    series_colors: BTreeMap<usize, egui::Color32>,
    custom_y_range: bool,
    y_min: f64,
    y_max: f64,
    curve: CurveStyle,
    fill: FillStyle,
    dashed: bool,
    focused_series: Option<usize>,
    x_search: String,
    y_search: String,
    axes_initialized: bool,
    numeric_columns: Vec<usize>,
    analyzed_shape: (usize, usize),
    cache: Option<(DataKey, Option<Rc<ChartData>>)>,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            kind: ChartKind::Line,
            x_column: None,
            series: Vec::new(),
            title: String::new(),
            show_legend: true,
            show_grid: true,
            show_values: false,
            start_at_zero: false,
            show_summary: false,
            show_points: false,
            full_numbers: false,
            line_width: 1.8,
            point_radius: 2.8,
            fill_opacity: 0.3,
            bar_width: 0.72,
            donut_hole: 0.58,
            x_title: String::new(),
            y_title: String::new(),
            palette: ChartPalette::Ember,
            series_colors: BTreeMap::new(),
            custom_y_range: false,
            y_min: 0.0,
            y_max: 100.0,
            curve: CurveStyle::Smooth,
            fill: FillStyle::Gradient,
            dashed: false,
            focused_series: None,
            x_search: String::new(),
            y_search: String::new(),
            axes_initialized: false,
            numeric_columns: Vec::new(),
            analyzed_shape: (0, 0),
            cache: None,
        }
    }
}

impl ChartState {
    pub(crate) fn sync(&mut self, result: &QueryResult) {
        self.numeric_columns = numeric_columns(result);
        self.analyzed_shape = (result.row_count(), result.column_count());
        self.cache = None;
        if !self.axes_initialized {
            self.x_column =
                (0..result.column_count()).find(|column| !self.numeric_columns.contains(column));
            self.axes_initialized = true;
        }
        self.repair(result);
    }

    fn refresh(&mut self, result: &QueryResult) {
        if self.analyzed_shape != (result.row_count(), result.column_count()) {
            self.sync(result);
        } else {
            self.repair(result);
        }
    }

    fn repair(&mut self, result: &QueryResult) {
        let numeric = &self.numeric_columns;
        if self
            .x_column
            .is_some_and(|column| column >= result.column_count())
        {
            self.x_column = None;
        }
        self.series.retain(|column| {
            numeric.contains(column) && *column != self.x_column.unwrap_or(usize::MAX)
        });
        self.series.sort_unstable();
        self.series.dedup();
        if self
            .focused_series
            .is_some_and(|column| !self.series.contains(&column))
        {
            self.focused_series = None;
        }

        if self.series.is_empty() {
            self.series.extend(
                numeric
                    .iter()
                    .copied()
                    .filter(|column| Some(*column) != self.x_column)
                    .take(1),
            );
        }
    }

    fn data(&mut self, result: &QueryResult, row_order: &[usize]) -> Option<Rc<ChartData>> {
        let key = DataKey {
            kind: self.kind,
            x_column: self.x_column,
            series: self.series.clone(),
            order: chart_rows(row_order, self.kind),
            total_rows: row_order.len(),
        };
        if self.cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let data = build_data_from_rows(result, &key.order, key.total_rows, self).map(Rc::new);
            self.cache = Some((key, data));
        }
        self.cache.as_ref().and_then(|(_, data)| data.clone())
    }

    fn number_label(&self, value: f64) -> String {
        if self.full_numbers {
            format_precise(value)
        } else {
            format_number(value)
        }
    }

    fn title_label<'a>(&'a self, data: &'a ChartData) -> &'a str {
        if self.title.trim().is_empty() {
            &data.title
        } else {
            self.title.trim()
        }
    }

    fn x_label<'a>(&'a self, data: &'a ChartData) -> &'a str {
        if self.x_title.trim().is_empty() {
            &data.x_name
        } else {
            self.x_title.trim()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DataKey {
    kind: ChartKind,
    x_column: Option<usize>,
    series: Vec<usize>,
    order: Vec<usize>,
    total_rows: usize,
}

pub(crate) struct ChartResponse {
    pub(crate) export_requested: Option<ChartExportFormat>,
}

#[derive(Debug, Clone)]
struct Datum {
    x: f64,
    y: f64,
    x_label: String,
}

#[derive(Debug, Clone)]
struct Series {
    column: usize,
    name: String,
    values: Vec<Datum>,
}

#[derive(Debug)]
struct ChartData {
    title: String,
    x_name: String,
    x_numeric: bool,
    series: Vec<Series>,
    shown_rows: usize,
    total_rows: usize,
    source_rows: usize,
    categories: Vec<(f64, String)>,
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    result: &QueryResult,
    row_order: &[usize],
    state: &mut ChartState,
) -> ChartResponse {
    state.refresh(result);
    let mut export_requested = None;
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        export_requested = show_chart_toolbar(ui, result, state, !row_order.is_empty());
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            state.repair(result);
            match state.data(result, row_order) {
                Some(data) => draw_chart(ui, &data, state),
                None => components::empty_state(
                    ui,
                    icons::diagram(),
                    if row_order.is_empty() {
                        "No rows to chart"
                    } else {
                        "Choose numeric data"
                    },
                    if row_order.is_empty() {
                        "Clear the data filter or run a query with rows"
                    } else {
                        "Select a numeric Y value. Donut charts need positive values."
                    },
                ),
            }
        });
    });
    ChartResponse { export_requested }
}

fn chart_menu_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: Option<egui::ImageSource<'_>>,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::TextStyle::Button.resolve(ui.style()),
        palette::TEXT(),
    );
    let icon_width = if icon.is_some() { 20.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + icon_width + 34.0, style::CONTROL_H),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    ui.painter().rect(
        rect,
        5.0,
        if response.hovered() || open {
            palette::SURFACE_HOVER()
        } else {
            palette::SURFACE()
        },
        egui::Stroke::new(
            1.0_f32,
            if response.has_focus() {
                palette::ACCENT()
            } else {
                palette::BORDER()
            },
        ),
        egui::StrokeKind::Outside,
    );
    if let Some(icon) = icon {
        egui::Image::new(icon)
            .fit_to_exact_size(egui::Vec2::splat(14.0))
            .tint(palette::TEXT())
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 14.0, rect.center().y),
                    egui::Vec2::splat(14.0),
                ),
            );
    }
    ui.painter().galley(
        egui::pos2(
            rect.left() + 8.0 + icon_width,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        palette::TEXT(),
    );
    let chevron = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        egui::Vec2::splat(12.0),
    );
    egui::Image::new(icons::chevron_down())
        .fit_to_exact_size(chevron.size())
        .tint(palette::TEXT_WEAK())
        .paint_at(ui, chevron);
    response
}

fn show_chart_toolbar(
    ui: &mut egui::Ui,
    result: &QueryResult,
    state: &mut ChartState,
    has_rows: bool,
) -> Option<ChartExportFormat> {
    let mut export_requested = None;
    let compact = ui.available_width() < 720.0;
    let chars = if compact { 18 } else { 28 };
    let x_name = state
        .x_column
        .and_then(|column| result.columns.get(column))
        .map_or("Row number", |column| column.name.as_str());
    let y_name = match state.series.as_slice() {
        [] => "Choose values".to_string(),
        [column] => result.columns[*column].name.clone(),
        columns => format!("{} values", columns.len()),
    };
    let frame = egui::Frame::new()
        .fill(palette::PANEL())
        .inner_margin(egui::Margin::symmetric(8, 5));
    let response = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let kind = chart_menu_button(ui, state.kind.label(), None);
            show_type_popup(ui, &kind, state);
            let x = chart_menu_button(ui, &format!("X: {}", shorten(x_name, chars)), None)
                .on_hover_text(x_name);
            show_x_popup(ui, &x, result, state);
            let y = chart_menu_button(ui, &format!("Y: {}", shorten(&y_name, chars)), None)
                .on_hover_text(&y_name);
            show_y_popup(ui, &y, result, state);
            let style = chart_menu_button(ui, "Style", Some(icons::settings()));
            show_style_popup(ui, &style, result, state);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = !state.series.is_empty() && has_rows;
                let export = ui
                    .add_enabled_ui(enabled, |ui| {
                        if compact {
                            components::soft_icon_button(ui, icons::save(), "Export", true)
                        } else {
                            chart_menu_button(ui, "Export", Some(icons::save()))
                        }
                    })
                    .inner;
                if enabled {
                    export_requested = show_export_popup(ui, &export);
                }
            });
        });
    });
    ui.painter().line_segment(
        [
            response.response.rect.left_top(),
            response.response.rect.right_top(),
        ],
        egui::Stroke::new(1.0_f32, palette::BORDER()),
    );
    export_requested
}

fn show_export_popup(ui: &egui::Ui, anchor: &egui::Response) -> Option<ChartExportFormat> {
    let mut selected = None;
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::TOP_END)
        .gap(6.0)
        .frame(popup_frame(ui))
        .show(|ui| {
            ui.set_width(230.0);
            components::style_menu_submenus(ui);
            for (format, label, hint) in [
                (ChartExportFormat::Svg, "SVG…", "Vector"),
                (ChartExportFormat::Png, "PNG…", "2560 × 1440"),
            ] {
                if components::menu_item(ui, label, Some(hint), true).clicked() {
                    selected = Some(format);
                    ui.close();
                }
            }
        });
    selected
}

fn show_style_popup(
    ui: &egui::Ui,
    anchor: &egui::Response,
    result: &QueryResult,
    state: &mut ChartState,
) {
    let menu_height = (ui.ctx().content_rect().height() - 70.0).clamp(160.0, 480.0);
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::TOP_START)
        .gap(6.0)
        .width(STYLE_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(STYLE_WIDTH - 12.0);
            components::style_menu_submenus(ui);
            egui::ScrollArea::vertical()
                .max_height(menu_height - 36.0)
                .show(ui, |ui| {
                    show_customization(ui, result, state);
                });
            ui.separator();
            if components::menu_item(ui, "Reset style", None, true).clicked() {
                reset_chart_style(state);
            }
        });
}

fn popup_frame(ui: &egui::Ui) -> egui::Frame {
    components::menu_popup_frame(ui.style())
}

fn show_type_popup(ui: &egui::Ui, anchor: &egui::Response, state: &mut ChartState) {
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::TOP_START)
        .align_alternatives(&[])
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 12.0);
            components::style_menu_submenus(ui);
            for kind in ChartKind::ALL {
                if components::menu_radio(ui, state.kind == kind, kind.menu_label()).clicked() {
                    state.kind = kind;
                    ui.close();
                }
            }
        });
}

fn show_y_popup(
    ui: &egui::Ui,
    anchor: &egui::Response,
    result: &QueryResult,
    state: &mut ChartState,
) {
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::TOP_START)
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 12.0);
            components::style_menu_submenus(ui);
            popup_title(ui, "Y values");
            components::text_input(
                ui,
                &mut state.y_search,
                "Search numeric columns…",
                ui.available_width(),
            );
            ui.add_space(6.0);
            let search = state.y_search.to_lowercase();
            let columns: Vec<_> = state
                .numeric_columns
                .iter()
                .copied()
                .filter(|column| {
                    Some(*column) != state.x_column
                        && result.columns[*column]
                            .name
                            .to_lowercase()
                            .contains(&search)
                })
                .collect();
            if columns.is_empty() {
                field_label(ui, "No matching numeric columns");
            }
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for column in columns {
                        ui.push_id(column, |ui| {
                            let mut selected = state.series.contains(&column);
                            let enabled = !selected || state.series.len() > 1;
                            let label = shorten(&result.columns[column].name, 29);
                            if ui
                                .add_enabled_ui(enabled, |ui| {
                                    components::menu_checkbox(ui, &mut selected, &label)
                                })
                                .inner
                                .on_hover_text(&result.columns[column].name)
                                .changed()
                            {
                                if selected {
                                    state.series.push(column);
                                } else {
                                    state.series.retain(|candidate| *candidate != column);
                                }
                            }
                        });
                    }
                });
            if state.kind == ChartKind::Donut && state.series.len() > 1 {
                field_label(ui, "Donut uses the first selected value");
            } else {
                field_label(ui, "Select multiple values to compare series");
            }
        });
}

fn show_x_popup(
    ui: &egui::Ui,
    anchor: &egui::Response,
    result: &QueryResult,
    state: &mut ChartState,
) {
    egui::Popup::menu(anchor)
        .align(egui::RectAlign::TOP_START)
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 12.0);
            components::style_menu_submenus(ui);
            popup_title(ui, "X axis");
            components::text_input(
                ui,
                &mut state.x_search,
                "Search columns…",
                ui.available_width(),
            );
            ui.add_space(6.0);
            if components::menu_radio(ui, state.x_column.is_none(), "Row number").clicked() {
                state.x_column = None;
                ui.close();
            }
            let search = state.x_search.to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (column, meta) in result
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, meta)| meta.name.to_lowercase().contains(&search))
                    {
                        ui.push_id(column, |ui| {
                            if components::menu_radio(
                                ui,
                                state.x_column == Some(column),
                                &shorten(&meta.name, 29),
                            )
                            .on_hover_text(format!("{} ({})", meta.name, meta.type_name))
                            .clicked()
                            {
                                state.x_column = Some(column);
                                ui.close();
                            }
                        });
                    }
                });
        });
}

fn show_customization(ui: &mut egui::Ui, result: &QueryResult, state: &mut ChartState) {
    if matches!(state.kind, ChartKind::Line | ChartKind::Area) {
        chart_stepper_row(ui, "Line width", &mut state.line_width, 1.0..=4.0, 0.2);
    }
    match state.kind {
        ChartKind::Bar | ChartKind::StackedBar => {
            chart_stepper_row(ui, "Bar width", &mut state.bar_width, 0.2..=0.95, 0.05);
        }
        ChartKind::Donut => {
            chart_stepper_row(ui, "Hole size", &mut state.donut_hole, 0.3..=0.8, 0.05);
        }
        ChartKind::Scatter => {
            chart_stepper_row(ui, "Point size", &mut state.point_radius, 1.5..=5.0, 0.5);
        }
        _ => {}
    }
    ui.separator();
    components::menu_submenu(ui, "Colors", |ui| {
        ui.set_width(230.0);
        for (palette, label) in [
            (ChartPalette::Ember, "Ember"),
            (ChartPalette::Theme, "App theme"),
            (ChartPalette::Ocean, "Ocean"),
            (ChartPalette::Sunset, "Sunset"),
        ] {
            if components::menu_radio(ui, state.palette == palette, label).clicked()
                && state.palette != palette
            {
                state.palette = palette;
                state.series_colors.clear();
            }
        }
        if state.kind != ChartKind::Donut {
            ui.separator();
            let colors = palette_colors(state.palette, crate::theme::current());
            for (index, column) in state.series.clone().into_iter().enumerate() {
                ui.push_id(column, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(components::MENU_ROW_H);
                        ui.add_space(8.0);
                        let mut color = state
                            .series_colors
                            .get(&column)
                            .copied()
                            .unwrap_or(colors[index % colors.len()]);
                        if egui::color_picker::color_edit_button_srgba(
                            ui,
                            &mut color,
                            egui::color_picker::Alpha::Opaque,
                        )
                        .changed()
                        {
                            state.series_colors.insert(column, color);
                        }
                        ui.add(egui::Label::new(&result.columns[column].name).truncate())
                            .on_hover_text(&result.columns[column].name);
                    });
                });
            }
        }
    });
    if matches!(state.kind, ChartKind::Line | ChartKind::Area) {
        components::menu_submenu(ui, "Curve", |ui| {
            ui.set_width(180.0);
            for (curve, label) in [
                (CurveStyle::Linear, "Straight"),
                (CurveStyle::Smooth, "Smooth"),
                (CurveStyle::Step, "Step"),
            ] {
                if components::menu_radio(ui, state.curve == curve, label).clicked() {
                    state.curve = curve;
                }
            }
        });
        components::menu_submenu(ui, "Stroke", |ui| {
            ui.set_width(180.0);
            for (dashed, label) in [(false, "Solid"), (true, "Dashed")] {
                if components::menu_radio(ui, state.dashed == dashed, label).clicked() {
                    state.dashed = dashed;
                }
            }
        });
    }
    if matches!(
        state.kind,
        ChartKind::Area | ChartKind::Bar | ChartKind::StackedBar
    ) {
        components::menu_submenu(ui, "Fill", |ui| {
            ui.set_width(180.0);
            for (fill, label) in [
                (FillStyle::Gradient, "Gradient"),
                (FillStyle::Solid, "Solid"),
            ] {
                if components::menu_radio(ui, state.fill == fill, label).clicked() {
                    state.fill = fill;
                }
            }
            if state.kind == ChartKind::Area {
                ui.separator();
                chart_stepper_row(ui, "Fill opacity", &mut state.fill_opacity, 0.1..=0.6, 0.05);
            }
        });
    }
    ui.separator();
    components::menu_checkbox(ui, &mut state.show_legend, "Legend");
    if state.kind != ChartKind::Donut {
        components::menu_checkbox(ui, &mut state.show_grid, "Grid");
    }
    components::menu_checkbox(ui, &mut state.show_values, "Value labels");
    if matches!(state.kind, ChartKind::Line | ChartKind::Area) {
        components::menu_checkbox(ui, &mut state.show_points, "Points");
    }
    components::menu_checkbox(ui, &mut state.full_numbers, "Full numbers");
    components::menu_checkbox(ui, &mut state.show_summary, "Statistics");
    ui.separator();
    components::menu_submenu(ui, "Titles", |ui| {
        ui.set_width(250.0);
        field_label(ui, "Chart title");
        components::text_input(
            ui,
            &mut state.title,
            "Use column names",
            ui.available_width(),
        );
        if state.kind != ChartKind::Donut {
            ui.add_space(6.0);
            field_label(ui, "X title");
            components::text_input(
                ui,
                &mut state.x_title,
                "Use column name",
                ui.available_width(),
            );
            ui.add_space(6.0);
            field_label(ui, "Y title");
            components::text_input(ui, &mut state.y_title, "Optional", ui.available_width());
        }
    });
    if state.kind != ChartKind::Donut {
        components::menu_submenu(ui, "Axis settings", |ui| {
            ui.set_width(250.0);
            if !matches!(state.kind, ChartKind::Bar | ChartKind::StackedBar) {
                components::menu_checkbox(ui, &mut state.start_at_zero, "Include zero");
            }
            if components::menu_checkbox(ui, &mut state.custom_y_range, "Custom Y range").changed()
                && state.custom_y_range
            {
                if let Some((_, Some(data))) = &state.cache {
                    let scale = chart_scale(data, state.kind, state.start_at_zero);
                    state.y_min = scale.y.min;
                    state.y_max = scale.y.max;
                }
            }
            if state.custom_y_range {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Min");
                    ui.add(egui::DragValue::new(&mut state.y_min).speed(1.0));
                    ui.label("Max");
                    ui.add(egui::DragValue::new(&mut state.y_max).speed(1.0));
                });
                if !valid_y_range(state) {
                    ui.colored_label(palette::WARNING(), "Max must be greater than min");
                }
            }
            if matches!(state.kind, ChartKind::Line | ChartKind::Area) {
                ui.separator();
                chart_stepper_row(ui, "Point size", &mut state.point_radius, 1.5..=5.0, 0.5);
            }
        });
    }
}

fn reset_chart_style(state: &mut ChartState) {
    let defaults = ChartState::default();
    let numeric_columns = std::mem::take(&mut state.numeric_columns);
    let cache = state.cache.take();
    *state = ChartState {
        kind: state.kind,
        x_column: state.x_column,
        series: std::mem::take(&mut state.series),
        numeric_columns,
        analyzed_shape: state.analyzed_shape,
        axes_initialized: true,
        cache,
        ..defaults
    };
}

fn chart_stepper_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
) {
    ui.horizontal(|ui| {
        ui.set_min_height(components::MENU_ROW_H);
        ui.add_space(34.0);
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if components::soft_icon_button(ui, icons::plus(), "Increase", *value < *range.end())
                .clicked()
            {
                *value = (*value + step).min(*range.end());
            }
            ui.add(
                egui::DragValue::new(value)
                    .speed(step as f64)
                    .range(range.clone())
                    .min_decimals(1)
                    .max_decimals(2),
            );
            if components::soft_icon_button(ui, icons::minus(), "Decrease", *value > *range.start())
                .clicked()
            {
                *value = (*value - step).max(*range.start());
            }
        });
    });
}

fn popup_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .strong()
            .color(palette::TEXT()),
    );
    ui.add_space(5.0);
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.5)
            .color(palette::TEXT_FAINT()),
    );
}

fn numeric_columns(result: &QueryResult) -> Vec<usize> {
    (0..result.column_count())
        .filter(|column| {
            let type_name = result.columns[*column].type_name.to_ascii_lowercase();
            let base = type_name
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .next()
                .unwrap_or("");
            if matches!(
                base,
                "tinyint"
                    | "smallint"
                    | "mediumint"
                    | "int"
                    | "int2"
                    | "int4"
                    | "int8"
                    | "integer"
                    | "bigint"
                    | "serial"
                    | "smallserial"
                    | "bigserial"
                    | "decimal"
                    | "numeric"
                    | "number"
                    | "real"
                    | "double"
                    | "float"
                    | "float4"
                    | "float8"
                    | "money"
            ) {
                return true;
            }
            let mut found = false;
            // Runtime sampling covers weak/empty metadata without rescanning a 100k-row result
            // every frame. A fresh result or streamed shape change invalidates the cache above.
            for row in result.rows.iter().take(512) {
                let Some(value) = row.get(*column) else {
                    continue;
                };
                if value.is_null() {
                    continue;
                }
                if number(value).is_none() {
                    return false;
                }
                found = true;
            }
            found
        })
        .collect()
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(value) => Some(*value as f64),
        Value::Float(value) if value.is_finite() => Some(*value),
        // NUMERIC/DECIMAL values are intentionally preserved as Text by the core layer.
        Value::Text(value) => value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite()),
        Value::Null | Value::Bool(_) | Value::Bytes(_) | Value::Float(_) => None,
    }
}

fn chart_rows(row_order: &[usize], kind: ChartKind) -> Vec<usize> {
    let categorical = matches!(
        kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
    );
    let limit = if categorical { MAX_BARS } else { MAX_POINTS };
    if row_order.len() <= limit || categorical {
        return row_order.iter().copied().take(limit).collect();
    }
    // Cover the whole filtered result, including the last row, with bounded render work.
    (0..limit)
        .map(|index| row_order[index * (row_order.len() - 1) / (limit - 1)])
        .collect()
}

fn build_data(result: &QueryResult, row_order: &[usize], state: &ChartState) -> Option<ChartData> {
    build_data_from_rows(
        result,
        &chart_rows(row_order, state.kind),
        row_order.len(),
        state,
    )
}

fn build_data_from_rows(
    result: &QueryResult,
    order: &[usize],
    total_rows: usize,
    state: &ChartState,
) -> Option<ChartData> {
    if state.series.is_empty() || order.is_empty() {
        return None;
    }
    let x_numeric = !matches!(
        state.kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
    ) && state
        .x_column
        .is_none_or(|column| state.numeric_columns.contains(&column));
    let x_name = state
        .x_column
        .and_then(|column| result.columns.get(column))
        .map_or_else(|| "Row number".to_string(), |column| column.name.clone());
    let categories: Vec<_> = order
        .iter()
        .enumerate()
        .filter_map(|(position, row_index)| {
            let row = result.rows.get(*row_index)?;
            let value = state.x_column.and_then(|column| row.get(column));
            let row_number = if order.len() < total_rows
                && !matches!(
                    state.kind,
                    ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
                ) {
                position * (total_rows - 1) / (order.len() - 1).max(1) + 1
            } else {
                position + 1
            };
            let x = if x_numeric {
                if state.x_column.is_some() {
                    value.and_then(number)?
                } else {
                    row_number as f64
                }
            } else {
                position as f64
            };
            let label = match value {
                Some(value) if value.is_null() => "(null)".to_string(),
                Some(value) if x_numeric => format_precise(number(value)?),
                Some(value) => value.display(),
                None => row_number.to_string(),
            };
            Some((*row_index, x, label))
        })
        .collect();
    let mut series = Vec::new();
    let selected_series = if state.kind == ChartKind::Donut {
        &state.series[..state.series.len().min(1)]
    } else {
        state.series.as_slice()
    };
    for column in selected_series {
        let Some(meta) = result.columns.get(*column) else {
            continue;
        };
        let values: Vec<_> = categories
            .iter()
            .filter_map(|(row_index, x, label)| {
                let y = result.rows.get(*row_index)?.get(*column).and_then(number)?;
                if state.kind == ChartKind::Donut && y <= 0.0 {
                    return None;
                }
                Some(Datum {
                    x: *x,
                    y,
                    x_label: label.clone(),
                })
            })
            .collect();
        if !values.is_empty() {
            series.push(Series {
                column: *column,
                name: meta.name.clone(),
                values,
            });
        }
    }
    if series.is_empty() {
        return None;
    }
    let title = format!(
        "{} by {}",
        series
            .iter()
            .map(|series| series.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        x_name
    );
    let shown_rows = if state.kind == ChartKind::Donut {
        series.first().map_or(0, |series| series.values.len())
    } else {
        categories.len()
    };
    Some(ChartData {
        title,
        x_name,
        x_numeric,
        series,
        shown_rows,
        total_rows,
        source_rows: order.len(),
        categories: categories
            .into_iter()
            .map(|(_, x, label)| (x, label))
            .collect(),
    })
}

fn chart_note(data: &ChartData, kind: ChartKind) -> String {
    if data.source_rows < data.total_rows {
        let detail = if matches!(kind, ChartKind::Line | ChartKind::Area | ChartKind::Scatter) {
            "sampled"
        } else {
            "first rows"
        };
        format!(
            "{} of {} rows ({detail})",
            format_precise(data.shown_rows as f64),
            format_precise(data.total_rows as f64)
        )
    } else if data.shown_rows < data.total_rows {
        format!(
            "{} of {} rows with {} values",
            data.shown_rows,
            data.total_rows,
            if kind == ChartKind::Donut {
                "positive"
            } else {
                "valid"
            }
        )
    } else {
        format!("{} rows · {}", data.shown_rows, kind.label())
    }
}

fn palette_colors(scheme: ChartPalette, theme: crate::theme::Theme) -> [egui::Color32; 6] {
    let rgb = egui::Color32::from_rgb;
    let colors = match scheme {
        ChartPalette::Ember => [
            rgb(255, 159, 85),
            rgb(255, 214, 112),
            rgb(117, 202, 195),
            rgb(175, 163, 244),
            rgb(239, 139, 175),
            rgb(142, 197, 137),
        ],
        ChartPalette::Theme => [
            theme.accent,
            theme.success,
            theme.warning,
            theme.danger,
            style::mix(theme.accent, theme.success, 0.52),
            style::mix(theme.warning, theme.danger, 0.46),
        ],
        ChartPalette::Ocean => [
            rgb(56, 162, 235),
            rgb(43, 196, 174),
            rgb(127, 137, 246),
            rgb(106, 202, 235),
            rgb(88, 185, 114),
            rgb(195, 147, 232),
        ],
        ChartPalette::Sunset => [
            rgb(247, 155, 72),
            rgb(233, 107, 141),
            rgb(180, 141, 237),
            rgb(240, 196, 88),
            rgb(91, 181, 190),
            rgb(211, 107, 93),
        ],
    };
    if scheme != ChartPalette::Theme
        && u32::from(theme.base.r()) + u32::from(theme.base.g()) + u32::from(theme.base.b()) > 600
    {
        colors.map(|color| style::mix(color, egui::Color32::BLACK, 0.28))
    } else {
        colors
    }
}

fn chart_colors(
    data: &ChartData,
    state: &ChartState,
    theme: crate::theme::Theme,
) -> Vec<egui::Color32> {
    let colors = palette_colors(state.palette, theme);
    if state.kind == ChartKind::Donut {
        return colors.to_vec();
    }
    data.series
        .iter()
        .enumerate()
        .map(|(index, series)| {
            if state
                .focused_series
                .is_some_and(|column| column != series.column)
            {
                return style::mix(theme.text_faint, theme.base, 0.5);
            }
            state
                .series_colors
                .get(&series.column)
                .copied()
                .unwrap_or(colors[index % colors.len()])
        })
        .collect()
}

fn valid_y_range(state: &ChartState) -> bool {
    state.y_min.is_finite() && state.y_max.is_finite() && state.y_min < state.y_max
}

fn configured_scale(data: &ChartData, state: &ChartState) -> ChartScale {
    let mut scale = chart_scale(data, state.kind, state.start_at_zero);
    if state.custom_y_range && valid_y_range(state) {
        scale.y = AxisScale {
            min: state.y_min,
            max: state.y_max,
            step: nice_axis(state.y_min, state.y_max, false).step,
        };
    }
    scale
}

#[derive(Clone, Copy)]
struct AxisScale {
    min: f64,
    max: f64,
    step: f64,
}

#[derive(Clone, Copy)]
struct ChartScale {
    x_min: f64,
    x_max: f64,
    y: AxisScale,
}

#[derive(Clone, Copy)]
struct SeriesSummary {
    min: f64,
    average: f64,
    max: f64,
}

type HoverValue = (String, f64, egui::Color32, egui::Pos2);

fn draw_chart(ui: &mut egui::Ui, data: &ChartData, state: &mut ChartState) {
    let kind = state.kind;
    let available = ui.available_size();
    let size = egui::vec2(available.x.max(0.0), available.y.max(0.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, state.title_label(data))
    });
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, palette::BASE());
    let title = shorten(
        state.title_label(data),
        ((rect.width() - 48.0) / 7.0).max(8.0) as usize,
    );
    let title_galley =
        painter.layout_no_wrap(title, egui::FontId::proportional(14.0), palette::TEXT());
    let title_width = title_galley.size().x;
    painter.galley(
        rect.left_top() + egui::vec2(24.0, 16.0),
        title_galley,
        palette::TEXT(),
    );
    ui.interact(
        egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(24.0, 16.0),
            egui::vec2(title_width, 20.0),
        ),
        ui.id().with("chart_title"),
        egui::Sense::hover(),
    )
    .on_hover_text(state.title_label(data));
    let mut plot_top = rect.top() + 58.0;
    if state.kind != ChartKind::Donut && state.show_legend {
        let items: Vec<_> = data
            .series
            .iter()
            .map(|series| {
                let galley = painter.layout_no_wrap(
                    shorten(&series.name, 24),
                    egui::FontId::proportional(10.5),
                    palette::TEXT_WEAK(),
                );
                let width = galley.size().x + 32.0;
                (galley, width)
            })
            .collect();
        let legend_width: f32 = items.iter().map(|(_, width)| width).sum();
        let inline = title_width + legend_width + 80.0 < rect.width();
        let mut x = if inline {
            rect.right() - 24.0 - legend_width
        } else {
            rect.left() + 24.0
        };
        let mut y = rect.top() + if inline { 18.0 } else { 43.0 };
        let colors = chart_colors(data, state, crate::theme::current());
        for (index, (galley, width)) in items.into_iter().enumerate() {
            if x + width > rect.right() - 24.0 {
                x = rect.left() + 24.0;
                y += 20.0;
            }
            if y > rect.top() + 83.0 {
                break;
            }
            let hit = egui::Rect::from_min_size(egui::pos2(x, y - 3.0), egui::vec2(width, 20.0));
            let response = ui.interact(
                hit,
                ui.id().with(("chart_legend", data.series[index].column)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    state.focused_series == Some(data.series[index].column),
                    &data.series[index].name,
                )
            });
            if response.clicked() {
                let column = data.series[index].column;
                state.focused_series = if state.focused_series == Some(column) {
                    None
                } else {
                    Some(column)
                };
            }
            response.on_hover_text("Click to focus this series; click again to show all");
            painter.circle_filled(egui::pos2(x + 4.0, y + 6.0), 3.0, colors[index]);
            painter.galley(egui::pos2(x + 14.0, y), galley, palette::TEXT_WEAK());
            x += width;
        }
        plot_top = plot_top.max(y + 27.0);
    }
    if data.shown_rows < data.total_rows || state.show_summary {
        if data.shown_rows < data.total_rows {
            painter.text(
                egui::pos2(rect.left() + 24.0, plot_top),
                egui::Align2::LEFT_TOP,
                chart_note(data, state.kind),
                egui::FontId::proportional(10.0),
                palette::TEXT_FAINT(),
            );
        }
        if state.show_summary && rect.width() > 690.0 {
            if let Some((_, summary)) = primary_summary(data) {
                painter.text(
                    egui::pos2(rect.right() - 24.0, plot_top),
                    egui::Align2::RIGHT_TOP,
                    format!(
                        "Min {}    Avg {}    Max {}",
                        state.number_label(summary.min),
                        state.number_label(summary.average),
                        state.number_label(summary.max)
                    ),
                    egui::FontId::proportional(10.0),
                    palette::TEXT_WEAK(),
                );
            }
        }
        plot_top += 24.0;
    }
    let colors = chart_colors(data, state, crate::theme::current());
    if kind == ChartKind::Donut {
        draw_donut(ui, &painter, rect, response, data, state, &colors);
        return;
    }
    let scale = configured_scale(data, state);
    let tick_width = axis_ticks(scale.y)
        .iter()
        .map(|value| {
            painter
                .layout_no_wrap(
                    state.number_label(*value),
                    egui::FontId::proportional(10.0),
                    palette::TEXT_FAINT(),
                )
                .size()
                .x
        })
        .fold(32.0_f32, f32::max);
    let left_margin = (tick_width + 22.0).max(68.0)
        + if state.y_title.trim().is_empty() {
            0.0
        } else {
            20.0
        };
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + left_margin, plot_top),
        rect.right_bottom() - egui::vec2(28.0, 58.0),
    );
    if plot.width() < 80.0 || plot.height() < 55.0 {
        return;
    }
    let map = |x: f64, y: f64| {
        egui::pos2(
            map_coordinate(x, scale.x_min, scale.x_max, plot.left(), plot.right()),
            map_coordinate(y, scale.y.min, scale.y.max, plot.bottom(), plot.top()),
        )
    };
    for value in axis_ticks(scale.y) {
        let y = map(scale.x_min, value).y;
        let is_zero = value.abs() < scale.y.step * 0.001;
        if state.show_grid || is_zero {
            let points = [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)];
            let stroke = egui::Stroke::new(
                0.8_f32,
                palette::BORDER_STRONG().gamma_multiply(if is_zero { 0.55 } else { 0.35 }),
            );
            if is_zero {
                painter.line_segment(points, stroke);
            } else {
                painter.extend(egui::Shape::dashed_line(&points, stroke, 3.0, 5.0));
            }
        }
        painter.text(
            egui::pos2(plot.left() - 9.0, y),
            egui::Align2::RIGHT_CENTER,
            state.number_label(value),
            egui::FontId::proportional(10.0),
            palette::TEXT_FAINT(),
        );
    }
    draw_x_axis(ui, &painter, plot, data, scale.x_min, scale.x_max, state);
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 13.0),
        egui::Align2::CENTER_BOTTOM,
        state.x_label(data),
        egui::FontId::proportional(11.0),
        palette::TEXT_WEAK(),
    );

    if !state.y_title.trim().is_empty() {
        let galley = painter.layout_no_wrap(
            state.y_title.trim().to_string(),
            egui::FontId::proportional(11.0),
            palette::TEXT_WEAK(),
        );
        let position = egui::pos2(rect.left() + 16.0, plot.center().y + galley.size().x / 2.0);
        painter.add(
            egui::epaint::TextShape::new(position, galley, palette::TEXT_WEAK())
                .with_angle(-std::f32::consts::FRAC_PI_2),
        );
    }
    let plot_painter = painter.with_clip_rect(plot.expand(1.0));
    if kind == ChartKind::StackedBar {
        draw_stacked_bars(&plot_painter, plot, data, scale, state, &colors);
    } else {
        for (series_index, series) in data.series.iter().enumerate() {
            let color = colors[series_index % colors.len()];
            match kind {
                ChartKind::Bar => {
                    let groups = data.shown_rows.max(1) as f32;
                    let group_width = (plot.width() / groups).clamp(3.0, 64.0) * state.bar_width;
                    let bar_width = group_width / data.series.len().max(1) as f32;
                    for datum in &series.values {
                        let center = map(datum.x, datum.y);
                        let base = map(datum.x, 0.0_f64.clamp(scale.y.min, scale.y.max));
                        let offset = (series_index as f32 - (data.series.len() - 1) as f32 / 2.0)
                            * bar_width;
                        let bar = egui::Rect::from_two_pos(
                            egui::pos2(center.x + offset - bar_width * 0.42, center.y),
                            egui::pos2(center.x + offset + bar_width * 0.42, base.y),
                        );
                        draw_bar(&plot_painter, bar, color, state, datum.y >= 0.0);
                        if state.show_values && data.shown_rows * data.series.len() <= 48 {
                            draw_value_label(
                                &plot_painter,
                                egui::pos2(center.x + offset, center.y),
                                datum.y,
                                datum.y >= 0.0,
                                state,
                            );
                        }
                    }
                }
                ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
                    let points: Vec<egui::Pos2> = series
                        .values
                        .iter()
                        .map(|datum| map(datum.x, datum.y))
                        .collect();
                    let curve = if kind == ChartKind::Scatter {
                        Vec::new()
                    } else {
                        curve_points(&points, state.curve)
                    };
                    if kind == ChartKind::Area && curve.len() > 1 {
                        let base_y = map(0.0, 0.0_f64.clamp(scale.y.min, scale.y.max)).y;
                        draw_area_fill(&plot_painter, &curve, base_y, color, state);
                    }
                    if matches!(kind, ChartKind::Line | ChartKind::Area) && curve.len() > 1 {
                        let stroke = egui::Stroke::new(state.line_width, color);
                        if state.dashed {
                            plot_painter.extend(egui::Shape::dashed_line(&curve, stroke, 6.0, 4.0));
                        } else {
                            plot_painter.add(egui::Shape::line(curve, stroke));
                        }
                    }
                    for (point, datum) in points.into_iter().zip(&series.values) {
                        if kind == ChartKind::Scatter
                            || (state.show_points && series.values.len() <= 200)
                        {
                            plot_painter.circle_filled(
                                point,
                                state.point_radius + 1.5,
                                palette::BASE(),
                            );
                            plot_painter.circle_filled(point, state.point_radius, color);
                        }
                        if state.show_values && data.shown_rows * data.series.len() <= 48 {
                            draw_value_label(&plot_painter, point, datum.y, true, state);
                        }
                    }
                }
                ChartKind::StackedBar | ChartKind::Donut => {}
            }
        }
    }

    let pointer = response
        .hovered()
        .then(|| ui.ctx().pointer_hover_pos())
        .flatten()
        .filter(|pointer| plot.contains(*pointer));
    if let Some((cursor_x, x_label, values)) =
        pointer.and_then(|pointer| hover_values(pointer.x, plot, data, scale, kind, &colors))
    {
        painter.line_segment(
            [
                egui::pos2(cursor_x, plot.top()),
                egui::pos2(cursor_x, plot.bottom()),
            ],
            egui::Stroke::new(1.0_f32, palette::TEXT_FAINT().gamma_multiply(0.72)),
        );
        for (_, _, color, point) in &values {
            if !plot.contains(*point) {
                continue;
            }
            painter.circle_filled(*point, 5.5, palette::BASE());
            painter.circle_stroke(*point, 5.5, egui::Stroke::new(2.0_f32, *color));
            painter.circle_filled(*point, 2.4, *color);
        }
        response.on_hover_ui_at_pointer(|ui| {
            ui.set_min_width(176.0);
            ui.label(egui::RichText::new(x_label).strong().color(palette::TEXT()));
            ui.add_space(3.0);
            for (name, value, color, _) in values {
                ui.horizontal(|ui| {
                    ui.colored_label(color, "●");
                    ui.label(egui::RichText::new(name).color(palette::TEXT_WEAK()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format_precise(value))
                                .monospace()
                                .color(palette::TEXT()),
                        );
                    });
                });
            }
        });
    }
}

fn curve_points(points: &[egui::Pos2], curve: CurveStyle) -> Vec<egui::Pos2> {
    if points.len() < 2 || curve == CurveStyle::Linear {
        return points.to_vec();
    }
    if curve == CurveStyle::Step {
        let mut output = vec![points[0]];
        for pair in points.windows(2) {
            output.push(egui::pos2(pair[1].x, pair[0].y));
            output.push(pair[1]);
        }
        return output;
    }
    if points.len() == 2 {
        return points.to_vec();
    }
    let direction = (points[1].x - points[0].x).signum();
    if direction == 0.0
        || points
            .windows(2)
            .any(|pair| (pair[1].x - pair[0].x).signum() != direction)
    {
        return points.to_vec();
    }
    // Monotone cubic interpolation: smooth lines never invent peaks between data points.
    let slopes: Vec<_> = points
        .windows(2)
        .map(|pair| (pair[1].y - pair[0].y) / (pair[1].x - pair[0].x))
        .collect();
    let mut tangents = vec![slopes[0]; points.len()];
    tangents[points.len() - 1] = *slopes.last().unwrap();
    for index in 1..points.len() - 1 {
        let (left, right) = (slopes[index - 1], slopes[index]);
        tangents[index] = if left * right <= 0.0 {
            0.0
        } else {
            2.0 / (1.0 / left + 1.0 / right)
        };
    }
    let mut output = vec![points[0]];
    for (index, pair) in points.windows(2).enumerate() {
        let dx = pair[1].x - pair[0].x;
        let steps = (dx.abs() / 10.0).ceil().clamp(1.0, 16.0) as usize;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * pair[0].y
                + (t3 - 2.0 * t2 + t) * dx * tangents[index]
                + (-2.0 * t3 + 3.0 * t2) * pair[1].y
                + (t3 - t2) * dx * tangents[index + 1];
            output.push(egui::pos2(
                pair[0].x + dx * t,
                y.clamp(pair[0].y.min(pair[1].y), pair[0].y.max(pair[1].y)),
            ));
        }
    }
    output
}

fn draw_area_fill(
    painter: &egui::Painter,
    points: &[egui::Pos2],
    baseline: f32,
    color: egui::Color32,
    state: &ChartState,
) {
    let mut mesh = egui::Mesh::default();
    let opacity = (state.fill_opacity * 255.0) as u8;
    let peak = points
        .iter()
        .map(|point| (baseline - point.y).abs())
        .fold(1.0_f32, f32::max);
    let top = translucent(color, opacity);
    let bottom = if state.fill == FillStyle::Gradient {
        translucent(color, 0)
    } else {
        top
    };
    for (index, point) in points.iter().enumerate() {
        let vertex_color = if state.fill == FillStyle::Gradient {
            translucent(
                color,
                (opacity as f32 * (baseline - point.y).abs() / peak) as u8,
            )
        } else {
            top
        };
        mesh.colored_vertex(*point, vertex_color);
        mesh.colored_vertex(egui::pos2(point.x, baseline), bottom);
        if index > 0 {
            let vertex = (index - 1) as u32 * 2;
            mesh.add_triangle(vertex, vertex + 1, vertex + 2);
            mesh.add_triangle(vertex + 1, vertex + 3, vertex + 2);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_bar(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    state: &ChartState,
    positive: bool,
) {
    if state.fill == FillStyle::Solid {
        painter.rect_filled(rect, 2.0, color);
        return;
    }
    let bright = translucent(color, 245);
    let faint = translucent(color, 65);
    let (top, bottom) = if positive {
        (bright, faint)
    } else {
        (faint, bright)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_value_label(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    value: f64,
    above: bool,
    state: &ChartState,
) {
    painter.text(
        anchor + egui::vec2(0.0, if above { -5.0 } else { 5.0 }),
        if above {
            egui::Align2::CENTER_BOTTOM
        } else {
            egui::Align2::CENTER_TOP
        },
        state.number_label(value),
        egui::FontId::proportional(10.0),
        palette::TEXT_WEAK(),
    );
}

fn draw_stacked_bars(
    painter: &egui::Painter,
    plot: egui::Rect,
    data: &ChartData,
    scale: ChartScale,
    state: &ChartState,
    colors: &[egui::Color32],
) {
    let width = (plot.width() / data.shown_rows.max(1) as f32).clamp(3.0, 64.0) * state.bar_width;
    for (anchor_x, _) in &data.categories {
        let x = map_coordinate(
            *anchor_x,
            scale.x_min,
            scale.x_max,
            plot.left(),
            plot.right(),
        );
        let mut positive = 0.0;
        let mut negative = 0.0;
        for (series_index, series) in data.series.iter().enumerate() {
            let Some(datum) = series
                .values
                .iter()
                .find(|datum| (datum.x - *anchor_x).abs() < f64::EPSILON)
            else {
                continue;
            };
            let (from, to) = if datum.y >= 0.0 {
                let from = positive;
                positive += datum.y;
                (from, positive)
            } else {
                let from = negative;
                negative += datum.y;
                (from, negative)
            };
            let y_from = map_coordinate(from, scale.y.min, scale.y.max, plot.bottom(), plot.top());
            let y_to = map_coordinate(to, scale.y.min, scale.y.max, plot.bottom(), plot.top());
            let bar = egui::Rect::from_two_pos(
                egui::pos2(x - width / 2.0, y_from),
                egui::pos2(x + width / 2.0, y_to),
            );
            draw_bar(
                painter,
                bar,
                colors[series_index % colors.len()],
                state,
                datum.y >= 0.0,
            );
            if state.show_values && data.shown_rows * data.series.len() <= 36 && bar.height() > 16.0
            {
                painter.text(
                    bar.center(),
                    egui::Align2::CENTER_CENTER,
                    state.number_label(datum.y),
                    egui::FontId::monospace(8.5),
                    palette::BASE(),
                );
            }
        }
    }
}

fn draw_donut(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    response: egui::Response,
    data: &ChartData,
    state: &ChartState,
    colors: &[egui::Color32],
) {
    let Some(series) = data.series.first() else {
        return;
    };
    let total: f64 = series.values.iter().map(|datum| datum.y).sum();
    if total <= 0.0 {
        return;
    }
    let body = egui::Rect::from_min_max(
        rect.left_top() + egui::vec2(24.0, 76.0),
        rect.right_bottom() - egui::vec2(24.0, 24.0),
    );
    let reserve_legend = state.show_legend && body.width() > 520.0;
    let chart_right = if reserve_legend {
        body.right() - body.width() * 0.34
    } else {
        body.right()
    };
    let bottom_legend = state.show_legend && !reserve_legend;
    let legend_height = if bottom_legend {
        (body.height() * 0.32).min(150.0)
    } else {
        0.0
    };
    let chart_bottom = body.bottom() - legend_height;
    let chart_rect = egui::Rect::from_min_max(body.min, egui::pos2(chart_right, chart_bottom));
    let center = chart_rect.center();
    let outer = chart_rect.width().min(chart_rect.height()) * 0.41;
    let inner = outer * state.donut_hole;
    let mut start = -std::f32::consts::FRAC_PI_2;
    let pointer = response
        .hovered()
        .then(|| ui.ctx().pointer_hover_pos())
        .flatten();
    let mut hovered = None;
    let mut mesh = egui::Mesh::default();
    let mut value_labels = Vec::new();

    for (index, datum) in series.values.iter().enumerate() {
        let sweep = std::f32::consts::TAU * (datum.y / total) as f32;
        let end = start + sweep;
        let steps = ((sweep.abs() * outer / 3.0).ceil() as usize).max(2);
        let color = colors[index % colors.len()].gamma_multiply(0.9);
        let first = mesh.vertices.len() as u32;
        for step in 0..=steps {
            let angle = egui::lerp(start..=end, step as f32 / steps as f32);
            let direction = egui::vec2(angle.cos(), angle.sin());
            mesh.colored_vertex(center + direction * inner, color);
            mesh.colored_vertex(center + direction * outer, color);
            if step < steps {
                let vertex = first + step as u32 * 2;
                mesh.add_triangle(vertex, vertex + 1, vertex + 2);
                mesh.add_triangle(vertex + 1, vertex + 3, vertex + 2);
            }
        }
        if let Some(pointer) = pointer {
            let delta = pointer - center;
            let radius = delta.length();
            let mut angle = delta.y.atan2(delta.x);
            if angle < -std::f32::consts::FRAC_PI_2 {
                angle += std::f32::consts::TAU;
            }
            if radius >= inner && radius <= outer && angle >= start && angle <= end {
                hovered = Some((index, datum));
            }
        }
        if state.show_values && datum.y / total >= 0.035 {
            let angle = (start + end) / 2.0;
            let label_pos = center + egui::vec2(angle.cos(), angle.sin()) * ((inner + outer) / 2.0);
            value_labels.push((label_pos, format!("{:.0}%", datum.y / total * 100.0)));
        }
        start = end;
    }

    painter.add(egui::Shape::mesh(mesh));
    for (position, label) in value_labels {
        painter.text(
            position,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(10.0),
            palette::BASE(),
        );
    }
    painter.circle_filled(center, inner - 1.0, palette::BASE());
    painter.text(
        center + egui::vec2(0.0, -4.0),
        egui::Align2::CENTER_BOTTOM,
        state.number_label(total),
        egui::FontId::proportional(19.0),
        palette::TEXT(),
    );
    painter.text(
        center + egui::vec2(0.0, 6.0),
        egui::Align2::CENTER_TOP,
        "Total",
        egui::FontId::monospace(9.0),
        palette::TEXT_FAINT(),
    );

    if state.show_legend {
        let legend = if reserve_legend {
            egui::Rect::from_min_max(egui::pos2(chart_right + 24.0, body.top() + 12.0), body.max)
        } else {
            egui::Rect::from_min_max(egui::pos2(body.left(), chart_bottom + 8.0), body.max)
        };
        let columns = if bottom_legend && legend.width() > 400.0 {
            2
        } else {
            1
        };
        let column_width = legend.width() / columns as f32;
        let rows = ((legend.height() - 12.0) / 25.0).floor().max(1.0) as usize;
        let limit = (rows * columns).min(12);
        for (index, datum) in series.values.iter().take(limit).enumerate() {
            let x = legend.left() + (index % columns) as f32 * column_width;
            let y = legend.top() + (index / columns) as f32 * 25.0;
            let value = format!(
                "{}  {:.1}%",
                state.number_label(datum.y),
                datum.y / total * 100.0
            );
            let value_galley = painter.layout_no_wrap(
                value,
                egui::FontId::proportional(10.0),
                palette::TEXT_FAINT(),
            );
            let chars = ((column_width - value_galley.size().x - 28.0) / 5.5).max(4.0) as usize;
            painter.circle_filled(
                egui::pos2(x + 4.0, y + 6.0),
                3.0,
                colors[index % colors.len()],
            );
            painter.text(
                egui::pos2(x + 14.0, y),
                egui::Align2::LEFT_TOP,
                shorten(&datum.x_label, chars),
                egui::FontId::proportional(11.0),
                palette::TEXT_WEAK(),
            );
            painter.galley(
                egui::pos2(x + column_width - value_galley.size().x - 8.0, y),
                value_galley,
                palette::TEXT_FAINT(),
            );
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(column_width, 22.0)),
                ui.id().with(("donut_legend", index)),
                egui::Sense::hover(),
            )
            .on_hover_text(&datum.x_label);
        }
    }

    if let Some((index, datum)) = hovered {
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(egui::RichText::new(&datum.x_label).strong());
            ui.horizontal(|ui| {
                ui.colored_label(colors[index % colors.len()], "●");
                ui.label(format!(
                    "{}  ({:.1}%)",
                    format_precise(datum.y),
                    datum.y / total * 100.0
                ));
            });
        });
    }
}

fn translucent(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn primary_summary(data: &ChartData) -> Option<(&Series, SeriesSummary)> {
    let series = data.series.first()?;
    let mut values = series.values.iter().map(|datum| datum.y);
    let first = values.next()?;
    let (min, max, sum, count) = values.fold(
        (first, first, first, 1_usize),
        |(min, max, sum, count), value| (min.min(value), max.max(value), sum + value, count + 1),
    );
    Some((
        series,
        SeriesSummary {
            min,
            average: sum / count as f64,
            max,
        },
    ))
}

fn hover_values(
    pointer_x: f32,
    plot: egui::Rect,
    data: &ChartData,
    scale: ChartScale,
    kind: ChartKind,
    colors: &[egui::Color32],
) -> Option<(f32, String, Vec<HoverValue>)> {
    let map_x = |x: f64| map_coordinate(x, scale.x_min, scale.x_max, plot.left(), plot.right());
    let anchor = data.categories.iter().min_by(|left, right| {
        (map_x(left.0) - pointer_x)
            .abs()
            .total_cmp(&(map_x(right.0) - pointer_x).abs())
    })?;
    let cursor_x = map_x(anchor.0);
    let mut values = Vec::with_capacity(data.series.len());
    for (index, series) in data.series.iter().enumerate() {
        let Some(datum) = series
            .values
            .iter()
            .find(|datum| (datum.x - anchor.0).abs() < f64::EPSILON)
        else {
            continue;
        };
        let display_y = if kind == ChartKind::StackedBar {
            data.series
                .iter()
                .take(index + 1)
                .filter_map(|candidate| {
                    candidate
                        .values
                        .iter()
                        .find(|value| (value.x - anchor.0).abs() < f64::EPSILON)
                        .map(|value| value.y)
                })
                .filter(|value| value.signum() == datum.y.signum())
                .sum()
        } else {
            datum.y
        };
        let y = map_coordinate(
            display_y,
            scale.y.min,
            scale.y.max,
            plot.bottom(),
            plot.top(),
        );
        values.push((
            series.name.clone(),
            datum.y,
            colors[index % colors.len()],
            egui::pos2(map_x(datum.x), y),
        ));
    }
    Some((cursor_x, anchor.1.clone(), values))
}

fn chart_scale(data: &ChartData, kind: ChartKind, start_at_zero: bool) -> ChartScale {
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for datum in data.series.iter().flat_map(|series| &series.values) {
        x_min = x_min.min(datum.x);
        x_max = x_max.max(datum.x);
        y_min = y_min.min(datum.y);
        y_max = y_max.max(datum.y);
    }
    if kind == ChartKind::StackedBar {
        {
            y_min = 0.0;
            y_max = 0.0;
            for (anchor_x, _) in &data.categories {
                let mut positive: f64 = 0.0;
                let mut negative: f64 = 0.0;
                for series in &data.series {
                    if let Some(datum) = series
                        .values
                        .iter()
                        .find(|datum| (datum.x - *anchor_x).abs() < f64::EPSILON)
                    {
                        if datum.y >= 0.0 {
                            positive += datum.y;
                        } else {
                            negative += datum.y;
                        }
                    }
                }
                y_min = y_min.min(negative);
                y_max = y_max.max(positive);
            }
        }
    }
    if matches!(kind, ChartKind::Bar | ChartKind::StackedBar) {
        x_min -= 0.6;
        x_max += 0.6;
    } else {
        let x_pad = ((x_max - x_min) * 0.025).max(f64::EPSILON);
        x_min -= x_pad;
        x_max += x_pad;
    }
    if (x_max - x_min).abs() < f64::EPSILON {
        x_min -= 1.0;
        x_max += 1.0;
    }
    ChartScale {
        x_min,
        x_max,
        y: nice_axis(
            y_min,
            y_max,
            start_at_zero || matches!(kind, ChartKind::Bar | ChartKind::StackedBar),
        ),
    }
}

fn nice_axis(mut min: f64, mut max: f64, include_zero: bool) -> AxisScale {
    if include_zero {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    if (max - min).abs() < f64::EPSILON {
        let pad = max.abs().max(1.0) * 0.1;
        min -= pad;
        max += pad;
    }
    let raw_step = (max - min) / 5.0;
    let magnitude = 10_f64.powf(raw_step.abs().log10().floor());
    let normalized = raw_step / magnitude;
    let nice = if normalized <= 1.5 {
        1.0
    } else if normalized <= 3.0 {
        2.0
    } else if normalized <= 4.0 {
        2.5
    } else if normalized <= 7.0 {
        5.0
    } else {
        10.0
    };
    let step = nice * magnitude;
    AxisScale {
        min: (min / step).floor() * step,
        max: (max / step).ceil() * step,
        step,
    }
}

fn axis_ticks(scale: AxisScale) -> Vec<f64> {
    let count = (((scale.max - scale.min) / scale.step).floor() as usize).min(12);
    (0..=count)
        .map(|index| scale.min + index as f64 * scale.step)
        .collect()
}

fn map_coordinate(value: f64, min: f64, max: f64, start: f32, end: f32) -> f32 {
    start + ((value - min) / (max - min)) as f32 * (end - start)
}

fn category_ticks(data: &ChartData, width: f32) -> Vec<(f64, &str)> {
    let count = data.categories.len();
    if count == 0 {
        return Vec::new();
    }
    let target = ((width / 110.0) as usize).max(2).min(count);
    if target == 1 {
        return vec![(data.categories[0].0, data.categories[0].1.as_str())];
    }
    (0..target)
        .map(|tick| {
            let index = tick * (count - 1) / (target - 1);
            (data.categories[index].0, data.categories[index].1.as_str())
        })
        .collect()
}

fn draw_x_axis(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    plot: egui::Rect,
    data: &ChartData,
    x_min: f64,
    x_max: f64,
    state: &ChartState,
) {
    if data.x_numeric && data.categories.len() > 8 {
        let count = ((plot.width() / 100.0) as usize).clamp(2, 8);
        for tick in 0..=count {
            let t = tick as f64 / count as f64;
            let x = map_coordinate(t, 0.0, 1.0, plot.left(), plot.right());
            painter.text(
                egui::pos2(x, plot.bottom() + 10.0),
                egui::Align2::CENTER_TOP,
                state.number_label(x_min + (x_max - x_min) * t),
                egui::FontId::proportional(10.0),
                palette::TEXT_WEAK(),
            );
        }
        return;
    }
    let ticks = category_ticks(data, plot.width());
    let positions: Vec<_> = ticks
        .iter()
        .map(|(x, _)| map_coordinate(*x, x_min, x_max, plot.left(), plot.right()))
        .collect();
    for (index, (_, label)) in ticks.iter().enumerate() {
        let x = positions[index];
        let slot_left = if index == 0 {
            plot.left()
        } else {
            (positions[index - 1] + x) / 2.0 + 5.0
        };
        let slot_right = if index + 1 == ticks.len() {
            plot.right()
        } else {
            (positions[index + 1] + x) / 2.0 - 5.0
        };
        let width = (slot_right - slot_left).clamp(12.0, 180.0);
        let mut job = egui::text::LayoutJob::simple(
            label.to_string(),
            egui::FontId::proportional(10.0),
            palette::TEXT_WEAK(),
            width,
        );
        job.wrap.max_rows = 2;
        let galley = painter.layout_job(job);
        let left = (x - galley.size().x / 2.0)
            .clamp(slot_left, (slot_right - galley.size().x).max(slot_left));
        let label_rect =
            egui::Rect::from_min_size(egui::pos2(left, plot.bottom() + 10.0), galley.size());
        painter.galley(label_rect.min, galley, palette::TEXT_WEAK());
        ui.interact(
            label_rect,
            ui.id().with(("chart_x_label", index)),
            egui::Sense::hover(),
        )
        .on_hover_text(*label);
    }
}

fn shorten(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value
            .chars()
            .take(max.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    }
}

fn format_number(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000.0 {
        format!("{}B", trim_decimal(value / 1_000_000_000.0, 1))
    } else if abs >= 1_000_000.0 {
        format!("{}M", trim_decimal(value / 1_000_000.0, 1))
    } else if abs >= 1_000.0 {
        format!("{}K", trim_decimal(value / 1_000.0, 1))
    } else if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        trim_decimal(value, 2)
    }
}

fn trim_decimal(value: f64, precision: usize) -> String {
    let text = format!("{value:.precision$}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn format_precise(value: f64) -> String {
    let text = if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        trim_decimal(value, 2)
    };
    let (sign, unsigned) = text
        .strip_prefix('-')
        .map_or(("", text.as_str()), |value| ("-", value));
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    let mut grouped = String::with_capacity(text.len() + text.len() / 3);
    grouped.push_str(sign);
    for (index, ch) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if let Some(fraction) = fraction {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}

pub(crate) fn to_svg(
    result: &QueryResult,
    row_order: &[usize],
    state: &ChartState,
    theme: crate::theme::Theme,
) -> Result<String, String> {
    let data = build_data(result, row_order, state)
        .ok_or_else(|| "No chart data. Check the selected values and data filters.".to_string())?;
    let plot_left = 92.0;
    let mut legend_items = Vec::new();
    if state.show_legend {
        let mut x = 40.0;
        let mut y = 105.0;
        for (index, series) in data.series.iter().enumerate() {
            let label = shorten(&series.name, 28);
            let width = 48.0 + label.chars().count() as f64 * 8.0;
            if x + width > 1240.0 && x > 40.0 {
                x = 40.0;
                y += 24.0;
            }
            if y > 153.0 {
                break;
            }
            legend_items.push((index, x, y, label));
            x += width;
        }
    }
    let plot_top = legend_items.last().map_or(112.0, |(_, _, y, _)| *y + 31.0);
    let plot_right = 1238.0;
    let plot_bottom = 632.0;
    let scale = configured_scale(&data, state);
    let sx = |x: f64| {
        plot_left + (x - scale.x_min) / (scale.x_max - scale.x_min) * (plot_right - plot_left)
    };
    let sy = |y: f64| {
        plot_bottom - (y - scale.y.min) / (scale.y.max - scale.y.min) * (plot_bottom - plot_top)
    };
    let color =
        |value: egui::Color32| format!("#{:02x}{:02x}{:02x}", value.r(), value.g(), value.b());
    let colors = chart_colors(&data, state, theme);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720" viewBox="0 0 1280 720" role="img" aria-labelledby="title desc">
<title id="title">{}</title><desc id="desc">{} chart exported from PlusPlus</desc>
<rect width="1280" height="720" rx="16" fill="{}"/>
<text x="40" y="46" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="24" font-weight="600">{}</text>
<text x="40" y="72" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{}</text>
"#,
        escape_xml(state.title_label(&data)),
        state.kind.label(),
        color(theme.base),
        color(theme.text),
        escape_xml(&shorten(state.title_label(&data), 92)),
        color(theme.text_faint),
        escape_xml(&chart_note(&data, state.kind))
    );
    if state.show_summary {
        if let Some((series, summary)) = primary_summary(&data) {
            svg.push_str(&format!(
            r#"<text x="1240" y="72" text-anchor="end" fill="{}" font-family="JetBrains Mono, monospace" font-size="12">{}   Min {}   Avg {}   Max {}</text>
"#,
            color(theme.text_weak),
            escape_xml(&shorten(&series.name, 18)),
            format_number(summary.min),
            format_number(summary.average),
            format_number(summary.max)
        ));
        }
    }
    svg.push_str("<defs>\n");
    for (index, series_color) in colors.iter().enumerate() {
        let hex = color(*series_color);
        svg.push_str(&format!(r#"<linearGradient id="area-{index}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="{hex}" stop-opacity="{}"/><stop offset="100%" stop-color="{hex}" stop-opacity="0"/></linearGradient>
<linearGradient id="bar-{index}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="{hex}" stop-opacity=".96"/><stop offset="100%" stop-color="{hex}" stop-opacity=".25"/></linearGradient>
<linearGradient id="negative-bar-{index}" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="{hex}" stop-opacity=".25"/><stop offset="100%" stop-color="{hex}" stop-opacity=".96"/></linearGradient>
"#, state.fill_opacity));
    }
    svg.push_str("</defs>\n");
    if state.kind == ChartKind::Donut {
        append_svg_donut(&mut svg, &data, state, &theme, &colors);
        svg.push_str("</svg>\n");
        return Ok(svg);
    }
    for (index, x, y, label) in &legend_items {
        let series_color = color(colors[*index % colors.len()]);
        svg.push_str(&format!(r#"<line x1="{x}" y1="{y}" x2="{}" y2="{y}" stroke="{series_color}" stroke-width="3" stroke-linecap="round"/><text x="{}" y="{}" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="13">{}</text>
"#, x + 18.0, x + 27.0, y + 4.0, color(theme.text_weak), escape_xml(label)));
    }
    for value in axis_ticks(scale.y) {
        let y = sy(value);
        if state.show_grid || value.abs() < scale.y.step * 0.001 {
            svg.push_str(&format!(
                r#"<line x1="{plot_left}" y1="{y:.1}" x2="{plot_right}" y2="{y:.1}" stroke="{}" stroke-width=".8" stroke-dasharray="3 5"/>
"#,
                color(theme.border)
            ));
        }
        svg.push_str(&format!(
            r#"<text x="80" y="{:.1}" text-anchor="end" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{}</text>
"#,
            y + 4.0,
            color(theme.text_faint),
            state.number_label(value)
        ));
    }
    if data.x_numeric && data.categories.len() > 8 {
        for tick in 0..=8 {
            let t = tick as f64 / 8.0;
            let x = plot_left + t * (plot_right - plot_left);
            let value = scale.x_min + t * (scale.x_max - scale.x_min);
            svg.push_str(&format!(
                r#"<text x="{x:.1}" y="662" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="11">{}</text>
"#, color(theme.text_weak), state.number_label(value)));
        }
    } else {
        let ticks = category_ticks(&data, (plot_right - plot_left) as f32);
        let max_chars =
            (((plot_right - plot_left) / ticks.len().max(1) as f64 - 12.0) / 6.0) as usize;
        for (x, label) in ticks {
            svg.push_str(&format!(
                r#"<text x="{:.1}" y="662" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="11"><title>{}</title>{}</text>
"#, sx(x), color(theme.text_weak), escape_xml(label), escape_xml(&shorten(label, max_chars))));
        }
    }
    if !state.y_title.trim().is_empty() {
        svg.push_str(&format!(r#"<text transform="translate(22 384) rotate(-90)" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="13">{}</text>
"#, color(theme.text_weak), escape_xml(state.y_title.trim())));
    }
    svg.push_str(&format!(r#"<defs><clipPath id="plot"><rect x="{plot_left}" y="{plot_top}" width="{}" height="{}"/></clipPath></defs><g clip-path="url(#plot)">
"#, plot_right - plot_left, plot_bottom - plot_top));

    if state.kind == ChartKind::StackedBar {
        let width = ((plot_right - plot_left) / data.shown_rows.max(1) as f64).clamp(3.0, 64.0)
            * state.bar_width as f64;
        {
            for (anchor_x, _) in &data.categories {
                let mut positive = 0.0;
                let mut negative = 0.0;
                for (series_index, series) in data.series.iter().enumerate() {
                    let Some(datum) = series
                        .values
                        .iter()
                        .find(|datum| (datum.x - *anchor_x).abs() < f64::EPSILON)
                    else {
                        continue;
                    };
                    let (from, to) = if datum.y >= 0.0 {
                        let from = positive;
                        positive += datum.y;
                        (from, positive)
                    } else {
                        let from = negative;
                        negative += datum.y;
                        (from, negative)
                    };
                    let y1 = sy(from);
                    let y2 = sy(to);
                    svg.push_str(&format!(
                        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="2" fill="{}" opacity=".9"/>
"#,
                        sx(*anchor_x) - width / 2.0,
                        y1.min(y2),
                        width,
                        (y1 - y2).abs().max(1.0),
                        svg_bar_fill(series_index, datum.y >= 0.0, colors[series_index % colors.len()], state)
                    ));
                    if state.show_values
                        && data.shown_rows * data.series.len() <= 36
                        && (y1 - y2).abs() > 18.0
                    {
                        svg.push_str(&format!(
                            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" dominant-baseline="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            sx(*anchor_x),
                            (y1 + y2) / 2.0,
                            color(theme.base),
                            state.number_label(datum.y)
                        ));
                    }
                }
            }
        }
    }

    for (series_index, series) in data.series.iter().enumerate() {
        let series_color = color(colors[series_index % colors.len()]);
        match state.kind {
            ChartKind::Bar => {
                let group = ((plot_right - plot_left) / data.shown_rows.max(1) as f64)
                    .clamp(3.0, 64.0)
                    * state.bar_width as f64;
                let bar_width = group / data.series.len().max(1) as f64;
                for datum in &series.values {
                    let center = sx(datum.x)
                        + (series_index as f64 - (data.series.len() - 1) as f64 / 2.0) * bar_width;
                    let y = sy(datum.y);
                    let base = sy(0.0_f64.clamp(scale.y.min, scale.y.max));
                    let bar_fill = svg_bar_fill(
                        series_index,
                        datum.y >= 0.0,
                        colors[series_index % colors.len()],
                        state,
                    );
                    svg.push_str(&format!(r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="2" fill="{}" opacity=".9"/>
"#, center - bar_width * 0.42, y.min(base), bar_width * 0.84, (base - y).abs().max(1.0), bar_fill));
                    if state.show_values && data.shown_rows * data.series.len() <= 48 {
                        svg.push_str(&format!(
                            r#"<text x="{center:.2}" y="{:.2}" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            y - 7.0,
                            color(theme.text_weak),
                            state.number_label(datum.y)
                        ));
                    }
                }
            }
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
                let raw_points: Vec<_> = series
                    .values
                    .iter()
                    .map(|datum| egui::pos2(sx(datum.x) as f32, sy(datum.y) as f32))
                    .collect();
                let points = curve_points(&raw_points, state.curve)
                    .iter()
                    .map(|point| format!("{:.2},{:.2}", point.x, point.y))
                    .collect::<Vec<_>>()
                    .join(" ");
                if state.kind == ChartKind::Area && series.values.len() > 1 {
                    let base = sy(0.0_f64.clamp(scale.y.min, scale.y.max));
                    let first_x = sx(series.values.first().map_or(0.0, |datum| datum.x));
                    let last_x = sx(series.values.last().map_or(0.0, |datum| datum.x));
                    let fill = if state.fill == FillStyle::Gradient {
                        format!("url(#area-{series_index})")
                    } else {
                        series_color.clone()
                    };
                    let opacity = if state.fill == FillStyle::Gradient {
                        1.0
                    } else {
                        state.fill_opacity
                    };
                    svg.push_str(&format!(r#"<polygon points="{first_x:.2},{base:.2} {points} {last_x:.2},{base:.2}" fill="{fill}" opacity="{opacity}"/>
"#));
                }
                if matches!(state.kind, ChartKind::Line | ChartKind::Area)
                    && series.values.len() > 1
                {
                    let dash = if state.dashed {
                        " stroke-dasharray=\"6 4\""
                    } else {
                        ""
                    };
                    svg.push_str(&format!(r#"<polyline points="{points}" fill="none" stroke="{series_color}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"{dash}/>
"#, state.line_width));
                }
                for datum in &series.values {
                    if state.kind == ChartKind::Scatter
                        || (state.show_points && series.values.len() <= 200)
                    {
                        svg.push_str(&format!(
                            r#"<circle cx="{:.2}" cy="{:.2}" r="{}" fill="{}"/>
"#,
                            sx(datum.x),
                            sy(datum.y),
                            state.point_radius,
                            series_color
                        ));
                    }
                    if state.show_values && data.shown_rows * data.series.len() <= 48 {
                        svg.push_str(&format!(
                            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            sx(datum.x),
                            sy(datum.y) - 8.0,
                            color(theme.text_weak),
                            state.number_label(datum.y)
                        ));
                    }
                }
            }
            ChartKind::StackedBar | ChartKind::Donut => {}
        }
    }
    svg.push_str("</g>\n");
    svg.push_str(&format!(r#"<text x="{}" y="706" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="13">{}</text>
</svg>
"#, (plot_left + plot_right) / 2.0, color(theme.text_weak), escape_xml(state.x_label(&data))));
    Ok(svg)
}

fn svg_bar_fill(index: usize, positive: bool, color: egui::Color32, state: &ChartState) -> String {
    if state.fill == FillStyle::Gradient {
        format!(
            "url(#{}bar-{index})",
            if positive { "" } else { "negative-" }
        )
    } else {
        format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
    }
}

fn append_svg_donut(
    svg: &mut String,
    data: &ChartData,
    state: &ChartState,
    theme: &crate::theme::Theme,
    colors: &[egui::Color32],
) {
    let Some(series) = data.series.first() else {
        return;
    };
    let total: f64 = series.values.iter().map(|datum| datum.y).sum();
    if total <= 0.0 {
        return;
    }
    let color =
        |value: egui::Color32| format!("#{:02x}{:02x}{:02x}", value.r(), value.g(), value.b());
    let center_x = if state.show_legend { 430.0 } else { 640.0 };
    let center_y = 390.0;
    let outer = 224.0;
    let inner = outer * state.donut_hole as f64;
    let radius = (outer + inner) / 2.0;
    let thickness = outer - inner;
    let circumference = std::f64::consts::TAU * radius;
    let mut offset = 0.0;
    for (index, datum) in series.values.iter().enumerate() {
        let length = circumference * datum.y / total;
        svg.push_str(&format!(
            r#"<circle cx="{center_x}" cy="{center_y}" r="{radius}" fill="none" stroke="{}" stroke-width="{thickness:.2}" stroke-dasharray="{:.2} {:.2}" stroke-dashoffset="-{offset:.2}" transform="rotate(-90 {center_x} {center_y})"/>
"#,
            color(colors[index % colors.len()]),
            (length - 2.0).max(0.0),
            circumference - (length - 2.0).max(0.0)
        ));
        if state.show_values && datum.y / total >= 0.035 {
            let angle = -std::f64::consts::FRAC_PI_2
                + (offset + length / 2.0) / circumference * std::f64::consts::TAU;
            let label_radius = radius;
            svg.push_str(&format!(
                r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" dominant-baseline="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{:.0}%</text>
"#,
                center_x + angle.cos() * label_radius,
                center_y + angle.sin() * label_radius,
                color(theme.base),
                datum.y / total * 100.0
            ));
        }
        offset += length;
    }
    svg.push_str(&format!(
        r#"<text x="{center_x}" y="{:.1}" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="28" font-weight="600">{}</text>
<text x="{center_x}" y="{:.1}" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">TOTAL</text>
"#,
        center_y - 3.0,
        color(theme.text),
        state.number_label(total),
        center_y + 22.0,
        color(theme.text_faint)
    ));
    if state.show_legend {
        let mut y = 218.0;
        for (index, datum) in series.values.iter().take(12).enumerate() {
            svg.push_str(&format!(
                r#"<circle cx="720" cy="{y}" r="5" fill="{}"/><text x="738" y="{:.1}" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="14">{}</text><text x="1210" y="{:.1}" text-anchor="end" fill="{}" font-family="JetBrains Mono, monospace" font-size="12">{} · {:.1}%</text>
"#,
                color(colors[index % colors.len()]),
                y + 5.0,
                color(theme.text_weak),
                escape_xml(&shorten(&datum.x_label, 28)),
                y + 4.0,
                color(theme.text_faint),
                state.number_label(datum.y),
                datum.y / total * 100.0
            ));
            y += 31.0;
        }
    }
}

pub(crate) fn svg_to_png(svg: &str) -> Result<Vec<u8>, String> {
    let mut options = resvg::usvg::Options {
        font_family: "IBM Plex Sans".into(),
        ..Default::default()
    };
    let fonts = options.fontdb_mut();
    for bytes in [
        include_bytes!("../../../app/assets/IBMPlexSans-Regular.ttf").as_slice(),
        include_bytes!("../../../app/assets/IBMPlexSans-SemiBold.ttf").as_slice(),
        include_bytes!("../../../app/assets/IBMPlexSansThai-Regular.ttf").as_slice(),
        include_bytes!("../../../app/assets/IBMPlexSansThai-SemiBold.ttf").as_slice(),
        include_bytes!("../../../app/assets/IBMPlexMono-Regular.ttf").as_slice(),
        include_bytes!("../../../app/assets/Unifont-Regular.otf").as_slice(),
    ] {
        fonts.load_font_data(bytes.to_vec());
    }
    fonts.set_sans_serif_family("IBM Plex Sans");
    fonts.set_monospace_family("IBM Plex Mono");
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &options)
        .map_err(|error| error.to_string())?;
    let mut image = resvg::tiny_skia::Pixmap::new(2560, 1440)
        .ok_or_else(|| "Could not allocate the chart image.".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
        &mut image.as_mut(),
    );
    image.encode_png().map_err(|error| error.to_string())
}

pub(crate) fn suggested_file_name(
    result: &QueryResult,
    state: &ChartState,
    format: ChartExportFormat,
) -> String {
    let stem = state
        .series
        .first()
        .and_then(|column| result.columns.get(*column))
        .map_or("query-chart", |column| column.name.as_str());
    let safe: String = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}.{}",
        if safe.is_empty() {
            "query-chart"
        } else {
            &safe
        },
        format.extension(),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::ColumnMeta;

    fn result() -> QueryResult {
        QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "month".into(),
                    type_name: "TEXT".into(),
                },
                ColumnMeta {
                    name: "revenue".into(),
                    type_name: "NUMERIC".into(),
                },
                ColumnMeta {
                    name: "orders".into(),
                    type_name: "INT".into(),
                },
            ],
            rows: vec![
                vec![
                    Value::Text("Jan".into()),
                    Value::Text("1250.50".into()),
                    Value::Int(8),
                ],
                vec![
                    Value::Text("Feb".into()),
                    Value::Text("1820.00".into()),
                    Value::Int(12),
                ],
            ],
            ..QueryResult::default()
        }
    }

    #[test]
    fn defaults_to_category_and_first_numeric_series() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        assert_eq!(state.x_column, Some(0));
        assert_eq!(state.series, vec![1]);
    }

    #[test]
    fn svg_escapes_query_column_names_and_contains_series() {
        let mut result = result();
        result.columns[1].name = "Revenue <net>".into();
        let mut state = ChartState::default();
        state.sync(&result);
        let svg = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(svg.contains("Revenue &lt;net&gt;"));
        assert!(svg.contains("<polyline"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn added_chart_types_export_their_distinct_shapes() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);

        state.kind = ChartKind::Area;
        let area = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(area.contains("<polygon"));
        assert!(area.contains("<polyline"));

        state.kind = ChartKind::StackedBar;
        state.series = vec![1, 2];
        let stacked = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(stacked.matches("<rect x=").count() >= 4);

        state.kind = ChartKind::Donut;
        let donut = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(donut.contains("stroke-dasharray"));
        assert!(donut.contains(">TOTAL</text>"));
        assert!(donut.contains(">Jan</text>"));
    }

    #[test]
    fn custom_title_and_appearance_are_reflected_in_svg() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.title = "Revenue & orders".into();
        state.show_grid = false;
        state.show_legend = false;
        state.show_values = true;
        let svg = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(svg.contains("Revenue &amp; orders"));
        assert!(!svg.contains("y1=\"105\""));
        assert!(svg.contains(">1.3K</text>"));
    }

    #[test]
    fn filtered_display_order_drives_chart_row_count() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        let data = build_data(&result, &[1], &state).unwrap();
        assert_eq!(data.total_rows, 1);
        assert_eq!(data.series[0].values[0].x_label, "Feb");
    }

    #[test]
    fn axis_uses_readable_round_ticks() {
        let scale = nice_axis(42_000.0, 88_600.0, false);
        assert_eq!(
            (scale.min, scale.max, scale.step),
            (40_000.0, 90_000.0, 10_000.0)
        );
        assert_eq!(axis_ticks(scale).len(), 6);

        let bars = nice_axis(318.0, 557.0, true);
        assert_eq!((bars.min, bars.max, bars.step), (0.0, 600.0, 100.0));
    }

    #[test]
    fn summary_and_tooltip_numbers_are_human_readable() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        let data = build_data(&result, &[0, 1], &state).unwrap();
        let (_, summary) = primary_summary(&data).unwrap();
        assert_eq!(summary.min, 1_250.5);
        assert_eq!(summary.average, 1_535.25);
        assert_eq!(summary.max, 1_820.0);
        assert_eq!(format_number(40_000.0), "40K");
        assert_eq!(format_precise(1_250.5), "1,250.5");
    }

    #[test]
    fn row_number_choice_survives_refresh_and_streaming() {
        let mut result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.x_column = None;
        state.refresh(&result);
        assert_eq!(state.x_column, None);
        result.rows.push(result.rows[0].clone());
        state.refresh(&result);
        assert_eq!(state.x_column, None);
        let data = build_data(&result, &[0, 1, 2], &state).unwrap();
        assert!(data.x_numeric);
        assert_eq!(data.categories[2], (3.0, "3".into()));
    }

    #[test]
    fn empty_filter_never_falls_back_to_all_rows() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        assert!(state.data(&result, &[]).is_none());
        assert!(to_svg(&result, &[], &state, crate::theme::current()).is_err());
    }

    #[test]
    fn large_charts_cover_the_full_display_order_with_bounded_work() {
        let order: Vec<_> = (0..10_000).rev().collect();
        let sampled = chart_rows(&order, ChartKind::Line);
        assert_eq!(sampled.len(), MAX_POINTS);
        assert_eq!(sampled.first(), order.first());
        assert_eq!(sampled.last(), order.last());
        assert!(sampled.windows(2).all(|pair| pair[0] > pair[1]));
        assert_eq!(chart_rows(&order, ChartKind::Bar), order[..MAX_BARS]);
    }

    #[test]
    fn appearance_reuses_data_but_axes_order_and_new_results_invalidate_it() {
        let mut result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        let first = state.data(&result, &[0, 1]).unwrap();
        state.line_width = 4.0;
        state.palette = ChartPalette::Ocean;
        state.title = "Custom title".into();
        assert!(Rc::ptr_eq(&first, &state.data(&result, &[0, 1]).unwrap()));
        let sorted = state.data(&result, &[1, 0]).unwrap();
        assert!(!Rc::ptr_eq(&first, &sorted));
        assert_eq!(sorted.categories[0].1, "Feb");
        state.x_column = None;
        let indexed = state.data(&result, &[1, 0]).unwrap();
        assert!(!Rc::ptr_eq(&sorted, &indexed));
        result.rows[1][1] = Value::Float(9_999.0);
        state.sync(&result);
        let updated = state.data(&result, &[1, 0]).unwrap();
        assert_eq!(updated.series[0].values[0].y, 9_999.0);
        assert!(!Rc::ptr_eq(&indexed, &updated));
    }

    #[test]
    fn stacked_bars_include_categories_missing_from_the_first_series() {
        let mut result = result();
        result.rows[1][1] = Value::Null;
        result.rows[1][2] = Value::Int(5_000);
        let mut state = ChartState::default();
        state.sync(&result);
        state.kind = ChartKind::StackedBar;
        state.series = vec![1, 2];
        let data = build_data(&result, &[0, 1], &state).unwrap();
        assert!(chart_scale(&data, state.kind, false).y.max >= 5_000.0);
        let svg = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert_eq!(svg.matches("rx=\"2\" fill=").count(), 3);
    }

    #[test]
    fn svg_preserves_custom_colors_axes_width_numbers_and_point_visibility() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state
            .series_colors
            .insert(1, egui::Color32::from_rgb(18, 52, 86));
        state.line_width = 4.0;
        state.show_points = false;
        state.show_legend = false;
        state.show_summary = false;
        state.full_numbers = true;
        state.x_title = "Period & month".into();
        state.y_title = "Revenue <net>".into();
        state.custom_y_range = true;
        state.y_min = 1_000.0;
        state.y_max = 2_000.0;
        let svg = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(svg.contains("stroke=\"#123456\" stroke-width=\"4\""));
        assert!(!svg.contains("<circle"));
        assert!(!svg.contains("   Avg "));
        assert!(svg.contains("Period &amp; month"));
        assert!(svg.contains("Revenue &lt;net&gt;"));
        assert!(svg.contains(">1,000</text>"));
        assert!(svg.contains("clip-path=\"url(#plot)\""));
        assert_eq!(
            configured_scale(&build_data(&result, &[0, 1], &state).unwrap(), &state)
                .y
                .min,
            1_000.0
        );
    }

    #[test]
    fn invalid_custom_range_uses_automatic_scale_and_large_x_values_stay_distinct() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.custom_y_range = true;
        state.y_min = 100.0;
        state.y_max = 0.0;
        let data = build_data(&result, &[0, 1], &state).unwrap();
        assert_eq!(
            configured_scale(&data, &state).y.max,
            chart_scale(&data, state.kind, false).y.max
        );
        assert_eq!(
            map_coordinate(1e12 + 1.0, 1e12, 1e12 + 2.0, 0.0, 100.0),
            50.0
        );
    }

    #[test]
    fn category_labels_fit_the_plot_and_include_the_last_category() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        let data = build_data(&result, &[0, 1], &state).unwrap();
        let ticks = category_ticks(&data, 100.0);
        assert_eq!(ticks, vec![(0.0, "Jan"), (1.0, "Feb")]);
    }

    #[test]
    fn smooth_curves_preserve_values_without_inventing_peaks() {
        let points = [
            egui::pos2(0.0, 100.0),
            egui::pos2(60.0, 20.0),
            egui::pos2(140.0, 80.0),
            egui::pos2(320.0, 40.0),
        ];
        let smooth = curve_points(&points, CurveStyle::Smooth);
        assert!(smooth.len() > points.len());
        for point in points {
            assert!(smooth.contains(&point));
        }
        for pair in points.windows(2) {
            for point in smooth
                .iter()
                .filter(|point| point.x >= pair[0].x && point.x <= pair[1].x)
            {
                assert!(point.y >= pair[0].y.min(pair[1].y) && point.y <= pair[0].y.max(pair[1].y));
            }
        }
        let reversed: Vec<_> = points.into_iter().rev().collect();
        assert!(curve_points(&reversed, CurveStyle::Smooth)
            .iter()
            .all(|point| point.x.is_finite() && point.y.is_finite()));
        assert_eq!(
            curve_points(&points[..2], CurveStyle::Step),
            vec![points[0], egui::pos2(points[1].x, points[0].y), points[1]]
        );
    }

    #[test]
    fn focusing_a_series_preserves_the_dataset_and_scale() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.series = vec![1, 2];
        let before = state.data(&result, &[0, 1]).unwrap();
        let original = chart_colors(&before, &state, crate::theme::current());
        state.focused_series = Some(1);
        let focused = state.data(&result, &[0, 1]).unwrap();
        assert!(Rc::ptr_eq(&before, &focused));
        let colors = chart_colors(&focused, &state, crate::theme::current());
        assert_eq!(colors[0], original[0]);
        assert_ne!(colors[1], original[1]);
        state.series = vec![2];
        state.repair(&result);
        assert_eq!(state.focused_series, None);
    }

    #[test]
    fn exported_area_and_bars_preserve_gradient_and_stroke_styles() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.kind = ChartKind::Area;
        state.dashed = true;
        let area = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(area.contains("fill=\"url(#area-0)\""));
        assert!(area.contains("stroke-dasharray=\"6 4\""));
        state.kind = ChartKind::Bar;
        let bars = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(bars.contains("fill=\"url(#bar-0)\""));
        state.fill = FillStyle::Solid;
        let solid = to_svg(&result, &[0, 1], &state, crate::theme::current()).unwrap();
        assert!(!solid.contains("fill=\"url(#bar-0)\""));
    }

    #[test]
    fn png_exports_decode_with_the_theme_and_custom_colors_for_every_chart_kind() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.series = vec![1, 2];
        state.fill = FillStyle::Solid;
        let color = egui::Color32::from_rgb(18, 122, 200);
        state.series_colors.insert(1, color);
        let theme = crate::theme::current();
        for kind in ChartKind::ALL {
            state.kind = kind;
            let expected = if kind == ChartKind::Donut {
                palette_colors(state.palette, theme)[0]
            } else {
                color
            };
            let svg = to_svg(&result, &[0, 1], &state, theme).unwrap();
            let png = svg_to_png(&svg).unwrap();
            let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            assert_eq!(image.dimensions(), (2560, 1440), "{kind:?}");
            assert_eq!(
                image.get_pixel(1280, 20).0,
                [theme.base.r(), theme.base.g(), theme.base.b(), 255]
            );
            assert!(
                image.enumerate_pixels().any(|(x, y, pixel)| {
                    x > 180
                        && y > 300
                        && y < 1260
                        && pixel.0[0].abs_diff(expected.r()) <= 24
                        && pixel.0[1].abs_diff(expected.g()) <= 24
                        && pixel.0[2].abs_diff(expected.b()) <= 24
                        && pixel.0[3] == 255
                }),
                "{kind:?} must contain the custom-colored chart"
            );
        }
    }

    #[test]
    fn png_export_renders_thai_text_and_rejects_invalid_svg() {
        let result = result();
        let mut state = ChartState::default();
        state.sync(&result);
        state.title = "ยอดขายรายเดือน".into();
        let theme = crate::theme::current();
        let svg = to_svg(&result, &[0, 1], &state, theme).unwrap();
        let image = image::load_from_memory(&svg_to_png(&svg).unwrap())
            .unwrap()
            .to_rgba8();
        let background = [theme.base.r(), theme.base.g(), theme.base.b(), 255];
        let title_pixels = (44..110)
            .flat_map(|y| (80..600).map(move |x| (x, y)))
            .filter(|&(x, y)| image.get_pixel(x, y).0 != background)
            .count();
        assert!(
            title_pixels > 100,
            "Thai title must remain visible in the PNG"
        );
        assert!(svg_to_png("invalid SVG").is_err());
    }

    #[test]
    fn chart_export_file_names_follow_the_selected_format() {
        let mut result = result();
        result.columns[1].name = "revenue / net".into();
        let mut state = ChartState::default();
        state.sync(&result);
        assert_eq!(
            suggested_file_name(&result, &state, ChartExportFormat::Svg),
            "revenue___net.svg"
        );
        assert_eq!(
            suggested_file_name(&result, &state, ChartExportFormat::Png),
            "revenue___net.png"
        );
    }

    #[test]
    fn chart_export_menu_requests_the_selected_format_in_wide_and_narrow_layouts() {
        use egui_kittest::kittest::Queryable as _;
        for width in [1100.0, 560.0] {
            let result = result();
            let mut state = ChartState::default();
            let requested = Rc::new(std::cell::Cell::new(None));
            let export = Rc::clone(&requested);
            let mut setup = false;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(width, 580.0))
                .build_ui(move |ui| {
                    if !setup {
                        crate::style::apply(ui.ctx());
                        egui_extras::install_image_loaders(ui.ctx());
                        setup = true;
                    }
                    if let Some(format) = show(ui, &result, &[0, 1], &mut state).export_requested {
                        export.set(Some(format));
                    }
                });
            harness.run_steps(3);
            for (format, label) in [
                (ChartExportFormat::Svg, "SVG…"),
                (ChartExportFormat::Png, "PNG…"),
            ] {
                harness.get_by_label("Export").click();
                harness.run_steps(3);
                assert!(harness.query_by_label("SVG…").is_some());
                assert!(harness.query_by_label("PNG…").is_some());
                harness.get_by_label(label).click();
                harness.run_steps(3);
                assert_eq!(requested.get(), Some(format));
                assert!(harness.query_by_label(label).is_none());
            }
        }
    }

    #[test]
    fn customization_and_column_pickers_are_usable_in_wide_and_narrow_layouts() {
        use egui_kittest::kittest::Queryable as _;
        for width in [1_100.0, 560.0] {
            let result = result();
            let state = Rc::new(std::cell::RefCell::new(ChartState::default()));
            let chart_state = Rc::clone(&state);
            let mut setup = false;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(width, 580.0))
                .build_ui(move |ui| {
                    if !setup {
                        crate::style::apply(ui.ctx());
                        egui_extras::install_image_loaders(ui.ctx());
                        setup = true;
                    }
                    show(ui, &result, &[0, 1], &mut chart_state.borrow_mut());
                });
            harness.run_steps(3);
            harness.get_by_label("Style").click();
            harness.run_steps(3);
            assert!(harness.query_by_label("Curve").is_some());
            assert!(harness.query_by_label("Legend").is_some());
            assert!(harness.query_all_by_label("Line width").next().is_some());
            harness.get_by_label("Legend").click();
            harness.run_steps(3);
            assert!(!state.borrow().show_legend);
            assert!(harness.query_by_label("Curve").is_some());
            harness.get_by_label("Curve").click();
            harness.run_steps(3);
            harness.get_by_label("Straight").click();
            harness.run_steps(3);
            assert_eq!(state.borrow().curve, CurveStyle::Linear);
            harness.get_by_label("Style").click();
            harness.run_steps(3);
            harness.get_by_label("X: month").click();
            harness.run_steps(3);
            assert!(harness.query_by_label("Row number").is_some());
            harness.get_by_label("Row number").click();
            harness.run_steps(3);
            assert_eq!(state.borrow().x_column, None);
            assert!(harness.query_by_label("Row number").is_none());
            harness.get_by_label("Y: revenue").click();
            harness.run_steps(3);
            harness.get_by_label("orders").click();
            harness.run_steps(3);
            assert_eq!(state.borrow().series, vec![1, 2]);
            assert!(harness.query_by_label("orders").is_some());
        }
    }

    fn render_chart_snapshot(kind: ChartKind, width: f32, popup: Option<&str>, name: &str) {
        let mut result = result();
        result.columns[0].name = "industry".into();
        result.rows = (0..8)
            .map(|index| {
                vec![
                    Value::Text(
                        [
                            "Agriculture and food",
                            "Consumer products",
                            "Financial services",
                            "Industrial products",
                            "Property and construction",
                            "Resources",
                            "Services",
                            "Technology",
                        ][index]
                            .into(),
                    ),
                    Value::Float(
                        [
                            42_000.0, 51_500.0, 49_200.0, 63_800.0, 71_200.0, 79_400.0, 74_300.0,
                            88_600.0,
                        ][index],
                    ),
                    Value::Int(index as i64 + 1),
                ]
            })
            .collect();
        let mut state = ChartState::default();
        state.sync(&result);
        state.kind = kind;
        state.title = "Revenue by industry".into();
        state.x_title = "Industry".into();
        let preview_theme =
            crate::theme::ThemeRegistry::load().theme_of(if name.ends_with("_light") {
                "daylight"
            } else {
                "carbon"
            });
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(width, 610.0))
            .with_pixels_per_point(2.0)
            .build_ui(move |ui| {
                if !setup {
                    crate::theme::set_current(preview_theme);
                    crate::style::apply(ui.ctx());
                    egui_extras::install_image_loaders(ui.ctx());
                    setup = true;
                }
                show(ui, &result, &(0..8).collect::<Vec<_>>(), &mut state);
            });
        harness.run_steps(6);
        if let Some(popup) = popup {
            use egui_kittest::kittest::Queryable as _;
            let position = harness.get_by_label(popup).rect().center() / 2.0;
            harness.hover_at(position);
            harness.run_steps(1);
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                });
                harness.run_steps(1);
            }
            harness.run_steps(6);
            let expected = match popup {
                "Style" => "Curve",
                "Line" => "Line chart",
                "X: industry" => "Row number",
                "Y: revenue" => "Y values",
                "Export" => "PNG…",
                _ => unreachable!(),
            };
            assert!(
                harness.query_by_label(expected).is_some(),
                "Dropdown must be visible in the preview"
            );
        }
        harness.snapshot(name);
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_customization() {
        render_chart_snapshot(
            ChartKind::Line,
            1_180.0,
            Some("Style"),
            "chart_customization",
        );
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_area() {
        render_chart_snapshot(ChartKind::Area, 1_180.0, None, "chart_area");
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_light() {
        render_chart_snapshot(ChartKind::Area, 1_180.0, None, "chart_light");
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_narrow() {
        render_chart_snapshot(ChartKind::Bar, 560.0, None, "chart_narrow");
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_donut() {
        render_chart_snapshot(ChartKind::Donut, 800.0, None, "chart_donut");
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_type_dropdown() {
        render_chart_snapshot(
            ChartKind::Line,
            1_180.0,
            Some("Line"),
            "chart_type_dropdown",
        );
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_x_dropdown() {
        render_chart_snapshot(
            ChartKind::Line,
            1_180.0,
            Some("X: industry"),
            "chart_x_dropdown",
        );
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_y_dropdown() {
        render_chart_snapshot(
            ChartKind::Line,
            1_180.0,
            Some("Y: revenue"),
            "chart_y_dropdown",
        );
    }

    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_chart_export_dropdown() {
        render_chart_snapshot(
            ChartKind::Line,
            1180.0,
            Some("Export"),
            "chart_export_dropdown",
        );
    }

    #[test]
    #[ignore = "export preview generator; run manually with --ignored"]
    fn snapshot_chart_png_export() {
        let mut result = result();
        result.columns[0].name = "เดือน".into();
        result.columns[1].name = "ยอดขาย".into();
        result.rows[0][0] = Value::Text("มกราคม".into());
        result.rows[1][0] = Value::Text("กุมภาพันธ์".into());
        let mut state = ChartState::default();
        state.sync(&result);
        state.kind = ChartKind::Area;
        state.title = "ยอดขายรายเดือน".into();
        state.x_title = "เดือน".into();
        state.y_title = "บาท".into();
        let svg = to_svg(
            &result,
            &[0, 1],
            &state,
            crate::theme::ThemeRegistry::load().theme_of("carbon"),
        )
        .unwrap();
        std::fs::write(
            "tests/snapshots/chart_export.png",
            svg_to_png(&svg).unwrap(),
        )
        .unwrap();
        std::fs::write("tests/snapshots/chart_export.svg", svg).unwrap();
    }
}
