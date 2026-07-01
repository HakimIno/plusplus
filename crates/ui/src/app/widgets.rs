use dbcore::ConnectionIcon;

use crate::icons;
use crate::style::palette;

/// Shared footprint for every button in the top title bar (icon buttons and layout toggles)
/// so they line up at a uniform size.
const TOOLBAR_BUTTON_SIZE: egui::Vec2 = egui::vec2(22.0, 22.0);
/// Breathing room between adjacent title-bar icon buttons.
const TOOLBAR_ICON_GAP: f32 = 0.0;

fn compact_connection_label(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "DB".to_string();
    }
    let mut label: String = trimmed.chars().take(8).collect();
    if trimmed.chars().count() > 8 {
        label.push('…');
    }
    label
}

pub(super) fn connection_tab_item(
    ui: &mut egui::Ui,
    name: &str,
    icon: ConnectionIcon,
    selected: bool,
    connected: bool,
    drag_float_y: Option<f32>,
) -> egui::Response {
    const CONN_ICON_SIZE: f32 = 16.0;

    fn paint_connection_chip(
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        icon: ConnectionIcon,
        label: &std::sync::Arc<egui::Galley>,
        fill: egui::Color32,
        stroke: egui::Stroke,
        icon_color: egui::Color32,
        text_color: egui::Color32,
        connected: bool,
    ) {
        painter.rect(
            rect,
            egui::CornerRadius::same(4),
            fill,
            stroke,
            egui::StrokeKind::Outside,
        );
        if connected {
            painter.circle_filled(
                rect.left_top() + egui::vec2(5.0, 5.0),
                2.0,
                palette::SUCCESS(),
            );
        }
        let content_rect = rect.shrink2(egui::vec2(3.0, 4.0));
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(content_rect.center().x, content_rect.top() + 8.0),
            egui::vec2(CONN_ICON_SIZE, CONN_ICON_SIZE),
        );
        icons::paint_connection_icon(ui, icon, icon_rect, icon_color);
        let label_pos = egui::pos2(
            content_rect.center().x - label.size().x * 0.5,
            content_rect.top() + 18.0,
        );
        painter.galley(label_pos, label.clone(), text_color);
    }

    let size = egui::vec2(40.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let dragging = drag_float_y.is_some();

    let fill = if selected {
        palette::SURFACE()
    } else if dragging {
        palette::SURFACE()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if dragging {
        egui::Stroke::new(1.0, palette::ACCENT())
    } else if selected {
        egui::Stroke::new(1.0, palette::BORDER_STRONG())
    } else if resp.hovered() {
        egui::Stroke::new(1.0, palette::BORDER())
    } else {
        egui::Stroke::NONE
    };
    let icon_color = if selected {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    let text_color = icon_color;
    let label = ui
        .painter()
        .layout_job(egui::text::LayoutJob::single_section(
            compact_connection_label(name),
            egui::TextFormat {
                font_id: egui::FontId::proportional(8.0),
                color: text_color,
                ..Default::default()
            },
        ));

    if ui.is_rect_visible(rect) {
        if let Some(float_y) = drag_float_y {
            ui.painter().rect(
                rect,
                egui::CornerRadius::same(4),
                palette::SURFACE_HOVER(),
                egui::Stroke::new(1.0, palette::BORDER()),
                egui::StrokeKind::Outside,
            );
            let float_rect =
                egui::Rect::from_min_size(egui::pos2(rect.left(), float_y), rect.size());
            let float_painter = egui::Painter::new(
                ui.ctx().clone(),
                egui::LayerId::new(egui::Order::Tooltip, resp.id.with("float")),
                egui::Rect::EVERYTHING,
            );
            paint_connection_chip(
                ui,
                &float_painter,
                float_rect,
                icon,
                &label,
                fill,
                stroke,
                icon_color,
                text_color,
                connected,
            );
        } else {
            paint_connection_chip(
                ui,
                ui.painter(),
                rect,
                icon,
                &label,
                fill,
                stroke,
                icon_color,
                text_color,
                connected,
            );
        }
    }

    resp
}

/// Outcome of interacting with a horizontal query-tab chip.
pub(super) struct QueryTabResponse {
    /// The tab body was clicked (select it).
    pub clicked: bool,
    /// The tab body was double-clicked (pin a preview tab as permanent).
    pub pinned: bool,
    /// The × close affordance was clicked.
    pub close: bool,
    /// A drag on the tab body just crossed egui's drag threshold (start reordering).
    pub drag_started: bool,
    /// The chip's rect this frame, so the drag handler can map the pointer to a slot.
    pub rect: egui::Rect,
    /// Full egui response for context menus and secondary interactions.
    pub response: egui::Response,
}

/// Whether a query-tab chip represents a plain SQL editor or a table opened from the schema.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryTabKind {
    Query,
    Table,
}

const TAB_ICON_SIZE: f32 = 12.0;
const TAB_ICON_GAP: f32 = 4.0;

/// Paint one tab chip (background pill, kind icon, title, × glyph). Shared by the in-strip
/// chip and its pointer-following twin during drag-to-reorder.
#[allow(clippy::too_many_arguments)]
fn paint_tab_chip(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: QueryTabKind,
    galley: &std::sync::Arc<egui::Galley>,
    fill: egui::Color32,
    stroke: egui::Stroke,
    text_color: egui::Color32,
    close_color: egui::Color32,
    pad: f32,
    close_w: f32,
) {
    painter.rect(
        rect,
        egui::CornerRadius::same(4),
        fill,
        stroke,
        egui::StrokeKind::Outside,
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + pad + TAB_ICON_SIZE * 0.5,
            rect.center().y,
        ),
        egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE),
    );
    let icon_src = match kind {
        QueryTabKind::Table => icons::table(),
        QueryTabKind::Query => icons::code(),
    };
    let img = egui::Image::new(icon_src).fit_to_exact_size(icon_rect.size());
    if matches!(kind, QueryTabKind::Table) {
        img.paint_at(ui, icon_rect);
    } else {
        img.tint(text_color).paint_at(ui, icon_rect);
    }
    let text_x = rect.left() + pad + TAB_ICON_SIZE + TAB_ICON_GAP;
    let pos = egui::pos2(text_x, rect.center().y - galley.size().y * 0.5);
    painter.galley(pos, galley.clone(), text_color);
    let c = egui::pos2(rect.right() - pad - close_w * 0.5, rect.center().y);
    let r = 3.5;
    let s = egui::Stroke::new(1.3, close_color);
    painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], s);
    painter.line_segment([c + egui::vec2(r, -r), c + egui::vec2(-r, r)], s);
}

/// A horizontal query-tab chip: title + a × close button. Mirrors the visual language of
/// [`connection_tab_item`] but laid out left-to-right for the tab strip above the editor.
/// `preview` tabs render in italics (transient, like other editors' preview tabs).
///
/// `drag_float_x` is set while this tab is being drag-reordered: the slot renders as an
/// empty placeholder and the chip itself is painted on a foreground layer with its left
/// edge at that x, following the pointer (same technique as egui's drag-and-drop demo).
pub(super) fn query_tab_item(
    ui: &mut egui::Ui,
    title: &str,
    kind: QueryTabKind,
    selected: bool,
    preview: bool,
    drag_float_x: Option<f32>,
) -> QueryTabResponse {
    let label: String = {
        let trimmed = title.trim();
        let name = if trimmed.is_empty() {
            "Untitled"
        } else {
            trimmed
        };
        let mut s: String = name.chars().take(18).collect();
        if name.chars().count() > 18 {
            s.push('…');
        }
        s
    };
    let dragging = drag_float_x.is_some();

    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = if selected {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    let galley = ui
        .painter()
        .layout_job(egui::text::LayoutJob::single_section(
            label,
            egui::TextFormat {
                font_id: font,
                color,
                italics: preview,
                ..Default::default()
            },
        ));
    let text_w = galley.size().x;
    let close_w = 14.0;
    let pad = 8.0;
    let size = egui::vec2(
        pad + TAB_ICON_SIZE + TAB_ICON_GAP + text_w + 4.0 + close_w + pad,
        26.0,
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    // The × hit area sits at the right edge; its own hover/click is tested separately so a
    // click there closes rather than selects. Inert while the tab floats mid-drag.
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - pad - close_w * 0.5, rect.center().y),
        egui::vec2(close_w, close_w),
    );
    let close_resp = ui.interact(
        close_rect,
        resp.id.with("close"),
        if dragging {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        },
    );

    let fill = if selected || dragging {
        palette::SURFACE()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if dragging {
        egui::Stroke::new(1.0, palette::ACCENT())
    } else if selected {
        egui::Stroke::new(1.0, palette::BORDER_STRONG())
    } else if resp.hovered() {
        egui::Stroke::new(1.0, palette::BORDER())
    } else {
        egui::Stroke::NONE
    };
    let close_color = if close_resp.hovered() && !dragging {
        palette::DANGER()
    } else {
        palette::TEXT_FAINT()
    };

    if ui.is_rect_visible(rect) {
        if let Some(float_x) = drag_float_x {
            // Empty-slot placeholder marking where the tab will land.
            ui.painter().rect(
                rect,
                egui::CornerRadius::same(4),
                palette::SURFACE_HOVER(),
                egui::Stroke::new(1.0, palette::BORDER()),
                egui::StrokeKind::Outside,
            );
            // The chip itself follows the pointer on a foreground layer (above panel
            // borders and neighbouring chips), Chrome/TablePlus-style. An `Area` is used
            // (not `scope_builder`) so the floating chip never advances the tab strip's
            // layout cursor — that would corrupt the neighbouring chips' rects and break
            // the drag-to-reorder hit-testing.
            let float_rect =
                egui::Rect::from_min_size(egui::pos2(float_x, rect.top()), rect.size());
            egui::Area::new(resp.id.with("float"))
                .order(egui::Order::Tooltip)
                .fixed_pos(float_rect.min)
                .show(ui.ctx(), |ui| {
                    ui.set_min_size(float_rect.size());
                    paint_tab_chip(
                        ui,
                        ui.painter(),
                        float_rect,
                        kind,
                        &galley,
                        fill,
                        stroke,
                        color,
                        close_color,
                        pad,
                        close_w,
                    );
                });
        } else {
            paint_tab_chip(
                ui,
                ui.painter(),
                rect,
                kind,
                &galley,
                fill,
                stroke,
                color,
                close_color,
                pad,
                close_w,
            );
        }
    }

    QueryTabResponse {
        clicked: resp.clicked() && !close_resp.hovered(),
        pinned: resp.double_clicked() && !close_resp.hovered(),
        close: close_resp.clicked() && !dragging,
        drag_started: resp.drag_started() && !close_resp.hovered(),
        rect,
        response: resp,
    }
}

/// Small layout toggle (sidebar on/off) used in the unified title bar.
pub(super) fn layout_toggle(
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
pub(super) enum LayoutSide {
    Connections,
    Schema,
    Details,
    Query,
}

/// Outline accent button for the title-bar update affordance.
pub(super) fn update_outline_button(ui: &mut egui::Ui, label: &str, busy: bool) -> egui::Response {
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

pub(super) fn toolbar_icon_button(
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
pub(super) struct BeautifyResponse {
    /// The main segment was clicked: format the active tab's SQL.
    pub clicked: bool,
    /// A preference in the dropdown changed: persist settings.
    pub prefs_changed: bool,
}

/// The query console's "Beautify ⌘I ⌄" split button (TablePlus-style): the main segment
/// reformats the SQL in the active connection's dialect, the chevron opens formatting
/// preferences. Painted as one pill with an internal hairline so the two hit areas read
/// as a single control.
pub(super) fn beautify_button(
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
                .checkbox(&mut prefs.uppercase, "Uppercase keywords")
                .changed()
            {
                out.prefs_changed = true;
            }
            ui.separator();
            for (width, label) in [(2u8, "Indent: 2 spaces"), (4u8, "Indent: 4 spaces")] {
                if ui.radio_value(&mut prefs.indent, width, label).changed() {
                    out.prefs_changed = true;
                }
            }
        });

    out
}

/// Hairline separator between toolbar icon groups.
pub(super) fn toolbar_sep(ui: &mut egui::Ui) {
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
