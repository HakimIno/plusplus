//! Custom unified title bar helpers (TablePlus-style).
//!
//! Helpers for the compact toolbar drawn into the native macOS titlebar space.

use egui::{self, CornerRadius, Rect, Stroke, Ui, UiBuilder};

use crate::style::palette;

#[cfg(target_os = "macos")]
const MAC_TRAFFIC_LIGHTS_INSET: f32 = 78.0;

/// Width reserved for the right-hand tool cluster.
const RIGHT_TOOLS_WIDTH: f32 = 140.0;
/// Width of the left-hand icon cluster (excluding traffic-light inset).
const LEFT_TOOLS_WIDTH: f32 = 108.0;

/// Left inset to clear native macOS traffic lights when drawing into the titlebar space.
pub fn traffic_lights_inset(ctx: &egui::Context, frame: Option<&eframe::Frame>) -> f32 {
    #[cfg(target_os = "macos")]
    {
        let _ = frame;
        return MAC_TRAFFIC_LIGHTS_INSET / ctx.zoom_factor();
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ctx, frame);
        0.0
    }
}

fn toggle_zoom(ui: &Ui) {
    let maximized = ui
        .ctx()
        .input(|i| i.viewport().maximized.unwrap_or(false));
    ui.ctx()
        .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
}

fn handle_chrome_response(ui: &Ui, resp: &egui::Response) {
    if resp.double_clicked() {
        toggle_zoom(ui);
    } else if resp.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// Total height of the unified title bar.
pub fn height(chrome_inset: f32) -> f32 {
    #[cfg(target_os = "macos")]
    {
        let _ = chrome_inset;
        32.0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = chrome_inset;
        28.0
    }
}

/// Three fixed columns for a TablePlus-style title bar: tools | breadcrumb | tools.
pub struct BarColumns {
    pub left: Rect,
    pub center: Rect,
    pub right: Rect,
}

pub fn columns(bar: Rect, chrome_inset: f32) -> BarColumns {
    let left_w = chrome_inset + LEFT_TOOLS_WIDTH;
    let right_w = RIGHT_TOOLS_WIDTH;
    let left = Rect::from_min_max(bar.min, egui::pos2(bar.left() + left_w, bar.bottom()));
    let right = Rect::from_min_max(
        egui::pos2(bar.right() - right_w, bar.top()),
        bar.max,
    );
    let center = Rect::from_min_max(
        egui::pos2(left.right(), bar.top()),
        egui::pos2(right.left(), bar.bottom()),
    );
    BarColumns {
        left,
        center,
        right,
    }
}

/// Compact connection path pill — full width, short height, visibly rounded on every corner.
/// Drag/double-click only here (not on icons).
const BREADCRUMB_HEIGHT: f32 = 22.0;

pub fn breadcrumb(ui: &mut Ui, text: &str) -> egui::Response {
    let pill_w = ui.available_width().max(80.0);
    let font = egui::FontId::proportional(10.0);
    let text_color = palette::TEXT_WEAK();
    let radius = CornerRadius::same(6);

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(pill_w, BREADCRUMB_HEIGHT),
        egui::Sense::click_and_drag(),
    );

    if ui.is_rect_visible(rect) {
        // Inside stroke stays within the clip rect so corner radii are not squared off.
        ui.painter().rect(
            rect,
            radius,
            palette::SURFACE(),
            Stroke::new(1.0, palette::BORDER()),
            egui::StrokeKind::Inside,
        );

        let text_rect = rect.shrink2(egui::vec2(10.0, 0.0));
        ui.scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).font(font).color(text_color))
                            .truncate()
                            .selectable(false),
                    );
                },
            );
        });
    }

    handle_chrome_response(ui, &response);
    response.on_hover_text(text)
}

/// Run `add_contents` inside one title-bar column (clipped so widgets never bleed).
pub fn column(ui: &mut Ui, rect: Rect, add_contents: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add_contents(ui);
    });
}
