//! Button primitives for the app's shared control language.

use egui::{ImageSource, Response, RichText, Ui};

use crate::icons;
use crate::style::palette;

/// Visual treatment for a shared app button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonVariant {
    Primary,
    Default,
    Ghost,
    Danger,
}

/// Fluent builder for text, icon, and toolbar buttons.
pub(crate) struct Btn<'a> {
    label: String,
    icon: Option<ImageSource<'static>>,
    variant: ButtonVariant,
    enabled: bool,
    tooltip: Option<&'a str>,
    icon_only: bool,
}

impl<'a> Btn<'a> {
    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            variant: ButtonVariant::Default,
            enabled: true,
            tooltip: None,
            icon_only: false,
        }
    }

    pub(crate) fn primary(label: impl Into<String>) -> Self {
        Self::new(label).variant(ButtonVariant::Primary)
    }

    pub(crate) fn danger(label: impl Into<String>) -> Self {
        Self::new(label).variant(ButtonVariant::Danger)
    }

    pub(crate) fn ghost_icon(src: ImageSource<'static>) -> Self {
        Self::new("")
            .icon(src)
            .variant(ButtonVariant::Ghost)
            .icon_only()
    }

    pub(crate) fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub(crate) fn icon(mut self, src: ImageSource<'static>) -> Self {
        self.icon = Some(src);
        self
    }

    pub(crate) fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub(crate) fn tooltip(mut self, text: &'a str) -> Self {
        self.tooltip = Some(text);
        self
    }

    pub(crate) fn icon_only(mut self) -> Self {
        self.icon_only = true;
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> Response {
        let (variant, enabled) = (self.variant, self.enabled);
        let text_color = match self.variant {
            ButtonVariant::Primary => palette::ON_ACCENT(),
            ButtonVariant::Danger => palette::DANGER(),
            ButtonVariant::Default | ButtonVariant::Ghost => {
                ui.visuals().widgets.inactive.fg_stroke.color
            }
        };
        let icon_tint = match self.variant {
            ButtonVariant::Primary => palette::ON_ACCENT(),
            ButtonVariant::Danger => palette::DANGER(),
            ButtonVariant::Default | ButtonVariant::Ghost => text_color,
        };

        let mut label = RichText::new(self.label).color(text_color);
        if matches!(self.variant, ButtonVariant::Primary) {
            label = label.strong();
        }

        let mut btn = match (self.icon, self.icon_only) {
            (Some(src), true) => {
                let img = icon_image(src, icons::SIZE, icon_tint);
                egui::Button::image(img)
            }
            (Some(src), false) => {
                let img = icon_image(src, icons::SIZE, icon_tint);
                egui::Button::image_and_text(img, label)
            }
            (None, _) => egui::Button::new(label),
        };

        btn = match self.variant {
            ButtonVariant::Primary => btn
                .fill(palette::ACCENT())
                .stroke(egui::Stroke::new(1.0, palette::ACCENT_HOVER())),
            // Destructive actions carry their weight in the red label and icon. They keep the
            // frame of whatever context they sit in — no forced outline: most of them are menu
            // items, and an always-on red box around one row of an otherwise frameless menu
            // read as a stray error state rather than a delete action. The danger now lands on
            // hover instead (see below), the way a native menu highlights a destructive row.
            ButtonVariant::Danger => btn,
            ButtonVariant::Ghost => btn
                .fill(egui::Color32::TRANSPARENT)
                .frame(false)
                .frame_when_inactive(false),
            ButtonVariant::Default => btn,
        };

        let resp = if matches!(variant, ButtonVariant::Danger) {
            // Scoped so the danger hover/press tint applies to this button alone and not to the
            // menu items laid out after it.
            ui.scope(|ui| {
                let d = palette::DANGER();
                let tint =
                    |alpha: u8| egui::Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), alpha);
                let w = &mut ui.visuals_mut().widgets;
                w.hovered.bg_fill = tint(30);
                w.hovered.weak_bg_fill = tint(30);
                w.active.bg_fill = tint(64);
                w.active.weak_bg_fill = tint(64);
                ui.add_enabled(enabled, btn)
            })
            .inner
        } else {
            ui.add_enabled(enabled, btn)
        };
        if let Some(text) = self.tooltip {
            resp.on_hover_text(text)
        } else {
            resp
        }
    }
}

fn icon_image(src: ImageSource<'static>, size: f32, tint: egui::Color32) -> egui::Image<'static> {
    egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(tint)
}

/// A submenu row carrying the same icon + label as [`Btn`], so a menu that mixes plain items
/// with a submenu still lines its labels up. `Ui::menu_button` takes atoms rather than a
/// `Button`, so it cannot go through `Btn` — this keeps the two in step.
pub(crate) fn menu_button<R>(
    ui: &mut Ui,
    src: ImageSource<'static>,
    text: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let color = ui.visuals().widgets.inactive.fg_stroke.color;
    let icon = icon_image(src, icons::SIZE, color);
    ui.menu_button((icon, RichText::new(text).color(color)), add_contents)
        .response
}

pub(crate) fn button(
    ui: &mut Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
) -> Response {
    let destructive = ["Delete", "Drop", "Truncate"]
        .iter()
        .any(|word| text.contains(word));
    let btn = if destructive {
        Btn::danger(text)
    } else {
        Btn::new(text)
    };
    btn.icon(src).enabled(enabled).show(ui)
}

pub(crate) fn primary_button(
    ui: &mut Ui,
    src: ImageSource<'static>,
    text: &str,
    enabled: bool,
) -> Response {
    Btn::primary(text).icon(src).enabled(enabled).show(ui)
}

pub(crate) fn icon_button(ui: &mut Ui, src: ImageSource<'static>, hover: &str) -> Response {
    Btn::ghost_icon(src).tooltip(hover).show(ui)
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;

    /// Screenshot generator (ignored): a context menu mixing plain items with a submenu, the
    /// shape of the connection menu. The submenu row must carry an icon like its siblings —
    /// without one its label hangs left of the column and the row reads as a different kind
    /// of thing.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_menu_with_submenu() {
        let theme = crate::theme::ThemeRegistry::load().theme_of("plusplus-dark");
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(240.0, 190.0))
            .with_pixels_per_point(2.0)
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::theme::set_current(theme);
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::style::palette::BASE(),
                );
                ui.menu_button("open", |ui| {
                    ui.set_min_width(180.0);
                    super::button(ui, crate::icons::connect(), "Reconnect", true);
                    super::menu_button(ui, crate::icons::database(), "Switch Database", |ui| {
                        ui.label("demo");
                    });
                    super::button(ui, crate::icons::edit(), "Edit\u{2026}", true);
                    super::button(ui, crate::icons::disconnect(), "Disconnect", true);
                    super::button(ui, crate::icons::trash(), "Delete", true);
                });
            });
        harness.run_steps(4);
        harness.get_by_label("open").click();
        harness.run();
        let found = harness.query_by_label("Reconnect").is_some();
        println!("MENU OPEN (Reconnect visible): {found}");
        harness.snapshot("menu_with_submenu");
    }
}
