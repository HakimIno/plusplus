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
    pub fn BASE() -> Color32 { current().base }
    /// Side and tool panels.
    pub fn PANEL() -> Color32 { current().panel }
    /// Raised controls: buttons, inputs, list items.
    pub fn SURFACE() -> Color32 { current().surface }
    /// Hover state for raised controls.
    pub fn SURFACE_HOVER() -> Color32 { current().surface_hover }
    /// Code / text-edit background (the deepest well).
    pub fn CODE_BG() -> Color32 { current().code_bg }
    /// Striped / alternate rows.
    pub fn STRIPE() -> Color32 { current().stripe }
    /// Selected-row / selection fill (accent-tinted, opaque so it reads on any surface).
    pub fn SELECTION() -> Color32 { current().selection }

    // --- borders ---
    pub fn BORDER() -> Color32 { current().border }
    pub fn BORDER_STRONG() -> Color32 { current().border_strong }

    // --- text ---
    pub fn TEXT() -> Color32 { current().text }
    pub fn TEXT_WEAK() -> Color32 { current().text_weak }
    pub fn TEXT_FAINT() -> Color32 { current().text_faint }

    // --- accent ---
    pub fn ACCENT() -> Color32 { current().accent }
    pub fn ACCENT_HOVER() -> Color32 { current().accent_hover }
    /// Text/icon colour that sits on top of an accent fill.
    pub fn ON_ACCENT() -> Color32 { current().on_accent }

    // --- semantic ---
    pub fn SUCCESS() -> Color32 { current().success }
    pub fn DANGER() -> Color32 { current().danger }
    /// Part of the token set for completeness; reserved for non-fatal notices.
    pub fn WARNING() -> Color32 { current().warning }
}

/// Apply the db-gui look to a context.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals(visuals());

    let mut style = (*ctx.global_style()).clone();

    // Roomier, more readable type scale.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
    ]
    .into();

    // Spacing.
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 6.0);
    s.button_padding = egui::vec2(10.0, 6.0);
    s.menu_margin = Margin::same(6);
    s.indent = 16.0;
    s.interact_size.y = 27.0;
    s.window_margin = Margin::same(14);
    s.scroll.bar_width = 9.0;
    s.scroll.bar_inner_margin = 3.0;

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
    let dbg = &mut style.debug;
    dbg.warn_if_rect_changes_id = false;
    dbg.show_unaligned = false;
    dbg.show_expand_width = false;
    dbg.show_expand_height = false;

    ctx.set_global_style(style);
}

fn visuals() -> egui::Visuals {
    let t = crate::theme::current();
    let mut v = if t.is_dark { egui::Visuals::dark() } else { egui::Visuals::light() };

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
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open, &mut w.noninteractive] {
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
pub fn status_dot(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 3.5, color);
    }
    resp
}

/// A centred empty-state placeholder: a large faint glyph, a title, and a hint line.
/// Used wherever a panel has nothing to show yet (no results, no selection, …).
pub fn empty_state(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    title: &str,
    hint: &str,
) {
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
