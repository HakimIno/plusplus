//! Custom unified title bar helpers (TablePlus-style).
//!
//! Helpers for the compact toolbar drawn into the native macOS titlebar space.

use egui::{self, Color32, CornerRadius, Rect, Ui, UiBuilder};

use crate::style::palette;

#[cfg(target_os = "macos")]
const MAC_TRAFFIC_LIGHTS_INSET: f32 = 78.0;

/// Horizontal breathing room between the side clusters and the centre breadcrumb.
const CLUSTER_GAP: f32 = 8.0;
/// Size of one self-drawn window-control button (Linux/Windows undecorated chrome).
#[cfg(not(target_os = "macos"))]
const WINDOW_BUTTON_SIZE: egui::Vec2 = egui::vec2(26.0, 22.0);
#[cfg(not(target_os = "macos"))]
const WINDOW_BUTTON_GAP: f32 = 4.0;

/// Left inset to clear native macOS traffic lights when drawing into the titlebar space.
pub fn traffic_lights_inset(ctx: &egui::Context, frame: Option<&eframe::Frame>) -> f32 {
    #[cfg(target_os = "macos")]
    {
        let _ = frame;
        MAC_TRAFFIC_LIGHTS_INSET / ctx.zoom_factor()
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

/// Standard title-bar chrome behaviour for any surface acting as empty header space:
/// double-click toggles maximize, drag starts a native window move (which also gives
/// Aero Snap on Windows and compositor edge-snapping elsewhere).
pub(crate) fn handle_chrome_response(ui: &Ui, resp: &egui::Response) {
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

pub fn breadcrumb(ui: &mut Ui, text: &str, marker: Option<Color32>) -> egui::Response {
    let pill_w = ui.available_width().max(80.0);
    let font = egui::FontId::proportional(10.0);
    let text_color = if marker.is_some() {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    let radius = CornerRadius::same(6);

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(pill_w, BREADCRUMB_HEIGHT),
        egui::Sense::click_and_drag(),
    );

    if ui.is_rect_visible(rect) {
        if let Some(marker) = marker {
            let label_width = ui
                .painter()
                .layout_no_wrap(text.to_owned(), font.clone(), text_color)
                .size()
                .x;
            paint_connection_dots(ui.painter(), rect, radius, marker, label_width);
        }

        let text_rect = rect.shrink2(egui::vec2(10.0, 0.0));
        ui.scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
            ui.with_layout(
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
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

/// Balanced halftone accents frame a quiet central label without a visible border.
/// Static geometry keeps an idle title bar from requesting continuous repaints.
fn paint_connection_dots(
    painter: &egui::Painter,
    rect: Rect,
    radius: CornerRadius,
    marker: Color32,
    label_width: f32,
) {
    painter.rect_filled(
        rect,
        radius,
        crate::style::mix(palette::SURFACE(), marker, 0.08),
    );
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    const SPACING: f32 = 7.0;
    let columns = ((rect.width() - 16.0) / SPACING).floor().max(0.0) as usize;
    let start_x = rect.center().x - columns as f32 * SPACING * 0.5;
    // Lift dark connection colours so the small dots stay visible on the surface.
    let dot_color = crate::style::mix(marker, Color32::WHITE, 0.22);
    let quiet_half_width = (label_width * 0.5 + 18.0).min(rect.width() * 0.5);
    for column in 0..=columns {
        let x = start_x + column as f32 * SPACING;
        let label_fade = ((x - rect.center().x).abs() - quiet_half_width) / 56.0;
        let edge_fade = (x - rect.left()).min(rect.right() - x) / 28.0;
        let strength = label_fade.clamp(0.0, 1.0) * edge_fade.clamp(0.0, 1.0);
        if strength <= 0.01 {
            continue;
        }
        for row in -1..=1 {
            let alpha = (strength * if row == 0 { 190.0 } else { 140.0 }) as u8;
            painter.circle_filled(
                egui::pos2(x, rect.center().y + row as f32 * 6.0),
                1.2,
                Color32::from_rgba_unmultiplied(dot_color.r(), dot_color.g(), dot_color.b(), alpha),
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

#[cfg(not(target_os = "macos"))]
fn window_button(ui: &mut Ui, kind: WindowButton, hover: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(WINDOW_BUTTON_SIZE, egui::Sense::click());
    let danger = matches!(kind, WindowButton::Close);

    let fill = if danger && resp.hovered() {
        palette::DANGER()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        Color32::TRANSPARENT
    };
    let text_color = if danger && resp.hovered() {
        Color32::WHITE
    } else {
        palette::TEXT_WEAK()
    };

    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            CornerRadius::same(4),
            fill,
            egui::Stroke::NONE,
            egui::StrokeKind::Outside,
        );

        let stroke = egui::Stroke::new(1.4_f32, text_color);
        let c = rect.center();
        match kind {
            WindowButton::Minimize => {
                let y = c.y + 4.0;
                ui.painter()
                    .line_segment([egui::pos2(c.x - 4.5, y), egui::pos2(c.x + 4.5, y)], stroke);
            }
            WindowButton::Maximize => {
                let r = Rect::from_center_size(c, egui::vec2(9.0, 8.0));
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(1),
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            WindowButton::Restore => {
                let back = Rect::from_center_size(c + egui::vec2(2.0, -2.0), egui::vec2(8.0, 7.0));
                let front = Rect::from_center_size(c + egui::vec2(-1.5, 1.5), egui::vec2(8.0, 7.0));
                ui.painter().rect_stroke(
                    back,
                    CornerRadius::same(1),
                    stroke,
                    egui::StrokeKind::Inside,
                );
                ui.painter().rect_filled(
                    front.expand(1.0),
                    CornerRadius::ZERO,
                    if resp.hovered() {
                        fill
                    } else {
                        palette::PANEL()
                    },
                );
                ui.painter().rect_stroke(
                    front,
                    CornerRadius::same(1),
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            WindowButton::Close => {
                ui.painter().line_segment(
                    [
                        egui::pos2(c.x - 4.0, c.y - 4.0),
                        egui::pos2(c.x + 4.0, c.y + 4.0),
                    ],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(c.x + 4.0, c.y - 4.0),
                        egui::pos2(c.x - 4.0, c.y + 4.0),
                    ],
                    stroke,
                );
            }
        }
    }

    ui.add_space(WINDOW_BUTTON_GAP);
    resp.on_hover_text(hover)
}

/// Hairline separator between title-bar groups on platforms that draw their own chrome
/// (Linux/Windows). Kept here so macOS chrome is untouched.
#[cfg(not(target_os = "macos"))]
pub fn group_separator(ui: &mut Ui) {
    ui.add_space(2.0);
    let h = 12.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, h), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().vline(
            rect.center().x,
            rect.top()..=rect.bottom(),
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );
    }
    ui.add_space(2.0);
}

/// Window controls for undecorated Linux/Windows windows.
#[cfg(not(target_os = "macos"))]
pub fn window_controls(ui: &mut Ui) -> bool {
    // The app decides whether unsaved work permits closing the window.
    let close_requested = window_button(ui, WindowButton::Close, "Close").clicked();

    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    let max_kind = if maximized {
        WindowButton::Restore
    } else {
        WindowButton::Maximize
    };
    let max_hover = if maximized { "Restore" } else { "Maximize" };
    if window_button(ui, max_kind, max_hover).clicked() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }

    if window_button(ui, WindowButton::Minimize, "Minimize").clicked() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    close_requested
}
