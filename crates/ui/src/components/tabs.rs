use dbcore::{ConnectionIcon, DbKind};

use crate::icons;
use crate::style::palette;

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

pub(crate) fn connection_tab_item(
    ui: &mut egui::Ui,
    name: &str,
    icon: ConnectionIcon,
    selected: bool,
    connected: bool,
    drag_float_y: Option<f32>,
) -> egui::Response {
    const CONN_ICON_SIZE: f32 = 16.0;

    #[allow(clippy::too_many_arguments)]
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

    let colors = super::interaction_colors(&resp, selected, dragging);
    let fill = colors.fill;
    let stroke = colors.stroke;
    let icon_color = colors.text;
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
pub(crate) struct QueryTabResponse {
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

/// Whether a query-tab chip represents a plain SQL editor or a relation opened from the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryTabKind {
    Query,
    Table,
    View,
    Function,
    Procedure,
    Trigger,
}

impl QueryTabKind {
    pub(crate) fn color(self) -> egui::Color32 {
        match self {
            Self::Query => palette::WARNING(),
            Self::Table => palette::ACCENT(),
            Self::View => palette::SUCCESS(),
            Self::Function => crate::style::mix(palette::ACCENT(), palette::DANGER(), 0.45),
            Self::Procedure => palette::WARNING(),
            Self::Trigger => palette::DANGER(),
        }
    }

    pub(crate) fn icon(self) -> egui::ImageSource<'static> {
        match self {
            Self::Table => icons::table(),
            Self::View => icons::view(),
            Self::Function | Self::Procedure | Self::Query => icons::code(),
            Self::Trigger => icons::play(),
        }
    }
}

const TAB_ICON_SIZE: f32 = 13.0;
const TAB_ICON_GAP: f32 = 6.0;

fn tab_icon_color(kind: QueryTabKind, selected: bool) -> egui::Color32 {
    let color = kind.color();
    if selected {
        color
    } else {
        color.linear_multiply(0.78)
    }
}

fn translucent(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Paint one tab chip (background pill, kind icon, title, × glyph). Shared by the in-strip
/// chip and its pointer-following twin during drag-to-reorder.
#[allow(clippy::too_many_arguments)]
fn paint_tab_chip(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: QueryTabKind,
    db_kind: Option<DbKind>,
    galley: &std::sync::Arc<egui::Galley>,
    fill: egui::Color32,
    stroke: egui::Stroke,
    text_color: egui::Color32,
    close_color: egui::Color32,
    pad: f32,
    close_w: f32,
    selected: bool,
    close_hovered: bool,
) {
    let rounding = egui::CornerRadius {
        nw: 4,
        ne: 4,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(rect, rounding, fill);
    if stroke != egui::Stroke::NONE {
        painter.hline(rect.x_range(), rect.top(), stroke);
    }
    if selected {
        painter.hline(
            rect.x_range(),
            rect.top(),
            egui::Stroke::new(1.0, palette::ACCENT()),
        );
    }

    let icon_color = tab_icon_color(kind, selected);
    let badge_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + pad + 7.0, rect.center().y),
        egui::vec2(16.0, 16.0),
    );
    if db_kind.is_none() {
        painter.rect_filled(
            badge_rect,
            egui::CornerRadius::same(4),
            translucent(icon_color, if selected { 34 } else { 20 }),
        );
    }
    let icon_rect = egui::Rect::from_center_size(
        badge_rect.center(),
        egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE),
    );
    if let Some(db_kind) = db_kind {
        egui::Image::new(icons::db_kind_icon(db_kind))
            .fit_to_exact_size(icon_rect.size())
            .tint(icons::db_kind_icon_tint(db_kind))
            .paint_at(ui, icon_rect);
    } else {
        egui::Image::new(kind.icon())
            .fit_to_exact_size(icon_rect.size())
            .tint(icon_color)
            .paint_at(ui, icon_rect);
    }
    let text_x = badge_rect.right() + TAB_ICON_GAP;
    let pos = egui::pos2(text_x, rect.center().y - galley.size().y * 0.5);
    painter.galley(pos, galley.clone(), text_color);

    let c = egui::pos2(rect.right() - pad - close_w * 0.5, rect.center().y);
    if close_hovered {
        painter.circle_filled(c, 7.0, translucent(palette::DANGER(), 28));
    }
    let r = 3.25;
    let s = egui::Stroke::new(1.4, close_color);
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
pub(crate) fn query_tab_item(
    ui: &mut egui::Ui,
    title: &str,
    kind: QueryTabKind,
    db_kind: Option<DbKind>,
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
    let close_w = 16.0;
    let pad = 9.0;
    let size = egui::vec2(
        pad + 16.0 + TAB_ICON_GAP + text_w + 8.0 + close_w + pad,
        29.0,
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
        palette::PANEL()
    };
    let stroke = if dragging {
        egui::Stroke::new(1.0, palette::ACCENT())
    } else {
        egui::Stroke::NONE
    };
    let close_color = if close_resp.hovered() && !dragging {
        palette::DANGER()
    } else if selected || resp.hovered() {
        palette::TEXT_WEAK()
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
                        db_kind,
                        &galley,
                        fill,
                        stroke,
                        color,
                        close_color,
                        pad,
                        close_w,
                        selected,
                        close_resp.hovered() && !dragging,
                    );
                });
        } else {
            paint_tab_chip(
                ui,
                ui.painter(),
                rect,
                kind,
                db_kind,
                &galley,
                fill,
                stroke,
                color,
                close_color,
                pad,
                close_w,
                selected,
                close_resp.hovered() && !dragging,
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
