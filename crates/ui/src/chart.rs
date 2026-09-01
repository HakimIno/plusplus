//! Query-result charting and SVG export.
//!
//! The chart deliberately consumes the same materialized rows and display order as the grid:
//! filters and sorts therefore carry across when the user switches from Data to Chart. Rendering
//! stays dependency-free so the on-screen painter and exported SVG share the same data rules.

use dbcore::{QueryResult, Value};

use crate::components;
use crate::icons;
use crate::style::{self, palette};

const MAX_POINTS: usize = 2_000;
const MAX_BARS: usize = 120;
const POPOVER_WIDTH: f32 = 244.0;

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
    numeric_columns: Vec<usize>,
    analyzed_shape: (usize, usize),
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
            numeric_columns: Vec::new(),
            analyzed_shape: (0, 0),
        }
    }
}

impl ChartState {
    pub(crate) fn sync(&mut self, result: &QueryResult) {
        self.numeric_columns = numeric_columns(result);
        self.analyzed_shape = (result.row_count(), result.column_count());
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

        if self.series.is_empty() {
            self.series.extend(
                numeric
                    .iter()
                    .copied()
                    .filter(|column| Some(*column) != self.x_column)
                    .take(1),
            );
        }
        if self.x_column.is_none() {
            self.x_column = (0..result.column_count()).find(|column| !numeric.contains(column));
        }
    }
}

pub(crate) struct ChartResponse {
    pub(crate) export_requested: bool,
}

#[derive(Clone)]
struct Datum {
    x: f64,
    y: f64,
    x_label: String,
}

#[derive(Clone)]
struct Series {
    name: String,
    values: Vec<Datum>,
}

struct ChartData {
    title: String,
    x_name: String,
    x_numeric: bool,
    series: Vec<Series>,
    shown_rows: usize,
    total_rows: usize,
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    result: &QueryResult,
    row_order: &[usize],
    state: &mut ChartState,
) -> ChartResponse {
    state.refresh(result);
    let mut export_requested = false;
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        export_requested = show_chart_toolbar(ui, result, state);
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            state.repair(result);
            let data = build_data(result, row_order, state);
            match data {
                Some(data) => draw_chart(ui, &data, state),
                None => components::empty_state(
                    ui,
                    icons::diagram(),
                    "Choose numeric data",
                    "Choose an X axis and at least one numeric Y value",
                ),
            }
        });
    });

    ChartResponse { export_requested }
}

fn show_chart_toolbar(ui: &mut egui::Ui, result: &QueryResult, state: &mut ChartState) -> bool {
    let mut export_requested = false;
    let mut reset_requested = false;
    let x_name = state
        .x_column
        .and_then(|column| result.columns.get(column))
        .map_or("Row number", |column| column.name.as_str());
    let y_name = match state.series.as_slice() {
        [] => "Y axis".to_string(),
        [column] => result.columns[*column].name.clone(),
        columns => format!("{} values", columns.len()),
    };

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 5))
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0, palette::BORDER()))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let type_button = components::Btn::new(state.kind.label()).show(ui);
                let y_button = components::Btn::new(format!("Y · {y_name}")).show(ui);
                let x_button = components::Btn::new(format!("X · {x_name}")).show(ui);
                let style_button = components::Btn::new("Style").show(ui);

                show_type_popup(ui, &type_button, state);
                show_y_popup(ui, &y_button, result, state);
                show_x_popup(ui, &x_button, result, state);
                show_style_popup(ui, &style_button, state);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    export_requested = components::Btn::new("Export SVG…")
                        .icon(icons::save())
                        .enabled(!state.series.is_empty() && !result.rows.is_empty())
                        .tooltip("Export the current chart and appearance settings")
                        .show(ui)
                        .clicked();
                    reset_requested = components::Btn::new("Reset")
                        .tooltip("Reset chart type, axes, and style")
                        .show(ui)
                        .clicked();
                });
            });
        });

    if reset_requested {
        *state = ChartState::default();
        state.sync(result);
    }
    export_requested
}

fn popup_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::popup(ui.style())
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0, palette::BORDER_STRONG()))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
}

fn show_type_popup(ui: &egui::Ui, anchor: &egui::Response, state: &mut ChartState) {
    let popup_id = anchor.id.with("chart_type_popup");
    egui::Popup::from_toggle_button_response(anchor)
        .id(popup_id)
        .align(egui::RectAlign::TOP)
        .align_alternatives(&[])
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 20.0);
            popup_title(ui, "Chart type");
            for kind in ChartKind::ALL {
                let mut selected = state.kind == kind;
                ui.horizontal(|ui| {
                    if components::accent_checkbox(
                        ui,
                        !selected,
                        &mut selected,
                        Some(kind.menu_label()),
                    )
                    .changed()
                        && selected
                    {
                        state.kind = kind;
                    }
                });
            }
        });
}

fn show_y_popup(
    ui: &egui::Ui,
    anchor: &egui::Response,
    result: &QueryResult,
    state: &mut ChartState,
) {
    let popup_id = anchor.id.with("chart_y_popup");
    egui::Popup::from_toggle_button_response(anchor)
        .id(popup_id)
        .align(egui::RectAlign::TOP)
        .align_alternatives(&[])
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 20.0);
            popup_title(ui, "Y values");
            if state.numeric_columns.is_empty() {
                ui.label(
                    egui::RichText::new("No numeric columns in this result")
                        .size(11.0)
                        .color(palette::TEXT_FAINT()),
                );
            }
            for column in state.numeric_columns.clone() {
                if Some(column) == state.x_column {
                    continue;
                }
                let mut selected = state.series.contains(&column);
                let can_toggle = !selected || state.series.len() > 1;
                ui.horizontal(|ui| {
                    if components::accent_checkbox(ui, can_toggle, &mut selected, None).changed() {
                        if selected {
                            state.series.push(column);
                        } else if state.series.len() > 1 {
                            state.series.retain(|candidate| *candidate != column);
                        }
                    }
                    ui.label(&result.columns[column].name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(shorten(&result.columns[column].type_name, 10))
                                .monospace()
                                .size(9.0)
                                .color(palette::TEXT_FAINT()),
                        );
                    });
                });
            }
            if state.kind == ChartKind::Donut && state.series.len() > 1 {
                ui.label(
                    egui::RichText::new("Donut uses the first selected value")
                        .size(10.5)
                        .color(palette::WARNING()),
                );
            }
        });
}

fn show_x_popup(
    ui: &egui::Ui,
    anchor: &egui::Response,
    result: &QueryResult,
    state: &mut ChartState,
) {
    let popup_id = anchor.id.with("chart_x_popup");
    egui::Popup::from_toggle_button_response(anchor)
        .id(popup_id)
        .align(egui::RectAlign::TOP)
        .align_alternatives(&[])
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 20.0);
            popup_title(ui, "X axis");
            let mut row_number = state.x_column.is_none();
            ui.horizontal(|ui| {
                if components::accent_checkbox(ui, !row_number, &mut row_number, None).changed()
                    && row_number
                {
                    state.x_column = None;
                }
                ui.label("Row number");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("INDEX")
                            .monospace()
                            .size(9.0)
                            .color(palette::TEXT_FAINT()),
                    );
                });
            });
            for (column, meta) in result.columns.iter().enumerate() {
                let mut selected = state.x_column == Some(column);
                ui.horizontal(|ui| {
                    if components::accent_checkbox(ui, !selected, &mut selected, None).changed()
                        && selected
                    {
                        state.x_column = Some(column);
                    }
                    ui.label(&meta.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(shorten(&meta.type_name, 10))
                                .monospace()
                                .size(9.0)
                                .color(palette::TEXT_FAINT()),
                        );
                    });
                });
            }
        });
}

fn show_style_popup(ui: &egui::Ui, anchor: &egui::Response, state: &mut ChartState) {
    egui::Popup::from_toggle_button_response(anchor)
        .id(anchor.id.with("chart_style_popup"))
        .align(egui::RectAlign::TOP)
        .align_alternatives(&[])
        .gap(6.0)
        .width(POPOVER_WIDTH)
        .frame(popup_frame(ui))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(POPOVER_WIDTH - 20.0);
            popup_title(ui, "Style");
            field_label(ui, "Title");
            components::text_input(
                ui,
                &mut state.title,
                "Generated from fields",
                ui.available_width(),
            );
            ui.add_space(7.0);
            setting_toggle(ui, &mut state.show_legend, "Show legend");
            setting_toggle(ui, &mut state.show_values, "Show value labels");
            if state.kind != ChartKind::Donut {
                setting_toggle(ui, &mut state.show_grid, "Show grid lines");
                setting_toggle(ui, &mut state.start_at_zero, "Start Y axis at zero");
            }
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

fn setting_toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) {
    ui.horizontal(|ui| {
        components::accent_checkbox(ui, true, value, Some(label));
    });
    ui.add_space(3.0);
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

fn build_data(result: &QueryResult, row_order: &[usize], state: &ChartState) -> Option<ChartData> {
    if state.series.is_empty() || result.rows.is_empty() {
        return None;
    }
    let limit = if matches!(
        state.kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
    ) {
        MAX_BARS
    } else {
        MAX_POINTS
    };
    let order: Vec<usize> = if row_order.is_empty() {
        (0..result.rows.len()).take(limit).collect()
    } else {
        row_order.iter().copied().take(limit).collect()
    };
    let total_rows = if row_order.is_empty() {
        result.rows.len()
    } else {
        row_order.len()
    };
    let x_numeric = !matches!(
        state.kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
    ) && state.x_column.is_some_and(|column| {
        order.iter().all(|row| {
            result
                .rows
                .get(*row)
                .and_then(|values| values.get(column))
                .is_some_and(|value| value.is_null() || number(value).is_some())
        })
    });
    let x_name = state
        .x_column
        .and_then(|column| result.columns.get(column))
        .map_or_else(|| "Row number".to_string(), |column| column.name.clone());

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
        let mut values = Vec::new();
        for (position, row_index) in order.iter().enumerate() {
            let Some(row) = result.rows.get(*row_index) else {
                continue;
            };
            let Some(y) = row.get(*column).and_then(number) else {
                continue;
            };
            let x_value = state.x_column.and_then(|x| row.get(x));
            if state.kind == ChartKind::Donut && y <= 0.0 {
                continue;
            }
            let x = if matches!(
                state.kind,
                ChartKind::Bar | ChartKind::StackedBar | ChartKind::Donut
            ) || !x_numeric
            {
                position as f64
            } else {
                x_value.and_then(number).unwrap_or(position as f64)
            };
            let x_label = match x_value {
                Some(value) if !value.is_null() && x_numeric => {
                    number(value).map_or_else(|| value.display(), format_precise)
                }
                Some(value) if !value.is_null() => value.display(),
                _ => (position + 1).to_string(),
            };
            values.push(Datum { x, y, x_label });
        }
        if !values.is_empty() {
            series.push(Series {
                name: meta.name.clone(),
                values,
            });
        }
    }
    if series.is_empty() {
        return None;
    }
    let title = if state.title.trim().is_empty() {
        format!(
            "{} by {}",
            series
                .iter()
                .map(|series| series.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            x_name
        )
    } else {
        state.title.trim().to_string()
    };
    let shown_rows = if state.kind == ChartKind::Donut {
        series.first().map_or(0, |series| series.values.len())
    } else {
        order.len()
    };
    Some(ChartData {
        title,
        x_name,
        x_numeric,
        series,
        shown_rows,
        total_rows,
    })
}

fn series_colors() -> [egui::Color32; 6] {
    [
        palette::ACCENT(),
        palette::SUCCESS(),
        palette::WARNING(),
        palette::DANGER(),
        style::mix(palette::ACCENT(), palette::SUCCESS(), 0.52),
        style::mix(palette::WARNING(), palette::DANGER(), 0.46),
    ]
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

fn draw_chart(ui: &mut egui::Ui, data: &ChartData, state: &ChartState) {
    let kind = state.kind;
    let available = ui.available_size();
    let size = egui::vec2(available.x.max(360.0), available.y.max(260.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &data.title));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, palette::BASE());
    painter.rect_stroke(
        rect.shrink(0.5),
        8.0,
        egui::Stroke::new(1.0, palette::BORDER()),
        egui::StrokeKind::Inside,
    );

    let title = shorten(&data.title, if rect.width() > 800.0 { 72 } else { 42 });
    painter.text(
        rect.left_top() + egui::vec2(18.0, 16.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(17.0),
        palette::TEXT(),
    );
    let note = if data.shown_rows < data.total_rows {
        format!(
            "{} OF {} ROWS · {}",
            data.shown_rows,
            data.total_rows,
            kind.label().to_ascii_uppercase()
        )
    } else {
        format!(
            "{} POINTS · {}",
            data.shown_rows,
            kind.label().to_ascii_uppercase()
        )
    };
    painter.text(
        rect.left_top() + egui::vec2(18.0, 43.0),
        egui::Align2::LEFT_TOP,
        note,
        egui::FontId::monospace(9.5),
        palette::TEXT_FAINT(),
    );

    if rect.width() > 690.0 {
        if let Some((series, summary)) = primary_summary(data) {
            painter.text(
                rect.right_top() + egui::vec2(-18.0, 18.0),
                egui::Align2::RIGHT_TOP,
                format!(
                    "{}   MIN {}   AVG {}   MAX {}",
                    shorten(&series.name, 18),
                    format_number(summary.min),
                    format_number(summary.average),
                    format_number(summary.max),
                ),
                egui::FontId::monospace(10.5),
                palette::TEXT_WEAK(),
            );
        }
    }

    let colors = series_colors();
    if kind == ChartKind::Donut {
        draw_donut(ui, &painter, rect, response, data, state, &colors);
        return;
    }

    if state.show_legend {
        let mut legend_x = rect.left() + 18.0;
        let legend_y = rect.top() + 70.0;
        for (index, series) in data.series.iter().enumerate() {
            let color = colors[index % colors.len()];
            painter.line_segment(
                [
                    egui::pos2(legend_x, legend_y + 5.0),
                    egui::pos2(legend_x + 16.0, legend_y + 5.0),
                ],
                egui::Stroke::new(2.5, color),
            );
            painter.circle_filled(egui::pos2(legend_x + 8.0, legend_y + 5.0), 2.5, color);
            let galley = painter.layout_no_wrap(
                series.name.clone(),
                egui::FontId::proportional(11.0),
                palette::TEXT_WEAK(),
            );
            painter.galley(
                egui::pos2(legend_x + 23.0, legend_y),
                galley.clone(),
                palette::TEXT_WEAK(),
            );
            legend_x += galley.size().x + 43.0;
        }
    }

    let plot = egui::Rect::from_min_max(
        rect.left_top() + egui::vec2(68.0, if state.show_legend { 98.0 } else { 73.0 }),
        rect.right_bottom() - egui::vec2(24.0, 44.0),
    );
    if plot.width() < 80.0 || plot.height() < 80.0 {
        return;
    }
    let scale = chart_scale(data, kind, state.start_at_zero);
    let map = |x: f64, y: f64| {
        egui::pos2(
            egui::remap(
                x as f32,
                scale.x_min as f32..=scale.x_max as f32,
                plot.left()..=plot.right(),
            ),
            egui::remap(
                y as f32,
                scale.y.min as f32..=scale.y.max as f32,
                plot.bottom()..=plot.top(),
            ),
        )
    };

    for value in axis_ticks(scale.y) {
        let y = map(scale.x_min, value).y;
        let is_zero = value.abs() < scale.y.step * 0.001;
        if state.show_grid || is_zero {
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                egui::Stroke::new(
                    if is_zero { 1.2 } else { 1.0 },
                    if is_zero {
                        palette::BORDER_STRONG().gamma_multiply(0.82)
                    } else {
                        palette::BORDER().gamma_multiply(0.58)
                    },
                ),
            );
        }
        painter.text(
            egui::pos2(plot.left() - 9.0, y),
            egui::Align2::RIGHT_CENTER,
            format_number(value),
            egui::FontId::monospace(10.0),
            palette::TEXT_FAINT(),
        );
    }
    draw_x_axis(&painter, plot, data, scale.x_min, scale.x_max);
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 13.0),
        egui::Align2::CENTER_BOTTOM,
        &data.x_name,
        egui::FontId::proportional(11.0),
        palette::TEXT_WEAK(),
    );

    if kind == ChartKind::StackedBar {
        draw_stacked_bars(&painter, plot, data, scale, state.show_values, &colors);
    } else {
        for (series_index, series) in data.series.iter().enumerate() {
            let color = colors[series_index % colors.len()];
            match kind {
                ChartKind::Bar => {
                    let groups = data.shown_rows.max(1) as f32;
                    let group_width = (plot.width() / groups * 0.72).clamp(3.0, 42.0);
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
                        painter.rect_filled(bar, 3.0, color.gamma_multiply(0.84));
                        if state.show_values && data.shown_rows * data.series.len() <= 48 {
                            draw_value_label(&painter, bar.center_top(), datum.y, datum.y >= 0.0);
                        }
                    }
                }
                ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
                    let points: Vec<egui::Pos2> = series
                        .values
                        .iter()
                        .map(|datum| map(datum.x, datum.y))
                        .collect();
                    if kind == ChartKind::Area && points.len() > 1 {
                        let base_y = map(0.0, 0.0_f64.clamp(scale.y.min, scale.y.max)).y;
                        for pair in points.windows(2) {
                            painter.add(egui::Shape::convex_polygon(
                                vec![
                                    egui::pos2(pair[0].x, base_y),
                                    pair[0],
                                    pair[1],
                                    egui::pos2(pair[1].x, base_y),
                                ],
                                translucent(color, 42),
                                egui::Stroke::NONE,
                            ));
                        }
                    }
                    if matches!(kind, ChartKind::Line | ChartKind::Area) && points.len() > 1 {
                        painter.add(egui::Shape::line(
                            points.clone(),
                            egui::Stroke::new(6.0, translucent(color, 20)),
                        ));
                        painter.add(egui::Shape::line(
                            points.clone(),
                            egui::Stroke::new(2.25, color),
                        ));
                    }
                    for (point, datum) in points.into_iter().zip(&series.values) {
                        if kind == ChartKind::Scatter || series.values.len() <= 64 {
                            painter.circle_filled(
                                point,
                                if kind == ChartKind::Scatter { 3.8 } else { 2.4 },
                                color,
                            );
                        }
                        if state.show_values && data.shown_rows * data.series.len() <= 48 {
                            draw_value_label(&painter, point, datum.y, true);
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
            egui::Stroke::new(1.0, palette::TEXT_FAINT().gamma_multiply(0.72)),
        );
        for (_, _, color, point) in &values {
            painter.circle_filled(*point, 5.5, palette::BASE());
            painter.circle_stroke(*point, 5.5, egui::Stroke::new(2.0, *color));
            painter.circle_filled(*point, 2.4, *color);
        }
        response.on_hover_ui_at_pointer(|ui| {
            ui.set_min_width(176.0);
            ui.label(
                egui::RichText::new(format!("{}  {}", data.x_name, x_label))
                    .strong()
                    .color(palette::TEXT()),
            );
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

fn draw_value_label(painter: &egui::Painter, anchor: egui::Pos2, value: f64, above: bool) {
    painter.text(
        anchor + egui::vec2(0.0, if above { -5.0 } else { 5.0 }),
        if above {
            egui::Align2::CENTER_BOTTOM
        } else {
            egui::Align2::CENTER_TOP
        },
        format_number(value),
        egui::FontId::monospace(9.0),
        palette::TEXT_WEAK(),
    );
}

fn draw_stacked_bars(
    painter: &egui::Painter,
    plot: egui::Rect,
    data: &ChartData,
    scale: ChartScale,
    show_values: bool,
    colors: &[egui::Color32],
) {
    let Some(primary) = data.series.first() else {
        return;
    };
    let width = (plot.width() / data.shown_rows.max(1) as f32 * 0.64).clamp(3.0, 46.0);
    for anchor in &primary.values {
        let x = egui::remap(
            anchor.x as f32,
            scale.x_min as f32..=scale.x_max as f32,
            plot.left()..=plot.right(),
        );
        let mut positive = 0.0;
        let mut negative = 0.0;
        for (series_index, series) in data.series.iter().enumerate() {
            let Some(datum) = series
                .values
                .iter()
                .find(|datum| (datum.x - anchor.x).abs() < f64::EPSILON)
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
            let y_from = egui::remap(
                from as f32,
                scale.y.min as f32..=scale.y.max as f32,
                plot.bottom()..=plot.top(),
            );
            let y_to = egui::remap(
                to as f32,
                scale.y.min as f32..=scale.y.max as f32,
                plot.bottom()..=plot.top(),
            );
            let bar = egui::Rect::from_two_pos(
                egui::pos2(x - width / 2.0, y_from),
                egui::pos2(x + width / 2.0, y_to),
            );
            painter.rect_filled(
                bar,
                2.0,
                colors[series_index % colors.len()].gamma_multiply(0.86),
            );
            if show_values && data.shown_rows * data.series.len() <= 36 && bar.height() > 16.0 {
                painter.text(
                    bar.center(),
                    egui::Align2::CENTER_CENTER,
                    format_number(datum.y),
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
    let chart_rect = egui::Rect::from_min_max(body.min, egui::pos2(chart_right, body.bottom()));
    let center = chart_rect.center();
    let outer = chart_rect.width().min(chart_rect.height()) * 0.41;
    let inner = outer * 0.58;
    let mut start = -std::f32::consts::FRAC_PI_2;
    let pointer = response
        .hovered()
        .then(|| ui.ctx().pointer_hover_pos())
        .flatten();
    let mut hovered = None;

    for (index, datum) in series.values.iter().enumerate() {
        let sweep = std::f32::consts::TAU * (datum.y / total) as f32;
        let end = start + sweep;
        let steps = ((sweep.abs() * 18.0).ceil() as usize).max(2);
        let color = colors[index % colors.len()];
        for step in 0..steps {
            let a = egui::lerp(start..=end, step as f32 / steps as f32);
            let b = egui::lerp(start..=end, (step + 1) as f32 / steps as f32);
            let polar = |radius: f32, angle: f32| {
                center + egui::vec2(angle.cos() * radius, angle.sin() * radius)
            };
            painter.add(egui::Shape::convex_polygon(
                vec![
                    polar(inner, a),
                    polar(outer, a),
                    polar(outer, b),
                    polar(inner, b),
                ],
                color.gamma_multiply(0.9),
                egui::Stroke::NONE,
            ));
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
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                format!("{:.0}%", datum.y / total * 100.0),
                egui::FontId::monospace(9.0),
                palette::BASE(),
            );
        }
        start = end;
    }

    painter.circle_filled(center, inner - 1.0, palette::BASE());
    painter.text(
        center + egui::vec2(0.0, -4.0),
        egui::Align2::CENTER_BOTTOM,
        format_number(total),
        egui::FontId::proportional(19.0),
        palette::TEXT(),
    );
    painter.text(
        center + egui::vec2(0.0, 6.0),
        egui::Align2::CENTER_TOP,
        "TOTAL",
        egui::FontId::monospace(9.0),
        palette::TEXT_FAINT(),
    );

    if reserve_legend {
        let mut y = body.top() + 12.0;
        let x = chart_right + 34.0;
        for (index, datum) in series.values.iter().take(12).enumerate() {
            let color = colors[index % colors.len()];
            painter.circle_filled(egui::pos2(x, y + 6.0), 4.0, color);
            painter.text(
                egui::pos2(x + 14.0, y),
                egui::Align2::LEFT_TOP,
                shorten(&datum.x_label, 18),
                egui::FontId::proportional(11.0),
                palette::TEXT_WEAK(),
            );
            painter.text(
                egui::pos2(body.right(), y),
                egui::Align2::RIGHT_TOP,
                format!(
                    "{}  ·  {:.1}%",
                    format_number(datum.y),
                    datum.y / total * 100.0
                ),
                egui::FontId::monospace(10.0),
                palette::TEXT_FAINT(),
            );
            y += 25.0;
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
    let primary = data.series.first()?;
    let map_x = |x: f64| {
        egui::remap(
            x as f32,
            scale.x_min as f32..=scale.x_max as f32,
            plot.left()..=plot.right(),
        )
    };
    let anchor = primary.values.iter().min_by(|left, right| {
        (map_x(left.x) - pointer_x)
            .abs()
            .total_cmp(&(map_x(right.x) - pointer_x).abs())
    })?;
    let cursor_x = map_x(anchor.x);
    let mut values = Vec::with_capacity(data.series.len());
    for (index, series) in data.series.iter().enumerate() {
        let Some(datum) = series.values.iter().min_by(|left, right| {
            (left.x - anchor.x)
                .abs()
                .total_cmp(&(right.x - anchor.x).abs())
        }) else {
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
                        .find(|value| (value.x - anchor.x).abs() < f64::EPSILON)
                        .map(|value| value.y)
                })
                .filter(|value| value.signum() == datum.y.signum())
                .sum()
        } else {
            datum.y
        };
        let y = egui::remap(
            display_y as f32,
            scale.y.min as f32..=scale.y.max as f32,
            plot.bottom()..=plot.top(),
        );
        values.push((
            series.name.clone(),
            datum.y,
            colors[index % colors.len()],
            egui::pos2(map_x(datum.x), y),
        ));
    }
    Some((cursor_x, anchor.x_label.clone(), values))
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
        if let Some(primary) = data.series.first() {
            y_min = 0.0;
            y_max = 0.0;
            for anchor in &primary.values {
                let mut positive: f64 = 0.0;
                let mut negative: f64 = 0.0;
                for series in &data.series {
                    if let Some(datum) = series
                        .values
                        .iter()
                        .find(|datum| (datum.x - anchor.x).abs() < f64::EPSILON)
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
    let count = (((scale.max - scale.min) / scale.step).round() as usize).min(12);
    (0..=count)
        .map(|index| scale.min + index as f64 * scale.step)
        .collect()
}

fn draw_x_axis(
    painter: &egui::Painter,
    plot: egui::Rect,
    data: &ChartData,
    x_min: f64,
    x_max: f64,
) {
    if data.x_numeric
        && data
            .series
            .first()
            .is_some_and(|series| series.values.len() > 8)
    {
        for tick in 0..=5 {
            let t = tick as f32 / 5.0;
            let x = egui::lerp(plot.left()..=plot.right(), t);
            let value = x_min + (x_max - x_min) * tick as f64 / 5.0;
            painter.text(
                egui::pos2(x, plot.bottom() + 9.0),
                egui::Align2::CENTER_TOP,
                format_number(value),
                egui::FontId::monospace(10.0),
                palette::TEXT_FAINT(),
            );
        }
        return;
    }
    let Some(series) = data.series.first() else {
        return;
    };
    let count = series.values.len();
    let step = (count / 7).max(1);
    for (index, datum) in series.values.iter().enumerate().step_by(step) {
        let x = egui::remap(
            datum.x as f32,
            x_min as f32..=x_max as f32,
            plot.left()..=plot.right(),
        );
        let label = shorten(&datum.x_label, 12);
        painter.text(
            egui::pos2(x, plot.bottom() + 9.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(10.0),
            palette::TEXT_FAINT(),
        );
        if index + step >= count && index + 1 < count {
            break;
        }
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
        .ok_or_else(|| "Choose at least one numeric Y series.".to_string())?;
    let plot_left = 92.0;
    let plot_top = 136.0;
    let plot_right = 1238.0;
    let plot_bottom = 632.0;
    let scale = chart_scale(&data, state.kind, state.start_at_zero);
    let sx = |x: f64| {
        plot_left + (x - scale.x_min) / (scale.x_max - scale.x_min) * (plot_right - plot_left)
    };
    let sy = |y: f64| {
        plot_bottom - (y - scale.y.min) / (scale.y.max - scale.y.min) * (plot_bottom - plot_top)
    };
    let color =
        |value: egui::Color32| format!("#{:02x}{:02x}{:02x}", value.r(), value.g(), value.b());
    let colors = [
        theme.accent,
        theme.success,
        theme.warning,
        theme.danger,
        style::mix(theme.accent, theme.success, 0.52),
        style::mix(theme.warning, theme.danger, 0.46),
    ];

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720" viewBox="0 0 1280 720" role="img" aria-labelledby="title desc">
<title id="title">{}</title><desc id="desc">{} chart exported from PlusPlus</desc>
<rect width="1280" height="720" rx="16" fill="{}"/>
<text x="40" y="46" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="24" font-weight="600">{}</text>
<text x="40" y="72" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{} POINTS · {}</text>
"#,
        escape_xml(&data.title),
        state.kind.label(),
        color(theme.base),
        color(theme.text),
        escape_xml(&data.title),
        color(theme.text_faint),
        data.shown_rows,
        state.kind.label().to_ascii_uppercase()
    );
    if let Some((series, summary)) = primary_summary(&data) {
        svg.push_str(&format!(
            r#"<text x="1240" y="46" text-anchor="end" fill="{}" font-family="JetBrains Mono, monospace" font-size="12">{}   MIN {}   AVG {}   MAX {}</text>
"#,
            color(theme.text_weak),
            escape_xml(&shorten(&series.name, 18)),
            format_number(summary.min),
            format_number(summary.average),
            format_number(summary.max)
        ));
    }
    if state.kind == ChartKind::Donut {
        append_svg_donut(&mut svg, &data, state, &theme, &colors);
        svg.push_str("</svg>\n");
        return Ok(svg);
    }
    if state.show_legend {
        let mut legend_x = 40.0;
        for (index, series) in data.series.iter().enumerate() {
            let series_color = color(colors[index % colors.len()]);
            svg.push_str(&format!(
            r#"<line x1="{}" y1="105" x2="{}" y2="105" stroke="{}" stroke-width="3" stroke-linecap="round"/><circle cx="{}" cy="105" r="3" fill="{}"/><text x="{}" y="109" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="13">{}</text>
"#,
            legend_x,
            legend_x + 18.0,
            series_color,
            legend_x + 9.0,
            series_color,
            legend_x + 27.0,
            color(theme.text_weak),
            escape_xml(&series.name)
            ));
            legend_x += 48.0 + series.name.chars().count() as f64 * 8.0;
        }
    }
    for value in axis_ticks(scale.y) {
        let y = sy(value);
        if state.show_grid || value.abs() < scale.y.step * 0.001 {
            svg.push_str(&format!(
                r#"<line x1="{plot_left}" y1="{y:.1}" x2="{plot_right}" y2="{y:.1}" stroke="{}" stroke-width="1"/>
"#,
                color(theme.border)
            ));
        }
        svg.push_str(&format!(
            r#"<text x="80" y="{:.1}" text-anchor="end" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{}</text>
"#,
            y + 4.0,
            color(theme.text_faint),
            format_number(value)
        ));
    }
    if data.x_numeric
        && data
            .series
            .first()
            .is_some_and(|series| series.values.len() > 8)
    {
        for tick in 0..=5 {
            let t = tick as f64 / 5.0;
            let x = plot_left + t * (plot_right - plot_left);
            let value = scale.x_min + t * (scale.x_max - scale.x_min);
            svg.push_str(&format!(
                r#"<text x="{x:.1}" y="672" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="11">{}</text>
"#,
                color(theme.text_faint),
                format_number(value)
            ));
        }
    } else if let Some(series) = data.series.first() {
        let step = (series.values.len() / 7).max(1);
        for datum in series.values.iter().step_by(step) {
            svg.push_str(&format!(
                r#"<text x="{:.1}" y="672" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="11">{}</text>
"#,
                sx(datum.x),
                color(theme.text_faint),
                escape_xml(&shorten(&datum.x_label, 12))
            ));
        }
    }

    if state.kind == ChartKind::StackedBar {
        let width =
            ((plot_right - plot_left) / data.shown_rows.max(1) as f64 * 0.64).clamp(3.0, 46.0);
        if let Some(primary) = data.series.first() {
            for anchor in &primary.values {
                let mut positive = 0.0;
                let mut negative = 0.0;
                for (series_index, series) in data.series.iter().enumerate() {
                    let Some(datum) = series
                        .values
                        .iter()
                        .find(|datum| (datum.x - anchor.x).abs() < f64::EPSILON)
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
                        sx(anchor.x) - width / 2.0,
                        y1.min(y2),
                        width,
                        (y1 - y2).abs().max(1.0),
                        color(colors[series_index % colors.len()])
                    ));
                    if state.show_values
                        && data.shown_rows * data.series.len() <= 36
                        && (y1 - y2).abs() > 18.0
                    {
                        svg.push_str(&format!(
                            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" dominant-baseline="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            sx(anchor.x),
                            (y1 + y2) / 2.0,
                            color(theme.base),
                            format_number(datum.y)
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
                let group = ((plot_right - plot_left) / data.shown_rows.max(1) as f64 * 0.72)
                    .clamp(3.0, 42.0);
                let bar_width = group / data.series.len().max(1) as f64;
                for datum in &series.values {
                    let center = sx(datum.x)
                        + (series_index as f64 - (data.series.len() - 1) as f64 / 2.0) * bar_width;
                    let y = sy(datum.y);
                    let base = sy(0.0_f64.clamp(scale.y.min, scale.y.max));
                    svg.push_str(&format!(r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="2" fill="{}" opacity=".9"/>
"#, center - bar_width * 0.42, y.min(base), bar_width * 0.84, (base - y).abs().max(1.0), series_color));
                    if state.show_values && data.shown_rows * data.series.len() <= 48 {
                        svg.push_str(&format!(
                            r#"<text x="{center:.2}" y="{:.2}" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            y - 7.0,
                            color(theme.text_weak),
                            format_number(datum.y)
                        ));
                    }
                }
            }
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter => {
                let points = series
                    .values
                    .iter()
                    .map(|datum| format!("{:.2},{:.2}", sx(datum.x), sy(datum.y)))
                    .collect::<Vec<_>>()
                    .join(" ");
                if state.kind == ChartKind::Area && series.values.len() > 1 {
                    let base = sy(0.0_f64.clamp(scale.y.min, scale.y.max));
                    let first_x = sx(series.values.first().map_or(0.0, |datum| datum.x));
                    let last_x = sx(series.values.last().map_or(0.0, |datum| datum.x));
                    svg.push_str(&format!(
                        r#"<polygon points="{first_x:.2},{base:.2} {points} {last_x:.2},{base:.2}" fill="{series_color}" opacity=".18"/>
"#
                    ));
                }
                if matches!(state.kind, ChartKind::Line | ChartKind::Area)
                    && series.values.len() > 1
                {
                    svg.push_str(&format!(r#"<polyline points="{points}" fill="none" stroke="{series_color}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
"#));
                }
                for datum in &series.values {
                    svg.push_str(&format!(
                        r#"<circle cx="{:.2}" cy="{:.2}" r="{}" fill="{}"/>
"#,
                        sx(datum.x),
                        sy(datum.y),
                        if state.kind == ChartKind::Scatter {
                            4.5
                        } else {
                            3.0
                        },
                        series_color
                    ));
                    if state.show_values && data.shown_rows * data.series.len() <= 48 {
                        svg.push_str(&format!(
                            r#"<text x="{:.2}" y="{:.2}" text-anchor="middle" fill="{}" font-family="JetBrains Mono, monospace" font-size="10">{}</text>
"#,
                            sx(datum.x),
                            sy(datum.y) - 8.0,
                            color(theme.text_weak),
                            format_number(datum.y)
                        ));
                    }
                }
            }
            ChartKind::StackedBar | ChartKind::Donut => {}
        }
    }
    svg.push_str(&format!(r#"<text x="{}" y="706" text-anchor="middle" fill="{}" font-family="Inter, system-ui, sans-serif" font-size="13">{}</text>
</svg>
"#, (plot_left + plot_right) / 2.0, color(theme.text_weak), escape_xml(&data.x_name)));
    Ok(svg)
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
    let radius = 168.0;
    let circumference = std::f64::consts::TAU * radius;
    let mut offset = 0.0;
    for (index, datum) in series.values.iter().enumerate() {
        let length = circumference * datum.y / total;
        svg.push_str(&format!(
            r#"<circle cx="{center_x}" cy="{center_y}" r="{radius}" fill="none" stroke="{}" stroke-width="112" stroke-dasharray="{:.2} {:.2}" stroke-dashoffset="-{offset:.2}" transform="rotate(-90 {center_x} {center_y})"/>
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
        format_number(total),
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
                format_number(datum.y),
                datum.y / total * 100.0
            ));
            y += 31.0;
        }
    }
}

pub(crate) fn suggested_file_name(result: &QueryResult, state: &ChartState) -> String {
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
        "{}.svg",
        if safe.is_empty() {
            "query-chart"
        } else {
            &safe
        }
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
}
