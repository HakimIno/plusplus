//! Welcome rendering and interaction.

use crate::app::{Action, DbGuiApp};
use crate::icons;
use crate::style;
use crate::style::palette;
use crate::title_bar;

impl DbGuiApp {
    /// Full-page first-run welcome screen. Replaces the entire window; no title bar.
    /// Called from `draw()` with an early return so no other panels render simultaneously.
    ///
    /// One full-bleed scene: an accent-tinted wash over the theme base, a layered landscape
    /// anchored to the bottom edge, and a centred speech-bubble card stack. Everything is
    /// derived from theme tokens — the illustration is a white SVG tinted to the accent, so
    /// depth comes from opacity tiers, never a second hue.
    pub(in crate::app) fn draw_welcome_page(
        &mut self,
        root: &mut egui::Ui,
        actions: &mut Vec<Action>,
    ) {
        let ctx = root.ctx().clone();

        // Enter is the keyboard path to the single CTA.
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::DismissWelcome);
        }

        // Snapshot the picker data up front: rendering swatches must not hold a borrow on
        // `self.themes` while a click calls `set_theme(&mut self, …)`.
        let theme_options: Vec<(String, String, egui::Color32, egui::Color32)> = self
            .themes
            .entries()
            .iter()
            .map(|e| (e.key.clone(), e.name.clone(), e.theme.base, e.theme.accent))
            .collect();

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(style::mix(palette::BASE(), palette::ACCENT(), 0.06)))
            .show_inside(root, |ui| {
                let full = ui.max_rect();

                // --- Landscape backdrop, anchored to the bottom edge ---
                // Scale by width; on short-and-wide windows the band is capped to ~42% of the
                // height by widening past the window instead (sides crop, the bottom always
                // reads as ground).
                let aspect = 1440.0 / 360.0;
                let img_w = full.width().max(full.height() * 0.42 * aspect);
                let img_h = img_w / aspect;
                let img_rect = egui::Rect::from_min_max(
                    egui::pos2(full.center().x - img_w / 2.0, full.bottom() - img_h),
                    egui::pos2(full.center().x + img_w / 2.0, full.bottom()),
                );
                // Rasterize at a fixed size and stretch to the rect. Letting the texture
                // follow the painted size (Image::paint_at) requests rect × pixels_per_point
                // texels, which blows past GPU limits on unbounded headless max_rects and
                // very large displays. 2048 stays under every backend's minimum max side;
                // LINEAR filtering hides the upscale on the soft shapes.
                let hills = egui::include_image!("../../../assets/illus/welcome-hills.svg").load(
                    &ctx,
                    egui::TextureOptions::LINEAR,
                    egui::SizeHint::Size {
                        width: 2048,
                        height: 512,
                        maintain_aspect_ratio: false,
                    },
                );
                if let Ok(egui::load::TexturePoll::Ready { texture }) = hills {
                    ui.painter().image(
                        texture.id,
                        img_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        palette::ACCENT().linear_multiply(0.5),
                    );
                }

                // The welcome page suppresses the title-bar chrome, so give the window a drag
                // handle: the strip where the titlebar would be starts a native window move.
                let strip = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 44.0));
                let drag = ui.interact(
                    strip,
                    ui.id().with("welcome_drag"),
                    egui::Sense::click_and_drag(),
                );
                title_bar::handle_chrome_response(ui, &drag);

                // Undecorated Linux/Windows windows have no native buttons, so the self-drawn
                // close/maximize/minimize cluster must survive onto the welcome page too.
                // Drawn after the drag strip so the buttons win pointer priority over it.
                #[cfg(not(target_os = "macos"))]
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(strip.shrink2(egui::vec2(10.0, 0.0)))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        title_bar::window_controls(ui);
                    },
                );

                // --- Centred card stack ---
                let card_w = 400.0_f32;
                let stack_h = 420.0_f32;
                let top = ((full.height() - stack_h) * 0.40).max(48.0) + full.top();
                let content = egui::Rect::from_min_size(
                    egui::pos2(full.center().x - card_w / 2.0, top),
                    egui::vec2(card_w, stack_h),
                );

                let mut card_rect = content; // updated below; used to seat the ++ mark
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    // Speech-bubble header card.
                    let bubble = egui::Frame::new()
                        .fill(palette::SURFACE())
                        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(24, 20))
                        .show(ui, |ui| {
                            ui.set_width(card_w - 48.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(
                                    egui::RichText::new("Welcome to ")
                                        .size(22.0)
                                        .strong()
                                        .color(palette::TEXT()),
                                );
                                ui.label(
                                    egui::RichText::new("plusplus")
                                        .size(22.0)
                                        .strong()
                                        .color(palette::ACCENT()),
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "A fast, native database client. Everything stays on this machine.",
                                )
                                .size(12.5)
                                .color(palette::TEXT_WEAK()),
                            );
                        });

                    // Bubble tail, pointing down toward the content card.
                    let br = bubble.response.rect;
                    let tail = vec![
                        egui::pos2(br.right() - 58.0, br.bottom() - 1.0),
                        egui::pos2(br.right() - 34.0, br.bottom() - 1.0),
                        egui::pos2(br.right() - 42.0, br.bottom() + 12.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(
                        tail.clone(),
                        palette::SURFACE(),
                        egui::Stroke::NONE,
                    ));
                    let tail_stroke = egui::Stroke::new(1.0_f32, palette::BORDER());
                    ui.painter().line_segment([tail[0], tail[2]], tail_stroke);
                    ui.painter().line_segment([tail[1], tail[2]], tail_stroke);

                    ui.add_space(18.0);

                    // Content card: feature rows, theme swatches, CTA.
                    let card = egui::Frame::new()
                        .fill(palette::SURFACE())
                        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(24, 20))
                        .show(ui, |ui| {
                            ui.set_width(card_w - 48.0);

                            for (icon, txt) in [
                                (icons::database(), "Connect to Postgres, MySQL, MSSQL & SQLite"),
                                (icons::table(), "Browse schemas, edit cells in safe transactions"),
                                (icons::code(), "SQL editor with completion and highlighting"),
                                (icons::diagram(), "ER diagrams of your schema"),
                            ] {
                                ui.horizontal(|ui| {
                                    let (chip, _) = ui.allocate_exact_size(
                                        egui::Vec2::splat(28.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(
                                        chip,
                                        egui::CornerRadius::same(8),
                                        palette::ACCENT().linear_multiply(0.14),
                                    );
                                    egui::Image::new(icon)
                                        .tint(palette::ACCENT())
                                        .paint_at(
                                            ui,
                                            egui::Rect::from_center_size(
                                                chip.center(),
                                                egui::Vec2::splat(15.0),
                                            ),
                                        );
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(txt)
                                            .size(12.5)
                                            .color(palette::TEXT()),
                                    );
                                });
                                ui.add_space(8.0);
                            }

                            ui.add_space(4.0);
                            let sep_y = ui.cursor().top();
                            ui.painter().hline(
                                ui.max_rect().x_range(),
                                sep_y,
                                egui::Stroke::new(1.0_f32, palette::BORDER()),
                            );
                            ui.add_space(12.0);

                            // Theme picker: one swatch per theme (base disc, accent dot),
                            // selected = accent ring. Hover shows the theme's name.
                            ui.label(
                                egui::RichText::new("Theme")
                                    .size(style::font::CAPTION)
                                    .color(palette::TEXT_FAINT()),
                            );
                            ui.add_space(6.0);
                            let mut chosen: Option<String> = None;
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                                for (key, name, base, accent) in &theme_options {
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::Vec2::splat(24.0),
                                        egui::Sense::click(),
                                    );
                                    let c = rect.center();
                                    let p = ui.painter();
                                    p.circle_filled(c, 10.0, *base);
                                    p.circle_filled(c, 3.5, *accent);
                                    if *key == self.theme {
                                        p.circle_stroke(
                                            c,
                                            11.5,
                                            egui::Stroke::new(1.5_f32, palette::ACCENT()),
                                        );
                                    } else {
                                        p.circle_stroke(
                                            c,
                                            10.0,
                                            egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()),
                                        );
                                    }
                                    let resp = resp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(name);
                                    if resp.clicked() {
                                        chosen = Some(key.clone());
                                    }
                                }
                            });
                            if let Some(key) = chosen {
                                if key != self.theme {
                                    self.set_theme(&ctx, key);
                                }
                            }

                            ui.add_space(16.0);

                            // CTA: full-width accent pill.
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 38.0),
                                egui::Sense::click(),
                            );
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    "Get Started",
                                )
                            });
                            let fill = if resp.hovered() || resp.is_pointer_button_down_on() {
                                palette::ACCENT_HOVER()
                            } else {
                                palette::ACCENT()
                            };
                            ui.painter().rect_filled(rect, egui::CornerRadius::same(10), fill);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Get Started",
                                egui::FontId::proportional(13.5),
                                palette::ON_ACCENT(),
                            );
                            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                actions.push(Action::DismissWelcome);
                            }

                            ui.add_space(2.0);
                        });
                    card_rect = card.response.rect;
                });

                // Landscape illustration beside the card stack (skipped when the
                // window is too narrow for it to sit clear of the cards).
                let mark = egui::Rect::from_min_size(
                    egui::pos2(card_rect.right() + 24.0, full.bottom() - 150.0),
                    egui::vec2(250.0, 150.0),
                );
                if mark.right() < full.right() - 8.0 {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(mark), |ui| {
                        crate::pet::show(ui);
                    });
                }
            });
    }
}
