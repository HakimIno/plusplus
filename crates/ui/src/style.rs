//! Visual design system: a single source of truth for colour, type, and spacing.
//!
//! Everything UI-facing pulls from [`palette`] rather than hard-coding colours, so the
//! whole app stays cohesive and re-themeable. The concrete colours come from the active
//! [`crate::theme::Theme`], so the same call sites follow whichever theme the user picks.
//! Applied at startup — and again whenever the theme changes — via [`apply`].

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Named colour tokens, resolved against the currently-active theme. UI code reads colours
/// through these accessors (e.g. `palette::ACCENT()`) so a theme switch flows everywhere
/// without touching call sites. See [`crate::theme`] for the underlying palettes.
///
/// `dead_code` is allowed because this is a complete design-system token set: a few tokens
/// (surfaces/borders) are consumed by [`visuals`] directly off the `Theme` rather than
/// through these accessors, but we expose them all for use by future call sites.
#[allow(non_snake_case, dead_code)]
pub mod palette {
    use egui::Color32;

    use crate::theme::current;

    // --- surfaces (darkest → lightest, for a dark theme) ---
    /// App / window background.
    pub fn BASE() -> Color32 {
        current().base
    }
    /// Side and tool panels.
    pub fn PANEL() -> Color32 {
        current().panel
    }
    /// Raised controls: buttons, inputs, list items.
    pub fn SURFACE() -> Color32 {
        current().surface
    }
    /// Hover state for raised controls.
    pub fn SURFACE_HOVER() -> Color32 {
        current().surface_hover
    }
    /// Code / text-edit background (the deepest well).
    pub fn CODE_BG() -> Color32 {
        current().code_bg
    }
    /// Striped / alternate rows.
    pub fn STRIPE() -> Color32 {
        current().stripe
    }
    /// Selected-row / selection fill (accent-tinted, opaque so it reads on any surface).
    pub fn SELECTION() -> Color32 {
        current().selection
    }

    // --- borders ---
    pub fn BORDER() -> Color32 {
        current().border
    }
    pub fn BORDER_STRONG() -> Color32 {
        current().border_strong
    }

    // --- text ---
    pub fn TEXT() -> Color32 {
        current().text
    }
    pub fn TEXT_WEAK() -> Color32 {
        current().text_weak
    }
    pub fn TEXT_FAINT() -> Color32 {
        current().text_faint
    }

    // --- accent ---
    pub fn ACCENT() -> Color32 {
        current().accent
    }
    pub fn ACCENT_HOVER() -> Color32 {
        current().accent_hover
    }
    /// Text/icon colour that sits on top of an accent fill.
    pub fn ON_ACCENT() -> Color32 {
        current().on_accent
    }

    // --- semantic ---
    pub fn SUCCESS() -> Color32 {
        current().success
    }
    pub fn DANGER() -> Color32 {
        current().danger
    }
    /// Part of the token set for completeness; reserved for non-fatal notices.
    pub fn WARNING() -> Color32 {
        current().warning
    }
}

/// Shared height (in points) for form controls — text inputs, dropdowns, and buttons all
/// line up to this so a row of them reads as one clean band. This is the single knob for the
/// whole app's control sizing: change it here and every control follows. Buttons/combos pick
/// it up via `spacing.interact_size.y` (set in [`apply`]); text fields via [`text_input`].
pub const CONTROL_H: f32 = 24.0;

/// Apply the plusplus look to a context.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals(visuals());

    let mut style = (*ctx.global_style()).clone();

    // Compact, minimal type scale — small but still readable.
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(15.0, FontFamily::Name(crate::HEADING_FAMILY.into())),
        ),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(12.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(10.5, FontFamily::Proportional),
        ),
    ]
    .into();

    // Spacing — tight and even for a clean, dense look.
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(5.0, 4.0);
    s.button_padding = egui::vec2(7.0, 2.0);
    s.menu_margin = Margin::same(5);
    s.indent = 14.0;
    // Buttons and combo boxes adopt the shared control height from here.
    s.interact_size.y = CONTROL_H;
    s.combo_width = 0.0; // let combos size to their content/width hint, not a min
    s.window_margin = Margin::same(12);
    s.scroll.bar_width = 8.0;
    s.scroll.bar_inner_margin = 2.0;

    // Silence egui's developer debug overlays, which are on by default in debug builds
    // (`cfg!(debug_assertions)`). Two of them fire on our virtualized results grid during
    // fast HiDPI scrolling and read as a flickering coloured column border:
    //   * `warn_if_rect_changes_id` — a 2px RED outline egui draws when the same on-screen
    //     rect maps to a different widget id between layout passes. Virtualized rows do
    //     exactly this while scrolling (a screen slot is row N in one pass, row N+1 in the
    //     next), so it false-positives constantly. This is the red border the user saw.
    //   * `show_unaligned` — orange edge lines on any rect not snapped to the pixel grid,
    //     which sub-pixel scroll offsets trigger every frame.
    // These are diagnostics, not real bugs (the headless probes verify no actual id
    // clashes) and they never compile into release. Turning them off makes debug builds
    // look like release.
    // `Style::debug` only exists under `cfg(debug_assertions)`; gating keeps release builds,
    // where these overlays are already compiled out, from referencing a missing field.
    #[cfg(debug_assertions)]
    {
        let dbg = &mut style.debug;
        dbg.warn_if_rect_changes_id = false;
        dbg.show_unaligned = false;
        dbg.show_expand_width = false;
        dbg.show_expand_height = false;
    }

    ctx.set_global_style(style);
}

fn visuals() -> egui::Visuals {
    let t = crate::theme::current();
    let mut v = if t.is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    v.panel_fill = t.panel;
    v.window_fill = t.base;
    v.extreme_bg_color = t.code_bg;
    v.faint_bg_color = t.stripe;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.accent);

    // Subtle 1px hairlines instead of egui's heavier defaults.
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };

    let window_radius = CornerRadius::same(10);
    v.window_corner_radius = window_radius;
    v.menu_corner_radius = CornerRadius::same(8);

    let w = &mut v.widgets;
    for state in [
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
        &mut w.noninteractive,
    ] {
        state.corner_radius = CornerRadius::same(6);
    }

    // Separators / frame hairlines.
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);

    // Default (resting) controls.
    w.inactive.bg_fill = t.surface;
    w.inactive.weak_bg_fill = t.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.fg_stroke = Stroke::new(1.0, t.text_weak);

    // Hover.
    w.hovered.bg_fill = t.surface_hover;
    w.hovered.weak_bg_fill = t.surface_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, t.text);

    // Pressed / active.
    w.active.bg_fill = t.accent;
    w.active.weak_bg_fill = t.accent;
    w.active.bg_stroke = Stroke::new(1.0, t.accent_hover);
    w.active.fg_stroke = Stroke::new(1.0, t.on_accent);

    // Open (combo boxes etc.).
    w.open.bg_fill = t.surface_hover;
    w.open.weak_bg_fill = t.surface_hover;
    w.open.bg_stroke = Stroke::new(1.0, t.border_strong);

    v
}

// --- reusable building blocks ------------------------------------------------

/// The app's single-line text field. Every text input in the app should go through this so
/// they share one height ([`CONTROL_H`]), padding, and alignment — change the look here and
/// it propagates everywhere. `width` is the exact outer width; pass `ui.available_width()` to
/// fill the rest of a row.
pub fn text_input(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add_sized(
        egui::vec2(width, CONTROL_H),
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .vertical_align(egui::Align::Center)
            .margin(Margin::symmetric(6, 0)),
    )
}

/// Single-line label that truncates with "…" when it doesn't fit the panel width.
/// Pass `tooltip` to show the full text on hover (defaults to `text`).
pub fn truncated_label(
    ui: &mut egui::Ui,
    text: &str,
    tooltip: Option<&str>,
    weak: bool,
    sense: egui::Sense,
) -> egui::Response {
    let label = if weak {
        egui::Label::new(egui::RichText::new(text).color(palette::TEXT_WEAK()))
    } else {
        egui::Label::new(text)
    };
    ui.add(label.truncate().selectable(false).sense(sense))
        .on_hover_text(tooltip.unwrap_or(text))
}

/// A muted, letter-spaced uppercase section label — the small caption that titles each
/// panel (CONNECTIONS, SCHEMA, …). Adds a little breathing room above itself.
pub fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        TextFormat {
            font_id: FontId::new(11.0, FontFamily::Proportional),
            color: palette::TEXT_FAINT(),
            extra_letter_spacing: 1.4,
            ..Default::default()
        },
    );
    ui.add(egui::Label::new(job).selectable(false));
    ui.add_space(3.0);
}

/// A small filled status dot (connected = green, idle = faint), vertically centred so it
/// sits neatly inline before a label.
#[allow(dead_code)]
pub fn status_dot(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 3.5, color);
    }
    resp
}

/// A centred empty-state placeholder: a large faint glyph, a title, and a hint line.
/// Used wherever a panel has nothing to show yet (no results, no selection, …).
pub fn empty_state(ui: &mut egui::Ui, icon: egui::ImageSource<'static>, title: &str, hint: &str) {
    ui.add_space((ui.available_height() * 0.30).max(24.0));
    ui.vertical_centered(|ui| {
        ui.add(
            egui::Image::new(icon)
                .fit_to_exact_size(egui::vec2(40.0, 40.0))
                .tint(palette::TEXT_FAINT()),
        );
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(title)
                .size(14.5)
                .color(palette::TEXT_WEAK()),
        );
        ui.add_space(3.0);
        ui.label(egui::RichText::new(hint).color(palette::TEXT_FAINT()));
    });
}

/// A centred decorative placeholder without labels. The base art is a static SVG; fishing motion
/// (swaying line, bobber, ice-hole ripples) is drawn on top because egui rasterizes SVGs once.
pub fn empty_illustration(ui: &mut egui::Ui, image: egui::ImageSource<'static>) {
    ui.add_space((ui.available_height() * 0.24).max(18.0));
    ui.vertical_centered(|ui| {
        let width = ui.available_width().min(320.0);
        let height = width * 210.0 / 320.0;
        let size = egui::vec2(width, height);
        let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());

        if ui.is_rect_visible(rect) {
            egui::Image::new(image)
                .fit_to_exact_size(size)
                .paint_at(ui, rect);

            let time = ui.input(|i| i.time) as f32;
            let painter = ui.painter_at(rect);
            let accent = palette::ACCENT();
            let accent_hover = palette::ACCENT_HOVER();
            let sx = |x: f32| rect.min.x + x / 320.0 * rect.width();
            let sy = |y: f32| rect.min.y + y / 210.0 * rect.height();
            let sr = |r: f32| r / 320.0 * rect.width();

            // Gentle bob + occasional slow tug, like waiting for a bite.
            let bob = (time * 1.5).sin();
            let tug = (time * 0.38).sin().powi(2);
            let line_drift = bob * 2.8 + tug * 4.5;
            let sway = (time * 0.9).sin() * 1.2;

            let rod_tip = egui::pos2(sx(84.0 + sway * 0.25), sy(38.0 + line_drift * 0.12));
            let hole_surface = egui::pos2(sx(84.0), sy(172.0));
            let bobber = egui::pos2(sx(84.0 + sway * 0.35), sy(158.0 + line_drift));

            // Fishing line from rod tip down into the ice hole.
            painter.line_segment(
                [rod_tip, bobber],
                Stroke::new(sr(1.6), accent),
            );
            painter.line_segment(
                [bobber, hole_surface],
                Stroke::new(sr(1.4), Color32::from_rgba_unmultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    170,
                )),
            );

            // Bobber float.
            painter.circle_filled(bobber, sr(3.4), accent_hover);
            painter.circle_stroke(bobber, sr(3.4), Stroke::new(sr(0.8), accent));
            painter.line_segment(
                [
                    egui::pos2(bobber.x - sr(2.8), bobber.y),
                    egui::pos2(bobber.x + sr(2.8), bobber.y),
                ],
                Stroke::new(sr(0.9), Color32::WHITE),
            );

            // Ice-hole ripples when the bobber dips.
            let ripple_boost = if bob < -0.55 { 1.35 } else { 1.0 };
            for i in 0..3 {
                let phase = (time * 0.75 * ripple_boost + i as f32 * 0.38).fract();
                let ripple_r = sr(6.0 + phase * 24.0);
                let alpha = ((1.0 - phase) * 0.34 * 255.0) as u8;
                painter.circle_stroke(
                    hole_surface,
                    ripple_r,
                    Stroke::new(
                        sr(1.1),
                        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha),
                    ),
                );
            }

            // Tiny splash marks when the line tugs.
            if tug > 0.82 {
                let splash = (tug - 0.82) / 0.18;
                let splash_alpha = (splash * 0.55 * 255.0) as u8;
                for (dx, dy) in [(-5.0_f32, -2.0), (5.0, -1.5), (0.0, -3.5)] {
                    painter.circle_filled(
                        egui::pos2(sx(84.0 + dx), sy(168.0 + dy)),
                        sr(1.0 + splash),
                        Color32::from_rgba_unmultiplied(accent_hover.r(), accent_hover.g(), accent_hover.b(), splash_alpha),
                    );
                }
            }

            // Soft glint on the sloth's glasses lenses.
            let glint_alpha = (0.2 + 0.35 * (time * 1.8).sin().powi(2)) * 255.0;
            for (x, y) in [(198.0_f32, 124.0), (203.0, 124.0)] {
                painter.circle_filled(
                    egui::pos2(sx(x), sy(y)),
                    sr(1.6),
                    Color32::from_rgba_unmultiplied(255, 255, 255, glint_alpha as u8),
                );
            }

            ui.ctx().request_repaint();
        }
    });
}
