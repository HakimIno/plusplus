//! Visual design system: a single source of truth for colour, type, and spacing.
//!
//! Everything UI-facing pulls from [`palette`] rather than hard-coding colours, so the
//! whole app stays cohesive and re-themeable. Applied once at startup via [`apply`].

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Named colour tokens. A calm, slightly-cool dark palette in the spirit of modern
/// developer tools (Linear / Raycast): layered surfaces, one confident accent, muted text.
pub mod palette {
    use egui::Color32;

    // --- surfaces (darkest → lightest) ---
    /// App / window background.
    pub const BASE: Color32 = Color32::from_rgb(0x15, 0x16, 0x1a);
    /// Side and tool panels.
    pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x21);
    /// Raised controls: buttons, inputs, list items.
    pub const SURFACE: Color32 = Color32::from_rgb(0x23, 0x26, 0x2d);
    /// Hover state for raised controls.
    pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x2c, 0x30, 0x39);
    /// Code / text-edit background (the deepest well).
    pub const CODE_BG: Color32 = Color32::from_rgb(0x11, 0x12, 0x16);
    /// Striped / alternate rows.
    pub const STRIPE: Color32 = Color32::from_rgb(0x1e, 0x21, 0x27);
    /// Selected-row / selection fill (accent-tinted, opaque so it reads on any surface).
    pub const SELECTION: Color32 = Color32::from_rgb(0x2a, 0x37, 0x5e);

    // --- borders ---
    pub const BORDER: Color32 = Color32::from_rgb(0x2b, 0x2f, 0x38);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x40, 0x4c);

    // --- text ---
    pub const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe6, 0xea);
    pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x99, 0xa0, 0xac);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x68, 0x6f, 0x7d);

    // --- accent ---
    pub const ACCENT: Color32 = Color32::from_rgb(0x6e, 0x8e, 0xff);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x84, 0x9f, 0xff);
    /// Text/icon colour that sits on top of an accent fill.
    pub const ON_ACCENT: Color32 = Color32::from_rgb(0xf6, 0xf8, 0xff);

    // --- semantic ---
    pub const SUCCESS: Color32 = Color32::from_rgb(0x4a, 0xcf, 0x8b);
    pub const DANGER: Color32 = Color32::from_rgb(0xee, 0x6a, 0x6a);
    /// Part of the token set for completeness; reserved for non-fatal notices.
    #[allow(dead_code)]
    pub const WARNING: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
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
    use palette::*;
    let mut v = egui::Visuals::dark();

    v.panel_fill = PANEL;
    v.window_fill = BASE;
    v.extreme_bg_color = CODE_BG;
    v.faint_bg_color = STRIPE;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = SELECTION;
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    // Subtle 1px hairlines instead of egui's heavier defaults.
    v.window_stroke = Stroke::new(1.0, BORDER);
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
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);

    // Default (resting) controls.
    w.inactive.bg_fill = SURFACE;
    w.inactive.weak_bg_fill = SURFACE;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT_WEAK);

    // Hover.
    w.hovered.bg_fill = SURFACE_HOVER;
    w.hovered.weak_bg_fill = SURFACE_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);

    // Pressed / active.
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT_HOVER);
    w.active.fg_stroke = Stroke::new(1.0, ON_ACCENT);

    // Open (combo boxes etc.).
    w.open.bg_fill = SURFACE_HOVER;
    w.open.weak_bg_fill = SURFACE_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);

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
            color: palette::TEXT_FAINT,
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
                .tint(palette::TEXT_FAINT),
        );
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(title)
                .size(14.5)
                .color(palette::TEXT_WEAK),
        );
        ui.add_space(3.0);
        ui.label(egui::RichText::new(hint).color(palette::TEXT_FAINT));
    });
}
