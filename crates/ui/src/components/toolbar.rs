//! Toolbar and title-bar controls.

use crate::icons;
use crate::style::palette;

const TOOLBAR_BUTTON_SIZE: egui::Vec2 = egui::vec2(22.0, 22.0);
const TOOLBAR_ICON_GAP: f32 = 0.0;

/// Small layout toggle (sidebar on/off) used in the unified title bar.
pub(crate) fn layout_toggle(
    ui: &mut egui::Ui,
    active: bool,
    side: LayoutSide,
    hover: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(TOOLBAR_BUTTON_SIZE, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        // Flat and borderless: a soft rounded backdrop only on hover. The on/off state reads
        // from the glyph colour (accent when the panel is shown) rather than a boxed frame.
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), palette::SURFACE_HOVER());
        }

        // VS Code-style layout glyph: thin outer frame + filled bar on one edge.
        let icon = rect.shrink(6.0);
        let bar_w = 2.5;
        let gap = 1.0;
        let color = if active {
            palette::ACCENT()
        } else if resp.hovered() {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let frame = egui::Stroke::new(1.0, color);
        ui.painter().rect_stroke(
            icon,
            egui::CornerRadius::same(2),
            frame,
            egui::StrokeKind::Inside,
        );

        match side {
            LayoutSide::Connections => {
                let left = egui::Rect::from_min_size(icon.min, egui::vec2(bar_w, icon.height()));
                ui.painter()
                    .rect_filled(left, egui::CornerRadius::ZERO, color);
            }
            LayoutSide::Schema => {
                let left = egui::Rect::from_min_size(icon.min, egui::vec2(bar_w, icon.height()));
                let mid = egui::Rect::from_min_size(
                    egui::pos2(icon.min.x + bar_w + gap, icon.min.y),
                    egui::vec2(icon.width() - bar_w - gap, icon.height()),
                );
                ui.painter()
                    .rect_filled(left, egui::CornerRadius::ZERO, color);
                ui.painter()
                    .rect_filled(mid, egui::CornerRadius::ZERO, color);
            }
            LayoutSide::Details => {
                let right = egui::Rect::from_min_size(
                    egui::pos2(icon.max.x - bar_w, icon.min.y),
                    egui::vec2(bar_w, icon.height()),
                );
                ui.painter()
                    .rect_filled(right, egui::CornerRadius::ZERO, color);
            }
            LayoutSide::Query => {
                let bottom = egui::Rect::from_min_size(
                    egui::pos2(icon.min.x, icon.max.y - bar_w),
                    egui::vec2(icon.width(), bar_w),
                );
                ui.painter()
                    .rect_filled(bottom, egui::CornerRadius::ZERO, color);
            }
        }
    }

    ui.add_space(TOOLBAR_ICON_GAP);
    resp.on_hover_text(hover)
}

#[derive(Clone, Copy)]
pub(crate) enum LayoutSide {
    Connections,
    Schema,
    Details,
    Query,
}

/// Outline accent button for the title-bar update affordance.
pub(crate) fn update_outline_button(ui: &mut egui::Ui, label: &str, busy: bool) -> egui::Response {
    let accent = palette::ACCENT();
    let text = egui::RichText::new(label).color(accent).strong().size(11.0);
    let btn = egui::Button::new(text)
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0, accent))
        .corner_radius(egui::CornerRadius::same(4))
        .min_size(egui::vec2(0.0, 22.0));
    let resp = ui.add_enabled(!busy, btn);
    ui.add_space(TOOLBAR_ICON_GAP);
    resp
}

pub(crate) fn toolbar_icon_button(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    hover: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(TOOLBAR_BUTTON_SIZE, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        // Flat and borderless: only a soft rounded backdrop on hover, no frame at rest.
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), palette::SURFACE_HOVER());
        }

        let color = if resp.hovered() {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(13.0, 13.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(icon_rect), |ui| {
            icons::show_colored(ui, src, 13.0, color);
        });
    }

    ui.add_space(TOOLBAR_ICON_GAP);
    resp.on_hover_text(hover)
}

/// Outcome of the Beautify split button.
pub(crate) struct BeautifyResponse {
    /// The main segment was clicked: format the active tab's SQL.
    pub clicked: bool,
    /// A preference in the dropdown changed: persist settings.
    pub prefs_changed: bool,
}

/// The query console's "Beautify ⌘I ⌄" split button (TablePlus-style): the main segment
/// reformats the SQL in the active connection's dialect, the chevron opens formatting
/// preferences. Painted as one pill with an internal hairline so the two hit areas read
/// as a single control.
pub(crate) fn beautify_button(
    ui: &mut egui::Ui,
    prefs: &mut crate::format::BeautifyPrefs,
    enabled: bool,
    dialect_label: &str,
) -> BeautifyResponse {
    let mut out = BeautifyResponse {
        clicked: false,
        prefs_changed: false,
    };

    // Platform-aware shortcut hint ("⌘I" on macOS, "Ctrl+I" elsewhere).
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::I);
    let hint = ui.ctx().format_shortcut(&shortcut);

    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_color = if enabled {
        palette::TEXT()
    } else {
        palette::TEXT_FAINT()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Beautify",
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: text_color,
            ..Default::default()
        },
    );
    job.append(
        &hint,
        6.0,
        egui::TextFormat {
            font_id: font,
            color: palette::TEXT_FAINT(),
            ..Default::default()
        },
    );
    let galley = ui.fonts_mut(|f| f.layout_job(job));

    // One allocation, two interaction zones: the label segment and the chevron segment.
    let pad_x = 9.0;
    let chevron_w = 19.0;
    let h = 22.0;
    let main_w = galley.size().x + pad_x * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(main_w + chevron_w, h), egui::Sense::hover());
    let main_rect = egui::Rect::from_min_size(rect.min, egui::vec2(main_w, h));
    let chev_rect = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + main_w, rect.min.y),
        egui::vec2(chevron_w, h),
    );
    let main_resp = ui.interact(
        main_rect,
        ui.id().with("beautify_main"),
        egui::Sense::click(),
    );
    let chev_resp = ui.interact(
        chev_rect,
        ui.id().with("beautify_menu"),
        egui::Sense::click(),
    );

    if ui.is_rect_visible(rect) {
        let radius = egui::CornerRadius::same(5);
        ui.painter().rect(
            rect,
            radius,
            palette::SURFACE(),
            egui::Stroke::new(1.0, palette::BORDER()),
            egui::StrokeKind::Outside,
        );
        // Per-segment hover wash, rounded only on its outer corners so it stays inside
        // the pill silhouette.
        if enabled && main_resp.hovered() {
            ui.painter().rect_filled(
                main_rect,
                egui::CornerRadius {
                    nw: 5,
                    sw: 5,
                    ne: 0,
                    se: 0,
                },
                palette::SURFACE_HOVER(),
            );
        }
        if chev_resp.hovered() {
            ui.painter().rect_filled(
                chev_rect,
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: 5,
                    se: 5,
                },
                palette::SURFACE_HOVER(),
            );
        }
        // Hairline between the two segments.
        ui.painter().vline(
            chev_rect.left(),
            rect.top() + 5.0..=rect.bottom() - 5.0,
            egui::Stroke::new(1.0, palette::BORDER()),
        );
        let text_pos = egui::pos2(
            main_rect.left() + pad_x,
            main_rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, text_color);
        // Chevron glyph: a small "v".
        let c = chev_rect.center();
        let r = 3.0;
        let stroke = egui::Stroke::new(1.3, palette::TEXT_WEAK());
        ui.painter().line_segment(
            [c + egui::vec2(-r, -r * 0.5), c + egui::vec2(0.0, r * 0.5)],
            stroke,
        );
        ui.painter().line_segment(
            [c + egui::vec2(0.0, r * 0.5), c + egui::vec2(r, -r * 0.5)],
            stroke,
        );
    }

    if enabled {
        out.clicked = main_resp.clicked();
        main_resp.on_hover_text(format!("Format the query for {dialect_label}"));
    }

    // The chevron stays active even with empty SQL so preferences remain reachable.
    egui::Popup::menu(&chev_resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(170.0);
            ui.label(
                egui::RichText::new(format!("Format for {dialect_label}"))
                    .small()
                    .color(palette::TEXT_FAINT()),
            );
            ui.separator();
            if ui
                .horizontal(|ui| {
                    crate::components::accent_checkbox(
                        ui,
                        true,
                        &mut prefs.uppercase,
                        Some("Uppercase keywords"),
                    )
                })
                .inner
                .changed()
            {
                out.prefs_changed = true;
            }
            ui.separator();
            for (width, label) in [(2u8, "Indent: 2 spaces"), (4u8, "Indent: 4 spaces")] {
                if ui
                    .horizontal(|ui| crate::components::accent_radio(ui, &mut prefs.indent, width, label))
                    .inner
                    .changed()
                {
                    out.prefs_changed = true;
                }
            }
        });

    out
}

/// Hairline separator between toolbar icon groups.
pub(crate) fn toolbar_sep(ui: &mut egui::Ui) {
    let h = 12.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(5.0, h), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let x = rect.center().x;
        ui.painter().vline(
            x,
            rect.top()..=rect.bottom(),
            egui::Stroke::new(1.0, palette::BORDER()),
        );
    }
}
