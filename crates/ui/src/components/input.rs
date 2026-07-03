//! Form controls and compact pickers.

use dbcore::DbKind;
use egui::{Color32, CornerRadius, FontFamily, FontId, ImageSource, Margin, Pos2, Stroke, Vec2};

use crate::icons;
use crate::style::{palette, CONTROL_H};

pub(crate) fn text_input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    ui.add_sized(
        egui::vec2(width, CONTROL_H),
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .vertical_align(egui::Align::Center)
            .margin(Margin::symmetric(6, 0)),
    )
}

pub(crate) fn icon_text_input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    icon: ImageSource<'static>,
    width: f32,
) -> egui::Response {
    const ICON: f32 = 14.0;
    let img = egui::Image::new(icon)
        .fit_to_exact_size(egui::vec2(ICON, ICON))
        .tint(palette::TEXT_FAINT());
    ui.add_sized(
        egui::vec2(width, CONTROL_H),
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .prefix((img, " "))
            .vertical_align(egui::Align::Center)
            .margin(Margin::symmetric(6, 0)),
    )
}

pub(crate) fn accent_checkbox(
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

pub(crate) fn accent_radio<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    label: &str,
) -> egui::Response {
    const SIZE: f32 = 16.0;

    let selected = *current == value;
    let sense = egui::Sense::click();
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), sense);

    let label_resp = ui.add(
        egui::Label::new(egui::RichText::new(label).size(11.5).color(palette::TEXT_WEAK()))
            .sense(sense),
    );

    if (resp.clicked() || label_resp.clicked()) && !selected {
        *current = value;
        resp.mark_changed();
    }

    if ui.is_rect_visible(rect) {
        let accent = palette::ACCENT();
        let center = rect.center();
        let outer_r = SIZE * 0.5 - 0.5;
        if selected {
            ui.painter().circle(
                center,
                outer_r,
                accent.linear_multiply(0.12),
                Stroke::new(1.5, accent),
            );
            ui.painter().circle_filled(center, outer_r * 0.42, accent);
        } else {
            let (fill, border) = if resp.hovered() {
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
            ui.painter().circle(center, outer_r, fill, border);
        }
    }

    resp
}

pub(crate) fn segmented(
    ui: &mut egui::Ui,
    items: &[(ImageSource<'_>, &str)],
    selected: usize,
) -> usize {
    let n = items.len().max(1);
    let height = 28.0;
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), palette::SURFACE());
    }
    let seg_w = rect.width() / n as f32;
    let mut result = selected;
    for (i, (icon, label)) in items.iter().enumerate() {
        let seg = egui::Rect::from_min_size(
            Pos2::new(rect.min.x + seg_w * i as f32, rect.min.y),
            Vec2::new(seg_w, height),
        );
        let resp = ui.interact(
            seg,
            ui.make_persistent_id(("segmented", i, *label)),
            egui::Sense::click(),
        );
        let active = i == selected;
        if ui.is_rect_visible(seg) {
            if active {
                ui.painter()
                    .rect_filled(seg.shrink(3.0), CornerRadius::same(6), palette::SELECTION());
            } else if resp.hovered() {
                ui.painter().rect_filled(
                    seg.shrink(3.0),
                    CornerRadius::same(6),
                    palette::SURFACE_HOVER(),
                );
            }
            let color = if active {
                palette::TEXT()
            } else {
                palette::TEXT_WEAK()
            };
            let icon_sz = 14.0;
            let gap = 6.0;
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                FontId::new(12.0, FontFamily::Proportional),
                color,
            );
            let group_w = icon_sz + gap + galley.size().x;
            let start_x = seg.center().x - group_w / 2.0;
            let icon_rect = egui::Rect::from_min_size(
                Pos2::new(start_x, seg.center().y - icon_sz / 2.0),
                Vec2::splat(icon_sz),
            );
            egui::Image::new(icon.clone()).tint(color).paint_at(ui, icon_rect);
            ui.painter().galley(
                Pos2::new(icon_rect.max.x + gap, seg.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
        if resp.clicked() {
            result = i;
        }
    }
    result
}

fn db_kind_button_image(kind: DbKind) -> egui::Image<'static> {
    egui::Image::new(icons::db_kind_icon(kind)).fit_to_exact_size(egui::vec2(
        icons::DB_KIND_ICON_SIZE,
        icons::DB_KIND_ICON_SIZE,
    ))
}

pub(crate) fn db_kind_selectable(
    ui: &mut egui::Ui,
    current: &mut DbKind,
    kind: DbKind,
) -> egui::Response {
    let selected = *current == kind;
    let btn = egui::Button::image_and_text(db_kind_button_image(kind), kind.label())
        .selected(selected)
        .frame_when_inactive(selected)
        .frame(true)
        .min_size(egui::vec2(ui.available_width(), 0.0));

    let mut response = ui.add(btn);
    if response.clicked() && !selected {
        *current = kind;
        response.mark_changed();
    }
    response
}

pub(crate) fn db_kind_combo(
    ui: &mut egui::Ui,
    current: &mut DbKind,
    id: impl std::hash::Hash,
    width: f32,
) -> egui::Response {
    let btn = egui::Button::image_and_text(db_kind_button_image(*current), current.label())
        .right_text(egui::RichText::new("▾").size(10.0))
        .min_size(egui::vec2(width, 0.0));
    let button_response = ui.add(btn);

    egui::Popup::menu(&button_response)
        .id(egui::Id::new(id).with("popup"))
        .width(button_response.rect.width())
        .show(|ui| {
            ui.set_min_width(ui.available_width());
            for kind in [
                DbKind::Postgres,
                DbKind::MySql,
                DbKind::MariaDb,
                DbKind::SqlServer,
                DbKind::Sqlite,
            ] {
                db_kind_selectable(ui, current, kind);
            }
        });

    button_response
}
