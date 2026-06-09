//! Custom unified title bar helpers (TablePlus-style).
//!
//! On macOS we extend content behind the native title bar via
//! [`egui::ViewportBuilder::with_fullsize_content_view`] and reserve space for the traffic
//! lights using [`eframe::WindowChromeMetrics`].

use egui::{self, CornerRadius, Margin, Rect, Stroke, Ui, UiBuilder};

use crate::style::palette;

/// Fallback inset when macOS chrome metrics are unavailable (e.g. headless tests).
#[cfg(target_os = "macos")]
const MAC_CHROME_FALLBACK: f32 = 78.0;

/// Width reserved for the right-hand tool cluster.
const RIGHT_TOOLS_WIDTH: f32 = 132.0;
/// Width of the left-hand icon cluster (excluding traffic-light inset).
const LEFT_TOOLS_WIDTH: f32 = 108.0;

/// Left inset to clear the macOS traffic lights, in egui points.
pub fn traffic_lights_inset(ctx: &egui::Context, frame: Option<&eframe::Frame>) -> f32 {
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::HasWindowHandle as _;

        if let Some(eframe) = frame {
            if let Some(window) = eframe.winit_window() {
                if let Ok(handle) = window.window_handle() {
                    if let Some(metrics) =
                        eframe::WindowChromeMetrics::from_window_handle(&handle.as_raw())
                    {
                        return metrics.traffic_lights_size.x / ctx.zoom_factor();
                    }
                }
            }
        }
        return MAC_CHROME_FALLBACK / ctx.zoom_factor();
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ctx, frame);
        0.0
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

/// One background interact for the title bar — drag to move, double-click to zoom.
///
/// Drawn *before* toolbar widgets so buttons stay clickable; empty chrome picks this up.
pub fn chrome_behind(ui: &mut Ui, bar: Rect, chrome_inset: f32) {
    let zone = Rect::from_min_max(
        egui::pos2(bar.left() + chrome_inset, bar.top()),
        bar.max,
    );
    let resp = ui.interact(
        zone,
        ui.id().with("title_chrome"),
        egui::Sense::click_and_drag(),
    );
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let maximized = ui
            .ctx()
            .input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// Connection path bar — full width of the centre column, left-aligned text.
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
    inner.response
}

/// Run `add_contents` inside one title-bar column (clipped so widgets never bleed).
pub fn column(ui: &mut Ui, rect: Rect, add_contents: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add_contents(ui);
    });
}
