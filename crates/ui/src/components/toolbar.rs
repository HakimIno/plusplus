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
    // A neutral, softly filled pill (a native "secondary" button) rather than an accent
    // outline: noticeable in the title bar without competing with the window's own colour.
    // Derived from the theme so it reads on light and dark title bars alike.
    let base = palette::PANEL();
    let ink = palette::TEXT();
    let id = ui.next_auto_id();
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered()) && !busy;
    let fill = crate::style::mix(base, ink, if hovered { 0.34 } else { 0.27 });
    let text = egui::RichText::new(label)
        .color(crate::style::mix(base, ink, 0.9))
        .size(12.0);
    let btn = egui::Button::new(text)
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, crate::style::mix(base, ink, 0.42)))
        .corner_radius(egui::CornerRadius::same(6))
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
        .gap(6.0)
        .frame(menu_popup_frame(ui.style()))
        .show(|ui| {
            ui.set_width(220.0);
            style_menu_submenus(ui);
            if menu_item(ui, "Run All", Some(&run_all_shortcut), can_run).clicked() {
                out.run_all = true;
                ui.close();
            }
            if menu_item(ui, "Run Current", Some(&run_current_shortcut), can_run).clicked() {
                out.run_current = true;
                ui.close();
            }
            ui.separator();
            menu_submenu(ui, "Default run", |ui| {
                ui.set_width(170.0);
                if super::menu_radio(ui, !run_all_by_default, "Run Current").clicked() {
                    out.default_run_all = Some(false);
                    ui.close();
                }
                if super::menu_radio(ui, run_all_by_default, "Run All").clicked() {
                    out.default_run_all = Some(true);
                    ui.close();
                }
            });
            if menu_item(ui, "Save query", None, can_save).clicked() {
                out.save_query = true;
                ui.close();
            }
        });
    out
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
    egui::Popup::menu(&chev_resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(6.0)
        .frame(menu_popup_frame(ui.style()))
        .show(|ui| {
            ui.set_width(200.0);
            style_menu_submenus(ui);
            if super::menu_checkbox(ui, &mut prefs.uppercase, "Uppercase keywords").changed() {
                out.prefs_changed = true;
            }
            ui.separator();
            for (width, label) in [(2u8, "Indent: 2 spaces"), (4u8, "Indent: 4 spaces")] {
                if super::menu_radio(ui, prefs.indent == width, label).clicked()
                    && prefs.indent != width
                {
                    prefs.indent = width;
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

/// The frame shared by the toolbar's dropdowns — Editor options, Beautify and Run — so they
/// read as one family: panel fill, a strong hairline, generous rounding, room inside.
pub(crate) fn menu_popup_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style)
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(6))
}

/// Make submenus opened from `ui` use [`menu_popup_frame`] too: egui frames a submenu from the
/// style of the menu that opens it.
pub(crate) fn style_menu_submenus(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals.window_fill = palette::PANEL();
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG());
    style.visuals.menu_corner_radius = egui::CornerRadius::same(10);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.item_spacing.y = 2.0;
}

/// A plain row in a dropdown menu, its label lined up with the labels of
/// [`super::menu_checkbox`] rows, and an optional shortcut hint on the right.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: Option<&str>,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), super::MENU_ROW_H), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        paint_menu_row(ui, rect, label, enabled, response.hovered());
        if let Some(shortcut) = shortcut {
            ui.painter().text(
                egui::pos2(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                shortcut,
                egui::TextStyle::Body.resolve(ui.style()),
                palette::TEXT_FAINT(),
            );
        }
    }
    response
}

/// A row that opens `add_contents` as a submenu on hover, marked with a chevron.
pub(crate) fn menu_submenu<R>(
    ui: &mut egui::Ui,
    label: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), super::MENU_ROW_H),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let open = egui::Popup::is_id_open(
        ui.ctx(),
        egui::containers::menu::SubMenu::id_from_widget_id(response.id),
    );
    if ui.is_rect_visible(rect) {
        paint_menu_row(ui, rect, label, true, response.hovered() || open);
        egui::Image::new(icons::chevron_right())
            .fit_to_exact_size(egui::Vec2::splat(12.0))
            .tint(palette::TEXT_WEAK())
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 14.0, rect.center().y),
                    egui::Vec2::splat(12.0),
                ),
            );
    }
    let _ = egui::containers::menu::SubMenu::new().show(ui, &response, |ui| {
        style_menu_submenus(ui);
        add_contents(ui)
    });
}

/// Hover wash and label of a menu row. The label starts where a checkbox row's does, so a
/// menu mixing the two reads as one column.
fn paint_menu_row(ui: &egui::Ui, rect: egui::Rect, label: &str, enabled: bool, hot: bool) {
    if enabled && hot {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), palette::SURFACE_HOVER());
    }
    let color = if enabled {
        palette::TEXT()
    } else {
        palette::TEXT_FAINT()
    };
    ui.painter().text(
        egui::pos2(rect.left() + MENU_LABEL_X, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        color,
    );
}

/// Where a menu row's label starts: past the 8pt inset, the 16pt box and its 10pt gap.
const MENU_LABEL_X: f32 = 34.0;
