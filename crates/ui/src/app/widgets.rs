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
    let size = egui::vec2(40.0, 36.0);
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
                rect.left_top() + egui::vec2(5.0, 5.0),
                2.0,
                palette::SUCCESS(),
            );
        }
    }

    let content_rect = rect.shrink2(egui::vec2(3.0, 4.0));
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(content_rect.center().x, content_rect.top() + 8.0),
        egui::vec2(13.0, 13.0),
    );
    let label_rect = egui::Rect::from_min_size(
        egui::pos2(content_rect.left(), content_rect.top() + 18.0),
        egui::vec2(content_rect.width(), 11.0),
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(icon_rect), |ui| {
        icons::show_colored(
            ui,
            icons::database(),
            13.0,
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
                        .size(8.0)
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

/// Small layout toggle (sidebar on/off) used in the unified title bar.
pub(super) fn layout_toggle(
    ui: &mut egui::Ui,
    active: bool,
    side: LayoutSide,
    hover: &str,
) -> egui::Response {
    let size = egui::vec2(22.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let fill = if active {
        palette::SURFACE()
    } else if resp.hovered() {
        palette::SURFACE_HOVER()
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if active {
        egui::Stroke::new(1.0, palette::ACCENT())
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

        let icon = rect.shrink(4.0);
        let bar_w = 3.0;
        let gap = 1.5;
        let color = if active {
            palette::ACCENT()
        } else {
            palette::TEXT_WEAK()
        };

        match side {
            LayoutSide::Connections => {
                let left = egui::Rect::from_min_size(icon.min, egui::vec2(bar_w, icon.height()));
                ui.painter().rect_filled(left, egui::CornerRadius::same(1), color);
            }
            LayoutSide::Schema => {
                let left = egui::Rect::from_min_size(icon.min, egui::vec2(bar_w, icon.height()));
                let mid = egui::Rect::from_min_size(
                    egui::pos2(icon.min.x + bar_w + gap, icon.min.y),
                    egui::vec2(bar_w * 1.4, icon.height()),
                );
                ui.painter().rect_filled(left, egui::CornerRadius::same(1), color);
                ui.painter().rect_filled(mid, egui::CornerRadius::same(1), color);
            }
            LayoutSide::Details => {
                let right = egui::Rect::from_min_size(
                    egui::pos2(icon.max.x - bar_w, icon.min.y),
                    egui::vec2(bar_w, icon.height()),
                );
                ui.painter().rect_filled(right, egui::CornerRadius::same(1), color);
            }
        }
    }

    resp.on_hover_text(hover)
}

#[derive(Clone, Copy)]
pub(super) enum LayoutSide {
    Connections,
    Schema,
    Details,
}

pub(super) fn toolbar_icon_button(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    hover: &str,
) -> egui::Response {
    ui.add_sized(
        egui::vec2(26.0, 22.0),
        egui::Button::image(
            egui::Image::new(src)
                .fit_to_exact_size(egui::vec2(14.0, 14.0))
                .tint(palette::TEXT_WEAK()),
        )
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE),
    )
    .on_hover_text(hover)
}

/// Hairline separator between toolbar icon groups.
pub(super) fn toolbar_sep(ui: &mut egui::Ui) {
    let h = 14.0;
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
