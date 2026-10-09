//! Visual design system: a single source of truth for colour, type, and spacing.
//!
//! Everything UI-facing pulls from [`palette`] rather than hard-coding colours, so the
//! whole app stays cohesive and re-themeable. The concrete colours come from the active
//! [`crate::theme::Theme`], so the same call sites follow whichever theme the user picks.
//! Applied at startup — and again whenever the theme changes — via [`apply`].

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Blend `a` toward `b` by `amount_b` (0.0 → all `a`, 1.0 → all `b`). Used to derive
/// in-between tokens — a muted accent, a punctuation grey — from the theme's own colours
/// rather than hard-coding a third value that no theme author ever chose.
pub fn mix(a: Color32, b: Color32, amount_b: f32) -> Color32 {
    let amount_a = 1.0 - amount_b;
    let ch = |x: u8, y: u8| (x as f32 * amount_a + y as f32 * amount_b).round() as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

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

/// Corner radius tokens for custom-painted widgets.
#[allow(dead_code)]
pub mod radius {
    pub const SM: u8 = 4;
    pub const MD: u8 = 6;
    pub const LG: u8 = 8;
    pub const WINDOW: u8 = 10;
}

/// Spacing tokens for dense studio controls.
#[allow(dead_code)]
pub mod space {
    pub const XS: f32 = 2.0;
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
}

/// The horizontal breathing room between independently-resizable workspace surfaces. Adjacent
/// panels each contribute this margin, so `2` produces a compact four-point seam.
pub const WORKSPACE_GUTTER: i8 = 2;
/// Each card owns half of a seam on every edge. Adjacent cards therefore produce a four-point
/// gutter without doubling the vertical gap; the workspace inset supplies the missing half at
/// an outside edge where there is no neighbouring card.
pub const WORKSPACE_GUTTER_Y: i8 = WORKSPACE_GUTTER;

/// Horizontal content inset shared by the Items and Details side panels.
pub const SIDE_PANEL_PAD_X: i8 = 10;

/// Dark seam between workspace cards. It is derived from the active base colour so light themes
/// keep their contrast while charcoal themes get the deeper gutter used by the studio layout.
pub fn workspace_gap() -> Color32 {
    palette::BASE().gamma_multiply(0.62)
}

/// Frame shared by workspace docks: schema/details sidebars, editor, results, and live log.
/// Rounded surfaces are separated by the outer-margin gutters, with no panel border.
pub fn workspace_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(radius::LG))
        .outer_margin(Margin::symmetric(WORKSPACE_GUTTER, WORKSPACE_GUTTER_Y))
        // Full-bleed grids and dock headers paint rectangular child backgrounds. A four-point
        // inset keeps those fills inside the curved silhouette instead of covering its corners.
        .inner_margin(Margin::same(4))
}

/// Mark the existing draggable panel boundary without adding another interactive widget.
/// `horizontal` means a top/bottom seam, so its three dots run left to right.
pub fn workspace_resize_grip(ui: &egui::Ui, panel_id: egui::Id, horizontal: bool) {
    let Some(handle) = ui.ctx().read_response(panel_id.with("__resize")) else {
        return;
    };
    let center = handle.rect.center();
    let span = if horizontal {
        handle.rect.x_range()
    } else {
        handle.rect.y_range()
    };
    if handle.hovered() || handle.dragged() {
        // egui's own one-point hover line spans the panel's outer rect, past the cards into
        // the window chrome. The seam colour is translucent over the window's BASE clear, so
        // repaint both to erase it before drawing the bar.
        let line = seam_rect(center, span, horizontal);
        ui.painter()
            .rect_filled(line, CornerRadius::ZERO, palette::BASE());
        ui.painter()
            .rect_filled(line, CornerRadius::ZERO, workspace_gap());
    }
    paint_resize_seam(
        ui.painter(),
        center,
        span,
        horizontal,
        handle.hovered(),
        handle.dragged(),
    );
}

/// The four-point gutter around a seam's centre line, along `span`.
fn seam_rect(center: egui::Pos2, span: egui::Rangef, horizontal: bool) -> egui::Rect {
    let across = |c: f32| egui::Rangef::point(c).expand(WORKSPACE_GUTTER as f32);
    if horizontal {
        egui::Rect::from_x_y_ranges(span, across(center.y))
    } else {
        egui::Rect::from_x_y_ranges(across(center.x), span)
    }
}

/// Paint a resize seam the same way everywhere: three faint dots at rest, and a bar filling
/// the whole gutter between the two cards while hovered or dragged, like VS Code's sash.
/// `span` is the seam's outer extent; the bar is inset by a gutter so it ends with the cards.
pub fn paint_resize_seam(
    painter: &egui::Painter,
    center: egui::Pos2,
    span: egui::Rangef,
    horizontal: bool,
    hovered: bool,
    dragged: bool,
) {
    if hovered || dragged {
        let amount = if dragged { 0.42 } else { 0.28 };
        painter.rect_filled(
            seam_rect(center, span.shrink(WORKSPACE_GUTTER as f32), horizontal),
            CornerRadius::same(WORKSPACE_GUTTER as u8),
            mix(palette::PANEL(), palette::TEXT(), amount),
        );
        return;
    }
    for offset in [-5.0, 0.0, 5.0] {
        let pos = if horizontal {
            center + egui::vec2(offset, 0.0)
        } else {
            center + egui::vec2(0.0, offset)
        };
        painter.circle_filled(pos, 1.0, palette::TEXT_FAINT());
    }
}

/// Font size tokens shared by custom-painted components.
#[allow(dead_code)]
pub mod font {
    pub const CAPTION: f32 = 10.5;
    pub const BODY: f32 = 12.5;
    pub const TITLE: f32 = 14.5;
}

/// Apply the plusplus look to a context.
pub fn apply(ctx: &egui::Context) {
    let t = crate::theme::current();
    // egui 0.32+ keeps separate dark/light styles and defaults to following the OS.
    // Custom-painted widgets read `theme::current()`; stock egui widgets read the active
    // style. Painting only the current bucket meant the first frame — when the system
    // theme arrives — could swap in stock light/dark visuals next to ours.
    ctx.set_theme(if t.is_dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });

    let v = visuals();

    // Compact, minimal type scale. One point above the original 12.5 body size: the old
    // scale read too small on most displays.
    // Headless tests do not install the app's embedded fonts; resolving an unbound named
    // family panics in epaint before the behavioral assertion can run.
    let heading_family = if cfg!(test) {
        FontFamily::Proportional
    } else {
        FontFamily::Name(crate::HEADING_FAMILY.into())
    };
    let text_styles = [
        (TextStyle::Heading, FontId::new(13.0, heading_family)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        ),
    ];

    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        style.text_styles = text_styles.clone().into();
        // Short hover/expand transitions instead of egui's instant snap.
        style.animation_time = 0.1;

        // Spacing — tight and even for a clean, dense look.
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(6.0, 4.0);
        s.button_padding = egui::vec2(9.0, 3.0);
        s.menu_margin = Margin::same(5);
        s.indent = 14.0;
        // Buttons and combo boxes adopt the shared control height from here.
        s.interact_size.y = CONTROL_H;
        s.combo_width = 0.0; // let combos size to their content/width hint, not a min
                             // Tighter vertical padding keeps dialog title bars compact.
        s.window_margin = Margin::symmetric(12, 4);
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
    });
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
    v.selection.stroke = Stroke::new(1.0_f32, t.accent);

    // Subtle 1px hairlines instead of egui's heavier defaults.
    v.window_stroke = Stroke::new(1.0_f32, t.border);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };

    let window_radius = CornerRadius::same(6);
    v.window_corner_radius = window_radius;
    v.menu_corner_radius = CornerRadius::same(4);

    let w = &mut v.widgets;
    for state in [
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
        &mut w.noninteractive,
    ] {
        state.corner_radius = CornerRadius::same(4);
    }

    // Separators / frame hairlines and table header column guides. We keep these on the
    // soft `border` token (not `border_strong`) in both light and dark so dividers whisper
    // rather than slice the layout — structure is carried by the surface tints, not lines.
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text_weak);
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border);

    // Default (resting) controls. No always-on outline: a resting input/button reads as a
    // raised `surface` fill against the panel, and only grows a visible edge on hover/focus.
    // This is the modern, minimal look — the previous 1px border on every control was the
    // main source of the "too many hard lines" feel.
    w.inactive.bg_fill = t.surface;
    w.inactive.weak_bg_fill = t.surface;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, t.text_weak);

    // Hover.
    w.hovered.bg_fill = t.surface_hover;
    w.hovered.weak_bg_fill = t.surface_hover;
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0_f32, t.text);

    // Pressed / active.
    w.active.bg_fill = t.accent;
    w.active.weak_bg_fill = t.accent;
    w.active.bg_stroke = Stroke::new(1.0_f32, t.accent_hover);
    w.active.fg_stroke = Stroke::new(1.0_f32, t.on_accent);

    // Open (combo boxes etc.).
    w.open.bg_fill = t.surface_hover;
    w.open.weak_bg_fill = t.surface_hover;
    w.open.bg_stroke = Stroke::new(1.0_f32, t.border_strong);

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stock egui follows the OS and keeps a second, unstyled light/dark bucket. If we
    /// only paint the active one, flipping the preference (the first-frame system-theme
    /// report) would show default colours next to plusplus-painted widgets.
    #[test]
    fn apply_keeps_both_egui_style_buckets_on_the_plusplus_palette() {
        let ctx = egui::Context::default();
        let theme = crate::theme::ThemeRegistry::load().theme_of("carbon");
        crate::theme::set_current(theme);
        apply(&ctx);

        ctx.set_theme(egui::ThemePreference::Light);
        assert_eq!(ctx.global_style().visuals.panel_fill, theme.panel);
        ctx.set_theme(egui::ThemePreference::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, theme.panel);
    }
}
