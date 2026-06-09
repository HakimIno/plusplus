//! Custom unified title bar helpers (TablePlus-style).
//!
//! Helpers for the compact toolbar drawn into the native macOS titlebar space.

use egui::{self, CornerRadius, Margin, Rect, Stroke, Ui, UiBuilder};

use crate::style::palette;

#[cfg(target_os = "macos")]
const MAC_TRAFFIC_LIGHTS_INSET: f32 = 78.0;

/// Width reserved for the right-hand tool cluster.
const RIGHT_TOOLS_WIDTH: f32 = 132.0;
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

/// Connection path bar — full width, left-aligned. Drag/double-click only here (not on icons).
pub fn breadcrumb(ui: &mut Ui, text: &str) -> egui::Response {
    let pill_w = ui.available_width().max(80.0);
    let font = egui::FontId::proportional(10.0);

    let frame = egui::Frame::new()
        .fill(palette::SURFACE())
        .stroke(Stroke::new(1.0, palette::BORDER()))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 1));

    let inner = frame.show(ui, |ui| {
        ui.set_width(pill_w);
        ui.set_max_width(pill_w);
        ui.add(
            egui::Label::new(egui::RichText::new(text).font(font).color(palette::TEXT_WEAK()))
                .truncate()
                .selectable(false),
        )
        .on_hover_text(text);
    });

    // Window chrome only on the path bar — keeps toolbar icon clicks from moving the window.
    let chrome = ui.interact(
        inner.response.rect,
        inner.response.id.with("path_chrome"),
        egui::Sense::click_and_drag(),
    );
    handle_chrome_response(ui, &chrome);

    inner.response
}

/// Run `add_contents` inside one title-bar column (clipped so widgets never bleed).
pub fn column(ui: &mut Ui, rect: Rect, add_contents: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add_contents(ui);
    });
}
