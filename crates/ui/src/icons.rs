//! Iconoir icons (https://iconoir.com), downloaded as SVG and embedded into the binary.
//! They are rendered via `egui_extras`' SVG image loader and tinted to the current theme
//! text colour, so they stay crisp at any size and adapt to light/dark themes.

use egui::{include_image, ImageSource};

/// Default on-canvas size for an icon, in points.
pub const SIZE: f32 = 16.0;

macro_rules! icon_fns {
    ($($name:ident => $path:literal),* $(,)?) => {
        $(
            #[inline]
            pub fn $name() -> ImageSource<'static> {
                include_image!($path)
            }
        )*
    };
}

icon_fns! {
    play       => "../assets/icons/play.svg",
    connect    => "../assets/icons/connect.svg",
    disconnect => "../assets/icons/disconnect.svg",
    plus       => "../assets/icons/plus.svg",
    edit       => "../assets/icons/edit.svg",
    trash      => "../assets/icons/trash.svg",
    database   => "../assets/icons/database.svg",
    table      => "../assets/icons/table.svg",
    column     => "../assets/icons/column.svg",
    key        => "../assets/icons/key.svg",
    index      => "../assets/icons/index.svg",
    filter     => "../assets/icons/filter.svg",
    warning    => "../assets/icons/warning.svg",
    close      => "../assets/icons/close.svg",
    save       => "../assets/icons/save.svg",
}

/// Build a themed image widget for an icon at the given size.
fn image(ui: &egui::Ui, src: ImageSource<'static>, size: f32, tint: egui::Color32) -> egui::Image<'static> {
    let _ = ui;
    egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(tint)
}

/// Render a decorative inline icon (no interaction), tinted to the normal text colour.
pub fn show(ui: &mut egui::Ui, src: ImageSource<'static>, size: f32) -> egui::Response {
    let tint = ui.visuals().text_color();
    ui.add(image(ui, src, size, tint))
}

/// Render a dimmed/weak inline icon (matches `ui.weak`).
pub fn show_weak(ui: &mut egui::Ui, src: ImageSource<'static>, size: f32) -> egui::Response {
    let tint = ui.visuals().weak_text_color();
    ui.add(image(ui, src, size, tint))
}

/// Render an inline icon tinted to an explicit colour (for semantic glyphs like the
/// error/warning triangle).
pub fn show_colored(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    size: f32,
    color: egui::Color32,
) -> egui::Response {
    ui.add(image(ui, src, size, color))
}

/// A text button with a leading icon.
pub fn button(ui: &mut egui::Ui, src: ImageSource<'static>, text: &str, enabled: bool) -> egui::Response {
    let tint = ui.visuals().widgets.inactive.fg_stroke.color;
    let img = image(ui, src, SIZE, tint);
    ui.add_enabled(enabled, egui::Button::image_and_text(img, text))
}

/// A filled, accent-coloured primary button — for the one main action in view (Run).
pub fn primary_button(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
) -> egui::Response {
    use crate::style::palette;
    let img = image(ui, src, SIZE, palette::ON_ACCENT());
    let btn = egui::Button::image_and_text(
        img,
        egui::RichText::new(text).color(palette::ON_ACCENT()).strong(),
    )
    .fill(palette::ACCENT())
    .stroke(egui::Stroke::new(1.0, palette::ACCENT_HOVER()));
    ui.add_enabled(enabled, btn)
}

/// A text button with a leading icon that reads as "on" (accent-tinted with a soft fill)
/// when `active`. Used for toggles like the filter-bar switch.
pub fn toggle_button(
    ui: &mut egui::Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    use crate::style::palette;
    let tint = if active {
        palette::ACCENT()
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let img = image(ui, src, SIZE, tint);
    let label = if active {
        egui::RichText::new(text).color(palette::ACCENT()).strong()
    } else {
        egui::RichText::new(text)
    };
    let mut btn = egui::Button::image_and_text(img, label);
    if active {
        btn = btn
            .fill(palette::SELECTION())
            .stroke(egui::Stroke::new(1.0, palette::ACCENT()));
    }
    ui.add_enabled(enabled, btn)
}

/// A compact icon-only button with a hover tooltip.
pub fn icon_button(ui: &mut egui::Ui, src: ImageSource<'static>, hover: &str) -> egui::Response {
    let tint = ui.visuals().widgets.inactive.fg_stroke.color;
    let img = image(ui, src, SIZE, tint);
    ui.add(egui::Button::image(img)).on_hover_text(hover)
}
