//! Labels, badges, and compact status marks.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Stroke};

use crate::style::palette;

pub(crate) fn truncated_label(
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

pub(crate) fn section_header(ui: &mut egui::Ui, text: &str) {
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

pub(crate) fn paint_table_header_cell(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    if !ui.is_rect_visible(rect) {
        return;
    }
    let t = crate::theme::current();
    ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, t.panel);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, t.border));
}

pub(crate) fn type_badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_uppercase(),
        FontId::new(9.0, FontFamily::Proportional),
        color,
    );
    let pad = egui::vec2(5.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
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

#[allow(dead_code)]
pub(crate) fn status_dot(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 3.5, color);
    }
    resp
}
