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

pub(super) fn connection_tab_item(
    ui: &mut egui::Ui,
    name: &str,
    selected: bool,
    connected: bool,
) -> egui::Response {
    let size = egui::vec2(56.0, 44.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let fill = if selected {
        palette::SURFACE()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if selected {
        egui::Stroke::new(1.0, palette::BORDER_STRONG())
    } else if resp.hovered() {
        egui::Stroke::new(1.0, palette::BORDER())
    } else {
        egui::Stroke::NONE
    };

    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(4),
            fill,
            stroke,
            egui::StrokeKind::Outside,
        );
        if connected {
            ui.painter().circle_filled(
                rect.left_top() + egui::vec2(7.0, 7.0),
                2.5,
                palette::SUCCESS(),
            );
        }
    }

    let content_rect = rect.shrink2(egui::vec2(3.0, 4.0));
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(content_rect.center().x, content_rect.top() + 10.0),
        egui::vec2(16.0, 16.0),
    );
    let label_rect = egui::Rect::from_min_size(
        egui::pos2(content_rect.left(), content_rect.top() + 22.0),
        egui::vec2(content_rect.width(), 14.0),
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(icon_rect), |ui| {
        icons::show_colored(
            ui,
            icons::database(),
            16.0,
            if selected {
                palette::TEXT()
            } else {
                palette::TEXT_WEAK()
            },
        );
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(label_rect), |ui| {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(compact_connection_label(name))
                        .size(9.0)
                        .color(if selected {
                            palette::TEXT()
                        } else {
                            palette::TEXT_WEAK()
                        }),
                )
                .selectable(false),
            );
        });
    });

    resp
}

pub(super) fn toolbar_icon_button(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    hover: &str,
) -> egui::Response {
    ui.add_sized(
        egui::vec2(32.0, 28.0),
        egui::Button::image(
            egui::Image::new(src)
                .fit_to_exact_size(egui::vec2(17.0, 17.0))
                .tint(palette::TEXT_WEAK()),
        )
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE),
    )
    .on_hover_text(hover)
}
