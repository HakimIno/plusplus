//! Toolbar and title-bar controls.

use crate::icons;
use crate::style::palette;

const TOOLBAR_ICON_GAP: f32 = 0.0;
const LAYOUT_TILE: egui::Vec2 = egui::vec2(46.0, 34.0);
const LAYOUT_GAP: f32 = 8.0;

fn layout_grid_width() -> f32 {
    LAYOUT_TILE.x * 3.0 + LAYOUT_GAP * 2.0
}

/// Visibility of the workspace chrome toggled from the title-bar Layout menu.
pub(crate) struct LayoutChrome<'a> {
    pub connections: &'a mut bool,
    pub schema: &'a mut bool,
    pub details: &'a mut bool,
    pub query: &'a mut bool,
    pub live_log: &'a mut bool,
}

/// One title-bar icon that opens a layout popover: a macOS-style grid of panel glyphs.
pub(crate) fn layout_menu(ui: &mut egui::Ui, chrome: &mut LayoutChrome<'_>) {
    let btn = super::soft_icon_button(ui, icons::layout_schema(), "Layout", true);
    ui.add_space(TOOLBAR_ICON_GAP);

    let popup_id = btn.id.with("layout_menu");
    let grid_w = layout_grid_width();
    let popup_frame = egui::Frame::popup(ui.style())
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0, palette::BORDER_STRONG()))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(10, 10));
    let popup = egui::Popup::from_toggle_button_response(&btn)
        .id(popup_id)
        .align(egui::RectAlign::BOTTOM)
        .align_alternatives(&[])
        .gap(9.0)
        .width(grid_w)
        .frame(popup_frame)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(grid_w);
            layout_section(ui, "Panels");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LAYOUT_GAP;
                layout_tile(
                    ui,
                    icons::layout_schema(),
                    "Schema",
                    "Schema panel",
                    chrome.schema,
                );
                layout_tile(
                    ui,
                    icons::layout_details(),
                    "Details",
                    "Details panel",
                    chrome.details,
                );
                layout_tile(
                    ui,
                    icons::layout_connections(),
                    "Connections",
                    "Connection tabs",
                    chrome.connections,
                );
            });
            ui.add_space(8.0);
            let y = ui.cursor().top();
            ui.painter().hline(
                ui.max_rect().x_range(),
                y,
                egui::Stroke::new(1.0, palette::BORDER()),
            );
            ui.add_space(10.0);
            layout_section(ui, "Editor");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LAYOUT_GAP;
                layout_tile(
                    ui,
                    icons::layout_query(),
                    "Query console",
                    "Query console",
                    chrome.query,
                );
                layout_tile(
                    ui,
                    icons::layout_log(),
                    "Live log",
                    "Live log panel",
                    chrome.live_log,
                );
            });
        });

    if let Some(response) = popup {
        let rect = response.response.rect;
        let anchor_x = btn
            .rect
            .center()
            .x
            .clamp(rect.left() + 10.0, rect.right() - 10.0);
        let left = egui::pos2(anchor_x - 8.0, rect.top() + 1.0);
        let right = egui::pos2(anchor_x + 8.0, rect.top() + 1.0);
        let tip = egui::pos2(anchor_x, rect.top() - 8.0);
        let painter = ui.ctx().layer_painter(response.response.layer_id);
        painter.add(egui::Shape::convex_polygon(
            vec![left, right, tip],
            palette::PANEL(),
            egui::Stroke::NONE,
        ));
        let stroke = egui::Stroke::new(1.0, palette::BORDER_STRONG());
        painter.line_segment([left, tip], stroke);
        painter.line_segment([tip, right], stroke);
    }
}

fn layout_section(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.0)
            .color(palette::TEXT_WEAK()),
    );
    ui.add_space(8.0);
}

fn layout_tile(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    tooltip: &str,
    a11y: &str,
    on: &mut bool,
) {
    let (rect, resp) = ui.allocate_exact_size(LAYOUT_TILE, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, *on, a11y));
    let resp = resp.on_hover_text(tooltip);
    if resp.clicked() {
        *on = !*on;
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    let radius = egui::CornerRadius::same(8);
    let accent = palette::ACCENT();
    let (fill, tint) = if *on {
        (
            egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 28),
            accent,
        )
    } else if resp.hovered() {
        (palette::SURFACE_HOVER(), palette::TEXT())
    } else {
        (egui::Color32::TRANSPARENT, palette::TEXT_FAINT())
    };
    ui.painter().rect_filled(rect, radius, fill);
    let glyph = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(22.0));
    egui::Image::new(icon)
        .fit_to_exact_size(glyph.size())
        .tint(tint)
        .paint_at(ui, glyph);
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
    let resp = super::soft_icon_button(ui, src, hover, true);

    ui.add_space(TOOLBAR_ICON_GAP);
    resp
}

#[derive(Default)]
pub(crate) struct RunResponse {
    pub run_current: bool,
    pub run_all: bool,
    pub save_query: bool,
}

const SHORTCUT_ICON_SIZE: f32 = 11.0;
const SHORTCUT_ICON_GAP: f32 = 2.0;

fn command_key_icon() -> egui::ImageSource<'static> {
    if cfg!(target_os = "macos") {
        icons::keyboard_command()
    } else {
        icons::keyboard_control()
    }
}

fn shortcut_icons_width(count: usize) -> f32 {
    count as f32 * SHORTCUT_ICON_SIZE + count.saturating_sub(1) as f32 * SHORTCUT_ICON_GAP
}

fn paint_shortcut_icons(
    ui: &egui::Ui,
    right: f32,
    center_y: f32,
    shortcut_icons: &[egui::ImageSource<'static>],
    color: egui::Color32,
) {
    let mut x = right - shortcut_icons_width(shortcut_icons.len());
    for icon in shortcut_icons {
        egui::Image::new(icon.clone())
            .fit_to_exact_size(egui::Vec2::splat(SHORTCUT_ICON_SIZE))
            .tint(color)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(x + SHORTCUT_ICON_SIZE * 0.5, center_y),
                    egui::Vec2::splat(SHORTCUT_ICON_SIZE),
                ),
            );
        x += SHORTCUT_ICON_SIZE + SHORTCUT_ICON_GAP;
    }
}

/// Split Run control: the main segment executes the selection/current statement, while the
/// chevron exposes both run scopes and the query-saving action in one compact menu.
pub(crate) fn run_button(ui: &mut egui::Ui, can_run: bool, can_save: bool) -> RunResponse {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_color = if can_run {
        palette::TEXT()
    } else {
        palette::TEXT_FAINT()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Run Current",
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
    let icon_size = 13.0;
    let icon_gap = 5.0;
    let shortcut_gap = 7.0;
    let current_shortcut = [command_key_icon(), icons::keyboard_return()];
    let chevron_w = 24.0;
    let main_w = icon_size
        + icon_gap
        + galley.size().x
        + shortcut_gap
        + shortcut_icons_width(current_shortcut.len())
        + pad_x * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(main_w + chevron_w, h), egui::Sense::hover());
    let main_rect = egui::Rect::from_min_size(rect.min, egui::vec2(main_w, h));
    let chevron_rect = egui::Rect::from_min_size(
        egui::pos2(main_rect.right(), rect.top()),
        egui::vec2(chevron_w, h),
    );
    let main = ui.interact(main_rect, ui.id().with("run_current"), egui::Sense::click());
    main.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, can_run, "Run Current")
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
            egui::Stroke::new(1.0, palette::BORDER()),
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
            egui::Stroke::new(1.0, palette::BORDER()),
        );
        egui::Image::new(icons::play())
            .fit_to_exact_size(egui::Vec2::splat(icon_size))
            .tint(text_color)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(
                        main_rect.left() + pad_x + icon_size * 0.5,
                        main_rect.center().y,
                    ),
                    egui::Vec2::splat(icon_size),
                ),
            );
        ui.painter().galley(
            egui::pos2(
                main_rect.left() + pad_x + icon_size + icon_gap,
                main_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            text_color,
        );
        paint_shortcut_icons(
            ui,
            main_rect.right() - pad_x,
            main_rect.center().y,
            &current_shortcut,
            palette::TEXT_FAINT(),
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
        run_current: can_run && main.clicked(),
        ..Default::default()
    };
    egui::Popup::menu(&chevron)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(164.0);
            if run_menu_item(
                ui,
                icons::play(),
                "Run All",
                &[
                    icons::keyboard_shift(),
                    command_key_icon(),
                    icons::keyboard_return(),
                ],
                can_run,
            ) {
                out.run_all = true;
                ui.close();
            }
            if run_menu_item(
                ui,
                icons::play(),
                "Run Current",
                &[command_key_icon(), icons::keyboard_return()],
                can_run,
            ) {
                out.run_current = true;
                ui.close();
            }
            ui.separator();
            if run_menu_item(ui, icons::save(), "Save query", &[], can_save) {
                out.save_query = true;
                ui.close();
            }
        });
    out
}

fn run_menu_item(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    label: &str,
    shortcut_icons: &[egui::ImageSource<'static>],
    enabled: bool,
) -> bool {
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
        egui::Image::new(icon)
            .fit_to_exact_size(egui::Vec2::splat(14.0))
            .tint(color)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 15.0, rect.center().y),
                    egui::Vec2::splat(14.0),
                ),
            );
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
        paint_shortcut_icons(
            ui,
            rect.right() - 8.0,
            rect.center().y,
            shortcut_icons,
            palette::TEXT_FAINT(),
        );
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
            egui::Stroke::new(1.0, palette::BORDER()),
        );
    }
}
