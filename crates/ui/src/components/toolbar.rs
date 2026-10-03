//! Toolbar and title-bar controls.

use crate::icons;
use crate::style::palette;

const TOOLBAR_ICON_GAP: f32 = 0.0;
const LAYOUT_MENU_W: f32 = 230.0;
const LAYOUT_ROW_H: f32 = 28.0;

/// Visibility of the workspace chrome toggled from the title-bar Layout menu.
pub(crate) struct LayoutChrome<'a> {
    pub connections: &'a mut bool,
    pub schema: &'a mut bool,
    pub details: &'a mut bool,
    pub query: &'a mut bool,
    pub live_log: &'a mut bool,
}

/// One title-bar icon that opens the layout menu: a plain list of the workspace's panels, each
/// with its icon, its name and a check while it is shown — the way editors list their layout.
pub(crate) fn layout_menu(ui: &mut egui::Ui, chrome: &mut LayoutChrome<'_>) {
    let btn = super::soft_icon_button(ui, icons::layout_schema(), "Layout", true);
    ui.add_space(TOOLBAR_ICON_GAP);

    let popup_frame = egui::Frame::popup(ui.style())
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(6, 8));
    egui::Popup::from_toggle_button_response(&btn)
        .id(btn.id.with("layout_menu"))
        .align(egui::RectAlign::BOTTOM_END)
        .align_alternatives(&[])
        .gap(6.0)
        .width(LAYOUT_MENU_W)
        .frame(popup_frame)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(LAYOUT_MENU_W);
            ui.spacing_mut().item_spacing.y = 1.0;
            layout_section(ui, "Panels");
            layout_row(
                ui,
                icons::layout_schema(),
                "Schema",
                "Schema panel",
                chrome.schema,
            );
            layout_row(
                ui,
                icons::layout_details(),
                "Details",
                "Details panel",
                chrome.details,
            );
            layout_row(
                ui,
                icons::layout_connections(),
                "Connections",
                "Connection tabs",
                chrome.connections,
            );
            ui.add_space(6.0);
            layout_section(ui, "Editor");
            layout_row(
                ui,
                icons::layout_query(),
                "Query console",
                "Query console",
                chrome.query,
            );
            layout_row(
                ui,
                icons::layout_log(),
                "Live log",
                "Live log panel",
                chrome.live_log,
            );
        });
}

fn layout_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(title)
                .size(11.0)
                .color(palette::TEXT_FAINT()),
        );
    });
    ui.add_space(2.0);
}

/// One full-width row: icon, name, and a check on the right while the panel is shown.
fn layout_row(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    label: &str,
    a11y: &str,
    on: &mut bool,
) {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), LAYOUT_ROW_H),
        egui::Sense::click(),
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, a11y));
    if resp.clicked() {
        *on = !*on;
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(6),
            palette::SURFACE_HOVER(),
        );
    }
    let color = if *on {
        palette::TEXT()
    } else {
        palette::TEXT_WEAK()
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Vec2::splat(16.0),
    );
    egui::Image::new(icon)
        .fit_to_exact_size(icon_rect.size())
        .tint(color)
        .paint_at(ui, icon_rect);
    ui.painter().text(
        egui::pos2(rect.left() + 34.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        color,
    );
    if *on {
        solid_check(
            ui.painter(),
            egui::pos2(rect.right() - 16.0, rect.center().y),
            7.5,
        );
    }
}

/// A filled disc in the text colour with a check cut out of it in the panel colour — the solid
/// "on" mark, drawn rather than loaded because the icon set only has the outlined circle.
fn solid_check(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    painter.circle_filled(center, radius, palette::TEXT());
    let u = radius / 7.5;
    let at = |x: f32, y: f32| center + egui::vec2(x * u, y * u);
    painter.add(egui::Shape::line(
        vec![at(-3.3, 0.2), at(-0.9, 2.6), at(3.5, -2.4)],
        egui::Stroke::new(1.7 * u, palette::PANEL()),
    ));
}

/// Outline accent button for the title-bar update affordance.
pub(crate) fn update_outline_button(ui: &mut egui::Ui, label: &str, busy: bool) -> egui::Response {
    let accent = palette::ACCENT();
    let text = egui::RichText::new(label).color(accent).strong().size(11.0);
    let btn = egui::Button::new(text)
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0_f32, accent))
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
    let resp = super::soft_icon_button(ui, src, hover, true);

    ui.add_space(TOOLBAR_ICON_GAP);
    resp
}

#[derive(Default)]
pub(crate) struct RunResponse {
    pub run_current: bool,
    pub run_all: bool,
    pub save_query: bool,
    /// A newly selected main-button scope. `None` means the preference was unchanged.
    pub default_run_all: Option<bool>,
}

/// Split Run control whose main segment follows the selected default scope. The chevron exposes
/// both one-off run actions, the saved default, and query saving.
pub(crate) fn run_button(
    ui: &mut egui::Ui,
    can_run: bool,
    can_save: bool,
    run_all_by_default: bool,
) -> RunResponse {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_color = if can_run {
        palette::TEXT()
    } else {
        palette::TEXT_FAINT()
    };
    let mut job = egui::text::LayoutJob::default();
    let default_label = if run_all_by_default {
        "Run All"
    } else {
        "Run Current"
    };
    let run_current_shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND,
        egui::Key::Enter,
    ));
    let run_all_shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Enter,
    ));
    job.append(
        default_label,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: text_color,
            ..Default::default()
        },
    );
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let h = 24.0;
    let pad_x = 9.0;
    let chevron_w = 24.0;
    let main_w = galley.size().x + pad_x * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(main_w + chevron_w, h), egui::Sense::hover());
    let main_rect = egui::Rect::from_min_size(rect.min, egui::vec2(main_w, h));
    let chevron_rect = egui::Rect::from_min_size(
        egui::pos2(main_rect.right(), rect.top()),
        egui::vec2(chevron_w, h),
    );
    let main = ui.interact(main_rect, ui.id().with("run_default"), egui::Sense::click());
    main.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, can_run, default_label)
    });
    let chevron = ui.interact(
        chevron_rect,
        ui.id().with("run_options"),
        egui::Sense::click(),
    );
    chevron
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Run options"));

    if ui.is_rect_visible(rect) {
        let radius = egui::CornerRadius::same(5);
        ui.painter().rect(
            rect,
            radius,
            palette::SURFACE(),
            egui::Stroke::new(1.0_f32, palette::BORDER()),
            egui::StrokeKind::Outside,
        );
        if can_run && main.hovered() {
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
        if chevron.hovered() {
            ui.painter().rect_filled(
                chevron_rect,
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: 5,
                    se: 5,
                },
                palette::SURFACE_HOVER(),
            );
        }
        ui.painter().vline(
            chevron_rect.left(),
            rect.top() + 5.0..=rect.bottom() - 5.0,
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );
        ui.painter().galley(
            egui::pos2(
                main_rect.left() + pad_x,
                main_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            text_color,
        );
        egui::Image::new(icons::chevron_down())
            .fit_to_exact_size(egui::Vec2::splat(12.0))
            .tint(palette::TEXT_WEAK())
            .paint_at(
                ui,
                egui::Rect::from_center_size(chevron_rect.center(), egui::Vec2::splat(12.0)),
            );
    }

    let mut out = RunResponse {
        run_current: can_run && main.clicked() && !run_all_by_default,
        run_all: can_run && main.clicked() && run_all_by_default,
        ..Default::default()
    };
    egui::Popup::menu(&chevron)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(164.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            if run_menu_item(ui, "Run All", can_run, Some(&run_all_shortcut)) {
                out.run_all = true;
                ui.close();
            }
            if run_menu_item(ui, "Run Current", can_run, Some(&run_current_shortcut)) {
                out.run_current = true;
                ui.close();
            }
            ui.separator();
            run_submenu(ui, "Default run", |ui| {
                ui.set_width(152.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                if run_default_menu_item(ui, "Run Current", !run_all_by_default) {
                    out.default_run_all = Some(false);
                    ui.close();
                }
                if run_default_menu_item(ui, "Run All", run_all_by_default) {
                    out.default_run_all = Some(true);
                    ui.close();
                }
            });
            if run_menu_item(ui, "Save query", can_save, None) {
                out.save_query = true;
                ui.close();
            }
        });
    out
}

/// A default-scope row inside the Default run submenu. The reserved check column keeps both
/// choices aligned whether selected or not.
fn run_default_menu_item(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label)
    });
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), palette::SELECTION());
        }
        if selected {
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                "✓",
                egui::TextStyle::Button.resolve(ui.style()),
                palette::ACCENT(),
            );
        }
        ui.painter().text(
            egui::pos2(rect.left() + 29.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            palette::TEXT(),
        );
    }
    response.clicked()
}

/// A submenu row painted with the same inset as the surrounding custom run-menu rows.
fn run_submenu<R>(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui) -> R) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), palette::SELECTION());
        }
        ui.painter().text(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            palette::TEXT(),
        );
        ui.painter().text(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "⏵",
            egui::TextStyle::Button.resolve(ui.style()),
            palette::TEXT_WEAK(),
        );
    }
    let _ = egui::containers::menu::SubMenu::new().show(ui, &response, add_contents);
}

fn run_menu_item(ui: &mut egui::Ui, label: &str, enabled: bool, shortcut: Option<&str>) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        if enabled && response.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(5), palette::SELECTION());
        }
        let color = if enabled {
            palette::TEXT()
        } else {
            palette::TEXT_FAINT()
        };
        ui.painter().text(
            egui::pos2(rect.left() + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
        if let Some(shortcut) = shortcut {
            ui.painter().text(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                shortcut,
                egui::TextStyle::Body.resolve(ui.style()),
                palette::TEXT_FAINT(),
            );
        }
    }
    enabled && response.clicked()
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
            egui::Stroke::new(1.0_f32, palette::BORDER()),
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
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );
        let text_pos = egui::pos2(
            main_rect.left() + pad_x,
            main_rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, text_color);
        egui::Image::new(icons::chevron_down())
            .fit_to_exact_size(egui::Vec2::splat(12.0))
            .tint(palette::TEXT_WEAK())
            .paint_at(
                ui,
                egui::Rect::from_center_size(chev_rect.center(), egui::Vec2::splat(12.0)),
            );
    }

    if enabled {
        out.clicked = main_resp.clicked();
        main_resp.on_hover_text(format!("Format the query for {dialect_label}"));
    }

    // The chevron stays active even with empty SQL so preferences remain reachable.
    // Framed like the app's other popovers (the pager's Limit/Offset one): panel fill, a
    // strong border, room inside, and a small gap below the button.
    egui::Popup::menu(&chev_resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(6.0)
        .frame(
            egui::Frame::popup(ui.style())
                .fill(palette::PANEL())
                .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(10)),
        )
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
                    .horizontal(|ui| {
                        crate::components::accent_radio(ui, &mut prefs.indent, width, label)
                    })
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
#[allow(dead_code)]
pub(crate) fn toolbar_sep(ui: &mut egui::Ui) {
    let h = 12.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(5.0, h), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let x = rect.center().x;
        ui.painter().vline(
            x,
            rect.top()..=rect.bottom(),
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );
    }
}
