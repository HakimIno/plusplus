//! Custom unified title bar helpers (TablePlus-style).
//!
//! Helpers for the compact toolbar drawn into the native macOS titlebar space.

use egui::{self, Color32, CornerRadius, Rect, Stroke, Ui, UiBuilder};

use crate::style::palette;

#[cfg(target_os = "macos")]
const MAC_TRAFFIC_LIGHTS_INSET: f32 = 78.0;

/// Horizontal breathing room between the side clusters and the centre breadcrumb.
const CLUSTER_GAP: f32 = 8.0;

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
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
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

/// Draw one title-bar cluster inside `rect` with the given flow `layout` and return the
/// space its widgets actually used. Side clusters size themselves from their contents,
/// so adding or removing a button never needs a width constant updated anywhere.
pub fn cluster(
    ui: &mut Ui,
    rect: Rect,
    layout: egui::Layout,
    add_contents: impl FnOnce(&mut Ui),
) -> Rect {
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), |ui| {
        ui.set_clip_rect(rect);
        add_contents(ui);
        ui.min_rect()
    })
    .inner
}

/// The space left for the centre breadcrumb once both measured side clusters are drawn.
/// Collapses to zero width (never inverts) when the window is extremely narrow.
pub fn center_rect(bar: Rect, left_used: Rect, right_used: Rect) -> Rect {
    let left_edge = left_used.right() + CLUSTER_GAP;
    let right_edge = (right_used.left() - CLUSTER_GAP).max(left_edge);
    Rect::from_min_max(
        egui::pos2(left_edge, bar.top()),
        egui::pos2(right_edge, bar.bottom()),
    )
}

/// Compact connection path pill — full width, short height, visibly rounded on every corner.
/// Drag/double-click only here (not on icons).
const BREADCRUMB_HEIGHT: f32 = 22.0;

pub fn breadcrumb(ui: &mut Ui, text: &str, fill: Option<Color32>) -> egui::Response {
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
            fill.unwrap_or_else(palette::SURFACE),
            Stroke::new(1.0, palette::BORDER()),
            egui::StrokeKind::Inside,
        );

        let text_rect = rect.shrink2(egui::vec2(10.0, 0.0));
        ui.scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(text).font(font).color(text_color))
                        .truncate()
                        .selectable(false),
                );
            });
        });
    }

    handle_chrome_response(ui, &response);
    response.on_hover_text(text)
}

