//! A small empty-state mascot drawn with `egui::Painter`.
//!
//! It intentionally stays calm: no physics, no dragging, no cursor tracking. The only motion is
//! a quick blink on its `+ +` eyes so the empty state has a little life without bouncing around.

use egui::{Color32, Pos2, Rect, Sense, Stroke};

/// One row of the 16x16 body sprite.
///
/// Characters map to palette slots in [`pixel_color`]: `.` transparent, `o` outline, `b` body,
/// `h` highlight, `s` shadow, `c` cheek. The eyes are painted on top so they can blink.
const SPRITE: [&str; 16] = [
    "................",
    ".....o....o.....",
    "....obo..obo....",
    ".....obbo.......",
    "...oobbbbbboo...",
    "..obbbbbbbbbbo..",
    ".obbhbbbbbbbbbo.",
    ".obbbbbbbbbbbbo.",
    "obbbbbbbbbbbbbbo",
    "obbbbbbbbbbbbbbo",
    "obbbbbbbbbbbbbbo",
    ".obbcbbbbbcbbbo.",
    ".obbbbbbbbbbbbo.",
    "..obbbbbbbbbbo..",
    "...obbssssbbo...",
    "....oossssoo....",
];

const GRID: f32 = 16.0;
const MASCOT_ALPHA: f32 = 0.8;
const LEFT_EYE: (f32, f32) = (5.2, 7.2);
const RIGHT_EYE: (f32, f32) = (10.8, 7.2);

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

/// Deterministic pseudo-random in `0.0..1.0` from a float seed, enough to vary blink timing.
fn rand01(seed: f32) -> f32 {
    let v = (seed * 12.9898).sin() * 43758.547;
    v - v.floor()
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn fade(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * MASCOT_ALPHA).round() as u8,
    )
}

fn pixel_color(ch: char, accent: Color32) -> Option<Color32> {
    let dark = Color32::from_rgb(40, 32, 54);
    Some(fade(match ch {
        'b' => blend(accent, Color32::WHITE, 0.72),
        'o' => blend(accent, dark, 0.56),
        'h' => blend(accent, Color32::WHITE, 0.9),
        's' => blend(accent, dark, 0.18),
        'c' => blend(accent, Color32::from_rgb(255, 142, 170), 0.62),
        _ => return None,
    }))
}

/// Draw the empty-state mascot, centred in the available area.
pub fn show(ui: &mut egui::Ui) {
    let accent = crate::style::palette::ACCENT();
    let dark = blend(accent, Color32::from_rgb(40, 32, 54), 0.62);

    ui.scope(|ui| {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }

        let px = (rect.width().min(rect.height()) * 0.22 / GRID)
            .floor()
            .clamp(3.0, 8.0);
        let half = GRID * px / 2.0;
        let pos = Pos2::new(rect.center().x, rect.center().y + px * 0.8);
        let id = ui.id().with("empty_pet");

        let t = ui.input(|i| i.time);
        let mut pet = ui.data_mut(|d| d.get_temp::<Pet>(id)).unwrap_or_default();
        if !pet.init {
            pet.last_t = t;
            pet.next_blink = t + 1.2;
            pet.init = true;
        }

        let dt = ((t - pet.last_t) as f32).clamp(0.0, 0.05);
        pet.last_t = t;

        if pet.blink > 0.0 {
            pet.blink = (pet.blink + dt * 10.0).min(2.0);
            if pet.blink >= 2.0 {
                pet.blink = 0.0;
                pet.next_blink = t + 1.8 + rand01(t as f32 + 3.0) as f64 * 3.2;
            }
        } else if t >= pet.next_blink {
            pet.blink = 0.001;
        }
        let lid = (1.0 - (pet.blink - 1.0).abs()).clamp(0.0, 1.0);

        let painter = ui.painter_at(rect);
        paint_shadow(&painter, pos, half, px, dark);
        paint_sprite(&painter, pos, accent, px);
        paint_face(&painter, pos, px, dark, lid);

        ui.data_mut(|d| d.insert_temp(id, pet));
        ui.ctx().request_repaint();
    });
}

fn paint_shadow(painter: &egui::Painter, pos: Pos2, half: f32, px: f32, dark: Color32) {
    let shadow = Rect::from_center_size(
        Pos2::new(pos.x, pos.y + half - px * 0.35),
        egui::vec2(half * 1.25, px * 1.2),
    );
    painter.rect_filled(
        shadow,
        egui::CornerRadius::same((px * 0.6) as u8),
        Color32::from_rgba_unmultiplied(
            dark.r(),
            dark.g(),
            dark.b(),
            (48.0 * MASCOT_ALPHA).round() as u8,
        ),
    );
}

fn paint_sprite(painter: &egui::Painter, pos: Pos2, accent: Color32, px: f32) {
    let left = pos.x - GRID * px / 2.0;
    let top = pos.y - GRID * px / 2.0;

    for (row, line) in SPRITE.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if let Some(color) = pixel_color(ch, accent) {
                let p = Pos2::new(left + col as f32 * px, top + row as f32 * px);
                let r = Rect::from_min_size(p, egui::vec2(px + 0.6, px + 0.6));
                painter.rect_filled(r, 0.0, color);
            }
        }
    }
}

fn paint_face(painter: &egui::Painter, pos: Pos2, px: f32, dark: Color32, lid: f32) {
    let left = pos.x - GRID * px / 2.0;
    let top = pos.y - GRID * px / 2.0;
    let eye_pos = |g: (f32, f32)| Pos2::new(left + g.0 * px, top + g.1 * px);

    for eye in [LEFT_EYE, RIGHT_EYE] {
        let c = eye_pos(eye);
        if lid > 0.55 {
            painter.line_segment(
                [
                    Pos2::new(c.x - px * 0.85, c.y),
                    Pos2::new(c.x + px * 0.85, c.y),
                ],
                Stroke::new((px * 0.42).max(1.0), fade(dark)),
            );
        } else {
            let stroke = Stroke::new((px * 0.38).max(1.0), fade(dark));
            painter.line_segment(
                [
                    Pos2::new(c.x - px * 0.72, c.y),
                    Pos2::new(c.x + px * 0.72, c.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - px * 0.72),
                    Pos2::new(c.x, c.y + px * 0.72),
                ],
                stroke,
            );
        }
    }
}
