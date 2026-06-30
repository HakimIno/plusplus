//! Visual design system: a single source of truth for colour, type, and spacing.
//!
//! Everything UI-facing pulls from [`palette`] rather than hard-coding colours, so the
//! whole app stays cohesive and re-themeable. The concrete colours come from the active
//! [`crate::theme::Theme`], so the same call sites follow whichever theme the user picks.
//! Applied at startup — and again whenever the theme changes — via [`apply`].

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Stroke, TextStyle, Vec2};

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
            FontId::new(12.5, FontFamily::Name(crate::HEADING_FAMILY.into())),
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

    // Separators / frame hairlines and table header column guides.
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_weak);
    let hairline = if t.is_dark {
        t.border_strong
    } else {
        t.border
    };
    w.noninteractive.bg_stroke = Stroke::new(1.0, hairline);

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

/// Like [`text_input`], but with a leading icon painted inside the field (TablePlus-style).
pub fn icon_text_input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    icon: egui::ImageSource<'static>,
    width: f32,
) -> egui::Response {
    const ICON: f32 = 14.0;
    let tint = palette::TEXT_FAINT();
    let img = egui::Image::new(icon)
        .fit_to_exact_size(egui::vec2(ICON, ICON))
        .tint(tint);
    ui.add_sized(
        egui::vec2(width, CONTROL_H),
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .prefix((img, " "))
            .vertical_align(egui::Align::Center)
            .margin(Margin::symmetric(6, 0)),
    )
}

/// Rounded-square checkbox styled with the accent colour — a filled square with a bold
/// white tick when checked, an empty bordered square when unchecked. Matches the schema
/// editor table (NULL / PK / Unique). Pass `label: None` for a box-only toggle.
pub fn accent_checkbox(
    ui: &mut egui::Ui,
    enabled: bool,
    checked: &mut bool,
    label: Option<&str>,
) -> egui::Response {
    const SIZE: f32 = 16.0;
    const R: CornerRadius = CornerRadius::same(4);

    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), sense);

    let label_resp = label.map(|label| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(label)
                    .size(11.5)
                    .color(if enabled {
                        palette::TEXT_WEAK()
                    } else {
                        palette::TEXT_FAINT()
                    }),
            )
            .sense(sense),
        )
    });

    let toggled = resp.clicked() || label_resp.as_ref().is_some_and(|r| r.clicked());
    if enabled && toggled {
        *checked = !*checked;
        resp.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let accent = palette::ACCENT();
        let painter = ui.painter();
        if *checked {
            let fill = if enabled {
                accent
            } else {
                accent.linear_multiply(0.4)
            };
            painter.rect_filled(rect, R, fill);
            let p = rect.min;
            let s = rect.size();
            let stroke = Stroke::new(2.2, Color32::WHITE);
            painter.line_segment(
                [
                    Pos2::new(p.x + s.x * 0.19, p.y + s.y * 0.52),
                    Pos2::new(p.x + s.x * 0.42, p.y + s.y * 0.76),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(p.x + s.x * 0.42, p.y + s.y * 0.76),
                    Pos2::new(p.x + s.x * 0.81, p.y + s.y * 0.25),
                ],
                stroke,
            );
        } else {
            let (fill, border) = if resp.hovered() && enabled {
                (
                    accent.linear_multiply(0.10),
                    Stroke::new(1.5, accent.linear_multiply(0.65)),
                )
            } else {
                (
                    Color32::TRANSPARENT,
                    Stroke::new(1.5, palette::BORDER_STRONG()),
                )
            };
            painter.rect(rect, R, fill, border, egui::StrokeKind::Inside);
        }
    }

    resp
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
    let header_color = if crate::theme::current().is_dark {
        palette::TEXT_FAINT()
    } else {
        palette::TEXT_WEAK()
    };
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        TextFormat {
            font_id: FontId::new(11.0, FontFamily::Proportional),
            color: header_color,
            extra_letter_spacing: 1.4,
            ..Default::default()
        },
    );
    ui.add(egui::Label::new(job).selectable(false));
    ui.add_space(3.0);
}

/// Centered modal window — shared baseline for Settings, connection editor, previews, etc.
pub fn dialog_window(title: impl Into<egui::WidgetText>) -> egui::Window<'static> {
    egui::Window::new(title)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
}

/// Compact window chrome: less vertical padding in the title band than the default frame.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    egui::Frame::window(&style).inner_margin(Margin::symmetric(12, 4))
}

/// Action bar at the bottom of modal dialogs — mirrors the window title band (fill, rule, height).
pub fn dialog_footer(ui: &mut egui::Ui, add_buttons: impl FnOnce(&mut egui::Ui)) {
    let t = crate::theme::current();
    let margin = ui.style().spacing.window_margin;
    let bar_h = CONTROL_H + margin.topf() + margin.bottomf();
    let bleed_x = margin.leftf() + margin.rightf();

    ui.add_space(8.0);

    // Size to the body content already laid out above — not `available_rect_before_wrap()`,
    // which spans to `max_rect` and forces the window to stretch full-width.
    let body_w = ui.min_rect().width();
    let (row_rect, _) =
        ui.allocate_exact_size(egui::vec2(body_w, bar_h), egui::Sense::hover());

    // Bleed into window padding for paint only; layout width stays at `body_w`.
    let paint_rect = egui::Rect::from_min_size(
        row_rect.min - egui::vec2(margin.leftf(), 0.0),
        egui::vec2(row_rect.width() + bleed_x, row_rect.height() + margin.bottomf()),
    );
    if ui.is_rect_visible(paint_rect) {
        let mut round = ui.style().visuals.window_corner_radius;
        round.nw = 0;
        round.ne = 0;
        ui.painter()
            .rect_filled(paint_rect, round, t.base);
        ui.painter().hline(
            paint_rect.x_range(),
            paint_rect.top(),
            Stroke::new(1.0, t.border),
        );
    }

    ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
        ui.with_layout(
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                add_buttons(ui);
            },
        );
    });
}

/// Header-band fill + bottom rule for [`egui_extras::Table`] columns.
pub fn paint_table_header_cell(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    if !ui.is_rect_visible(rect) {
        return;
    }
    let t = crate::theme::current();
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, t.panel);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, t.border),
    );
}

/// A small rounded type tag (INTEGER, DATE, …) tinted with a semantic colour, used by the
/// Details panel so a column's kind is readable at a glance.
pub fn type_badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_uppercase(),
        FontId::new(9.0, FontFamily::Proportional),
        color,
    );
    let pad = egui::vec2(5.0, 2.0);
    let (rect, _) =
        ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let tint = |a: u8| Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a);
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(3),
            tint(22),
            egui::Stroke::new(1.0, tint(64)),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(rect.min + pad, galley, color);
    }
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

/// Theme-accent loading spinner.
pub fn spinner(size: f32) -> egui::Spinner {
    egui::Spinner::new().size(size).color(palette::ACCENT())
}

/// A centred loading placeholder: spinner plus a short status line.
pub fn loading_state(ui: &mut egui::Ui, message: &str) {
    ui.add_space((ui.available_height() * 0.30).max(24.0));
    ui.vertical_centered(|ui| {
        ui.add(spinner(32.0));
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(message)
                .size(14.5)
                .color(palette::TEXT_WEAK()),
        );
    });
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

/// A centred, decorative empty-state: a draggable pixel-art slime that idles, blinks and can be
/// flung around its box. See [`crate::pet`].
pub fn empty_illustration(ui: &mut egui::Ui) {
    crate::pet::show(ui);
}
