//! Empty-state illustration.
//!
//! Currently the GitHub "first pull request" tugboat SVG. The blinking chameleon
//! is kept in [`chameleon`] so it can come back later.

use egui::{Color32, Pos2, Sense, Vec2};

/// Native size of `profile-first-pr-dark.svg`.
const SVG_SIZE: Vec2 = Vec2::new(500.0, 300.0);

/// Draw the empty-state illustration, centred in the available area.
pub fn show(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover()).widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Image, true, "Empty state mark")
        });
        if !ui.is_rect_visible(rect) {
            return;
        }

        let aspect = SVG_SIZE.x / SVG_SIZE.y;
        let max_w = (rect.width().min(rect.height() * aspect) * 0.92).clamp(140.0, 420.0);
        let size = Vec2::new(max_w, max_w / aspect);
        let img_rect = egui::Rect::from_center_size(rect.center(), size);

        let tex = egui::include_image!("../assets/illus/profile-first-pr-dark.svg").load(
            ui.ctx(),
            egui::TextureOptions::LINEAR,
            egui::SizeHint::Size {
                width: 1000,
                height: 600,
                maintain_aspect_ratio: true,
            },
        );

        let painter = ui.painter_at(rect);
        match tex {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                painter.image(
                    texture.id,
                    img_rect,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                ui.ctx().request_repaint();
            }
            Err(_) => {}
        }
    });
}

/// Blinking chameleon-on-pencil. Unused while the tugboat is on screen.
#[allow(dead_code)]
mod chameleon {
    use egui::{Color32, Pos2, Sense, Stroke, Vec2};
    use std::time::Duration;

    /// Eye centres as fractions of [`empty-chameleon.png`].
    const EYES: [(f32, f32); 2] = [(0.5878, 0.3190), (0.8443, 0.3003)];

    #[derive(Clone)]
    struct Pet {
        /// `0.0` eyes open; `1.0` fully shut.
        blink: f32,
        next_blink: f64,
        last_t: f64,
        init: bool,
    }

    impl Default for Pet {
        fn default() -> Self {
            Self {
                blink: 0.0,
                next_blink: 0.0,
                last_t: 0.0,
                init: false,
            }
        }
    }

    fn rand01(seed: f32) -> f32 {
        let v = (seed * 12.9898).sin() * 43758.547;
        v - v.floor()
    }

    fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
        Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
    }

    fn with_alpha(color: Color32, alpha: f32) -> Color32 {
        Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }

    /// Sleep until the next visible change. During a blink we target 60 fps; between blinks the
    /// mascot costs no frames and wakes exactly when the next blink starts.
    fn repaint_delay(t: f64, blink: f32, next_blink: f64) -> Duration {
        if blink > 0.0 || t >= next_blink {
            Duration::from_millis(16)
        } else {
            Duration::from_secs_f64((next_blink - t).max(0.016))
        }
    }

    /// Draw the chameleon mascot, centred in the available area.
    pub fn show(ui: &mut egui::Ui) {
        let accent = crate::style::palette::ACCENT();
        let faint = crate::style::palette::TEXT_FAINT();

        ui.scope(|ui| {
            let rect = ui.available_rect_before_wrap();
            ui.allocate_rect(rect, Sense::hover()).widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Image, true, "Empty state mark")
            });
            if !ui.is_rect_visible(rect) {
                return;
            }

            let side = (rect.width().min(rect.height()) * 0.72).clamp(110.0, 220.0);
            let img_rect = egui::Rect::from_center_size(
                rect.center() + Vec2::new(0.0, side * 0.02),
                Vec2::splat(side),
            );

            let t = ui.input(|i| i.time);
            let id = ui.id().with("empty_pet");
            let mut pet = ui.data_mut(|d| d.get_temp::<Pet>(id)).unwrap_or_default();
            if !pet.init {
                pet.last_t = t;
                pet.next_blink = t + 1.4;
                pet.init = true;
            }

            let dt = ((t - pet.last_t) as f32).clamp(0.0, 0.05);
            pet.last_t = t;

            if pet.blink > 0.0 {
                pet.blink = (pet.blink + dt * 9.0).min(2.0);
                if pet.blink >= 2.0 {
                    pet.blink = 0.0;
                    pet.next_blink = t + 2.0 + rand01(t as f32 + 3.0) as f64 * 3.5;
                }
            } else if t >= pet.next_blink {
                pet.blink = 0.001;
            }
            let lid = (1.0 - (pet.blink - 1.0).abs()).clamp(0.0, 1.0);

            let tint = with_alpha(blend(faint, accent, 0.22), 0.58);

            let tex = egui::include_image!("../assets/illus/empty-chameleon.png").load(
                ui.ctx(),
                egui::TextureOptions::LINEAR,
                egui::SizeHint::Size {
                    width: 512,
                    height: 512,
                    maintain_aspect_ratio: true,
                },
            );

            let painter = ui.painter_at(rect);
            match tex {
                Ok(egui::load::TexturePoll::Ready { texture }) => {
                    painter.image(
                        texture.id,
                        img_rect,
                        egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                }
                Ok(egui::load::TexturePoll::Pending { .. }) => {
                    ui.ctx().request_repaint();
                }
                Err(_) => {}
            }

            paint_pupils(&painter, img_rect, tint, lid);

            let repaint_after = repaint_delay(t, pet.blink, pet.next_blink);
            ui.data_mut(|d| d.insert_temp(id, pet));
            ui.ctx().request_repaint_after(repaint_after);
        });
    }

    fn paint_pupils(painter: &egui::Painter, img: egui::Rect, color: Color32, lid: f32) {
        let stroke = Stroke::new((img.width() * 0.018).clamp(1.6, 3.2), color);
        let arm = img.width() * 0.028;

        for &(fx, fy) in &EYES {
            let eye = Pos2::new(img.left() + fx * img.width(), img.top() + fy * img.height());
            painter.line_segment(
                [Pos2::new(eye.x - arm, eye.y), Pos2::new(eye.x + arm, eye.y)],
                stroke,
            );
            if lid <= 0.55 {
                painter.line_segment(
                    [Pos2::new(eye.x, eye.y - arm), Pos2::new(eye.x, eye.y + arm)],
                    stroke,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn idle_pet_sleeps_until_the_next_blink() {
            assert_eq!(repaint_delay(1.0, 0.0, 4.5), Duration::from_secs_f64(3.5));
        }

        #[test]
        fn blinking_pet_targets_sixty_fps() {
            assert_eq!(repaint_delay(4.5, 0.25, 4.5), Duration::from_millis(16));
            assert_eq!(repaint_delay(4.5, 0.0, 4.5), Duration::from_millis(16));
        }
    }
}
