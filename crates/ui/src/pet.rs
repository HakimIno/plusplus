//! A cute, draggable pixel-art cat that fills empty states (e.g. before a query is run).
//!
//! Everything here is drawn procedurally with [`egui::Painter`] — each "pixel" is one small
//! filled rectangle — so there is no sprite-sheet asset and no extra dependency. The cat has
//! a tiny bit of life: it idles with a breathing bob, blinks, follows the cursor with its
//! pupils, twitches its whiskers, and can be grabbed and thrown around its box (or poked with
//! a click to make it hop), bouncing off the floor and walls with a squash-and-stretch. State
//! lives in egui's per-widget temp memory so it persists across frames without touching the
//! rest of the app.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// One row of the 16×16 sprite — a cat's head with two pointy ears and a little body.
/// Characters map to palette slots in [`pixel_color`]: `.` transparent · `o` outline ·
/// `b` body · `h` highlight · `s` shadow · `w` eye-white · `c` cheek. The nose, mouth,
/// whiskers and pupils are painted on top so they can animate.
const SPRITE: [&str; 16] = [
    "...o........o...",
    "..obo......obo..",
    "..obbo....obbo..",
    ".oobbboooobbboo.",
    ".obbbbbbbbbbbbo.",
    ".obbhbbbbbbbbbo.",
    ".obwwbbbbbbwwbo.",
    ".obwwbbbbbbwwbo.",
    ".obbbbbbbbbbbbo.",
    ".obbccbbbbccbbo.",
    ".obbbbbbbbbbbbo.",
    "..obbbbbbbbbbo..",
    "..obbbbbbbbbbo..",
    ".obbbbbbbbbbbbo.",
    ".obbssssssssbbo.",
    "..osssssssssso..",
];

const GRID: f32 = 16.0;
/// Eye-white blocks in grid coords (top-left pixel of each 2×2 eye).
const LEFT_EYE: (f32, f32) = (3.0, 6.0);
const RIGHT_EYE: (f32, f32) = (11.0, 6.0);

/// Persisted, per-frame physics + animation state for the blob.
#[derive(Clone)]
struct Pet {
    /// Sprite-box centre, in screen coordinates.
    pos: Pos2,
    vel: Vec2,
    /// Vertical scale: `1.0` neutral, `<1` squished, `>1` stretched. Driven by a spring.
    squash: f32,
    squash_vel: f32,
    grabbed: bool,
    grab_offset: Vec2,
    /// Where the eyes are currently looking, smoothed toward the live target.
    look: Vec2,
    /// `0.0` eyes open … `1.0` fully shut.
    blink: f32,
    next_blink: f64,
    next_hop: f64,
    last_t: f64,
    init: bool,
}

impl Default for Pet {
    fn default() -> Self {
        Self {
            pos: Pos2::ZERO,
            vel: Vec2::ZERO,
            squash: 1.0,
            squash_vel: 0.0,
            grabbed: false,
            grab_offset: Vec2::ZERO,
            look: Vec2::ZERO,
            blink: 0.0,
            next_blink: 0.0,
            next_hop: 0.0,
            last_t: 0.0,
            init: false,
        }
    }
}

/// Deterministic pseudo-random in `0.0..1.0` from a float seed — enough jitter for blink/hop
/// timing without pulling in an rng crate.
fn rand01(seed: f32) -> f32 {
    let v = (seed * 12.9898).sin() * 43758.547;
    v - v.floor()
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Resolve a sprite character to a colour for the current accent, or `None` if transparent.
fn pixel_color(ch: char, accent: Color32) -> Option<Color32> {
    let dark = Color32::from_rgb(40, 32, 54);
    Some(match ch {
        'b' => accent,
        'o' => blend(accent, dark, 0.62),
        'h' => blend(accent, Color32::WHITE, 0.58),
        's' => blend(accent, dark, 0.28),
        'w' => Color32::from_rgb(248, 250, 255),
        'c' => blend(accent, Color32::from_rgb(255, 120, 150), 0.55),
        _ => return None,
    })
}

/// Draw the empty-state pixel pet: a draggable, animated slime centred in the available area.
pub fn show(ui: &mut egui::Ui) {
    let accent = crate::style::palette::ACCENT();
    let dark = blend(accent, Color32::from_rgb(40, 32, 54), 0.62);

    ui.scope(|ui| {
        // Claim the whole panel as the playground so there's plenty of room to fling the cat.
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }

        // Pixel size scaled to the smaller side for a comfortable medium cat — big enough to
        // read clearly, small enough to leave room to play; the floor sits above the bottom.
        let px = (rect.width().min(rect.height()) * 0.22 / GRID)
            .floor()
            .clamp(3.0, 8.0);
        let half = GRID * px / 2.0;
        let floor_y = rect.bottom() - half - px * 1.5;
        let id = ui.id().with("empty_pet");

        let t = ui.input(|i| i.time);
        let pointer = ui.input(|i| i.pointer.hover_pos());

        let mut pet = ui
            .data_mut(|d| d.get_temp::<Pet>(id))
            .unwrap_or_default();
        if !pet.init {
            pet.pos = Pos2::new(rect.center().x, floor_y);
            pet.last_t = t;
            pet.next_blink = t + 1.5;
            pet.next_hop = t + 3.0;
            pet.init = true;
        }
        let dt = ((t - pet.last_t) as f32).clamp(0.0, 0.05);
        pet.last_t = t;

        // --- input: grab / drag / throw -------------------------------------------------
        let body_box = Rect::from_center_size(pet.pos, egui::vec2(GRID * px * 0.8, GRID * px));
        let resp = ui.interact(body_box, id.with("hit"), Sense::click_and_drag());
        if resp.drag_started() {
            if let Some(p) = pointer {
                pet.grab_offset = pet.pos - p;
            }
            pet.grabbed = true;
        }
        if pet.grabbed && resp.dragged() {
            if let Some(p) = pointer {
                let target = p + pet.grab_offset;
                let clamped = Pos2::new(
                    target.x.clamp(rect.left() + half, rect.right() - half),
                    target.y.clamp(rect.top() + half, floor_y),
                );
                if dt > 0.0 {
                    pet.vel = (clamped - pet.pos) / dt;
                }
                pet.pos = clamped;
                pet.squash = (pet.squash - 0.02).max(0.85); // slight stretch while held
            }
        }
        if resp.drag_stopped() {
            pet.grabbed = false;
            // Cap the throw so a fast flick doesn't rocket it off-box.
            pet.vel = pet.vel.clamp(Vec2::splat(-900.0), Vec2::splat(900.0));
        }
        // A poke (plain click) makes the cat perk up and hop happily.
        if resp.clicked() {
            pet.vel.y = -430.0;
            pet.vel.x = (rand01(t as f32 + 2.0) - 0.5) * 220.0;
            pet.squash = 1.2;
            pet.blink = 0.001;
            pet.next_hop = t + 2.5;
        }

        // --- physics when free ----------------------------------------------------------
        if !pet.grabbed {
            pet.vel.y += 2000.0 * dt; // gravity
            pet.pos += pet.vel * dt;

            let resting = (pet.pos.y - floor_y).abs() < 0.5 && pet.vel.y.abs() < 30.0;

            // Floor: bounce with energy loss, squish on a hard landing.
            if pet.pos.y >= floor_y {
                if pet.vel.y > 60.0 {
                    pet.squash = (pet.squash - pet.vel.y * 0.0006).max(0.45);
                    pet.squash_vel = 0.0;
                }
                pet.pos.y = floor_y;
                pet.vel.y = -pet.vel.y * 0.45;
                if pet.vel.y.abs() < 40.0 {
                    pet.vel.y = 0.0;
                }
                pet.vel.x *= 0.80; // ground friction
            }
            // Walls.
            let (l, r) = (rect.left() + half, rect.right() - half);
            if pet.pos.x < l {
                pet.pos.x = l;
                pet.vel.x = pet.vel.x.abs() * 0.5;
            } else if pet.pos.x > r {
                pet.pos.x = r;
                pet.vel.x = -pet.vel.x.abs() * 0.5;
            }

            // Idle life: an occasional little hop and a gentle breathing bob.
            if resting {
                pet.vel.x *= 0.85;
                if t >= pet.next_hop {
                    pet.vel.y = -380.0 - rand01(t as f32) * 220.0;
                    pet.vel.x = (rand01(t as f32 + 7.0) - 0.5) * 260.0;
                    pet.squash = 1.18; // stretch up on take-off
                    pet.next_hop = t + 2.5 + rand01(t as f32 * 1.3) as f64 * 3.5;
                }
            }
        }

        // --- spring the squash back to neutral, plus a soft idle breathe ----------------
        let breathe = (t as f32 * 1.6).sin() * 0.03;
        let rest_target = 1.0 + breathe;
        pet.squash_vel += (-180.0 * (pet.squash - rest_target) - 16.0 * pet.squash_vel) * dt;
        pet.squash += pet.squash_vel * dt;
        pet.squash = pet.squash.clamp(0.45, 1.25);

        // --- look at the cursor, blink on a timer ---------------------------------------
        let eye_centre = pet.pos - Vec2::new(0.0, half * 0.15);
        let look_target = if pet.grabbed {
            (pet.vel * 0.04).clamp(Vec2::splat(-1.0), Vec2::splat(1.0))
        } else if let Some(p) = pointer {
            let d = (p - eye_centre) / (half * 2.0);
            d.clamp(Vec2::splat(-1.0), Vec2::splat(1.0))
        } else {
            Vec2::ZERO
        };
        pet.look += (look_target - pet.look) * (8.0 * dt).min(1.0);

        if pet.blink > 0.0 {
            // Quick close-then-open once a blink has been triggered.
            pet.blink = (pet.blink + dt * 9.0).min(2.0);
            if pet.blink >= 2.0 {
                pet.blink = 0.0;
                pet.next_blink = t + 1.5 + rand01(t as f32 + 3.0) as f64 * 4.0;
            }
        } else if t >= pet.next_blink {
            pet.blink = 0.001;
        }
        let lid = (1.0 - (pet.blink - 1.0).abs()).clamp(0.0, 1.0); // 0 open → 1 shut → 0

        // --- paint ----------------------------------------------------------------------
        let painter = ui.painter_at(rect);
        paint_shadow(&painter, pet.pos, floor_y, half, px, dark, pet.squash);
        paint_sprite(&painter, &pet, accent, half, px);
        paint_face(&painter, &pet, accent, half, px, dark, lid);

        ui.data_mut(|d| d.insert_temp(id, pet));
        ui.ctx().request_repaint();
    });
}

/// Soft contact shadow on the floor; it shrinks as the blob lifts off.
fn paint_shadow(
    painter: &egui::Painter,
    pos: Pos2,
    floor_y: f32,
    half: f32,
    px: f32,
    dark: Color32,
    squash: f32,
) {
    let lift = ((floor_y - pos.y) / (half * 3.0)).clamp(0.0, 1.0);
    let w = half * 1.5 * (1.0 - lift * 0.6) * (2.0 - squash);
    let h = px * 1.3 * (1.0 - lift * 0.4);
    let alpha = (90.0 * (1.0 - lift)) as u8;
    let shadow = Rect::from_center_size(Pos2::new(pos.x, floor_y + half - px), egui::vec2(w, h));
    painter.rect_filled(
        shadow,
        egui::CornerRadius::same((h / 2.0) as u8),
        Color32::from_rgba_unmultiplied(dark.r(), dark.g(), dark.b(), alpha),
    );
}

/// Paint the body grid with squash-and-stretch anchored at the blob's feet.
fn paint_sprite(painter: &egui::Painter, pet: &Pet, accent: Color32, half: f32, px: f32) {
    let sy = pet.squash;
    let sx = 1.0 + (1.0 - sy) * 0.6; // conserve a bit of volume
    let pw = px * sx;
    let ph = px * sy;
    let foot_y = pet.pos.y + half; // feet stay put while it squishes
    let left = pet.pos.x - GRID * pw / 2.0;
    let top = foot_y - GRID * ph;

    for (row, line) in SPRITE.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if let Some(color) = pixel_color(ch, accent) {
                let p = Pos2::new(left + col as f32 * pw, top + row as f32 * ph);
                // +0.6 closes hairline seams between adjacent pixels.
                let r = Rect::from_min_size(p, egui::vec2(pw + 0.6, ph + 0.6));
                painter.rect_filled(r, 0.0, color);
            }
        }
    }
}

/// Pupils (tracking the cursor), eye-shine, blink lids, and the cat muzzle — pink nose, an
/// "ω" mouth and whiskers — drawn over the body.
fn paint_face(
    painter: &egui::Painter,
    pet: &Pet,
    accent: Color32,
    half: f32,
    px: f32,
    dark: Color32,
    lid: f32,
) {
    let sy = pet.squash;
    let sx = 1.0 + (1.0 - sy) * 0.6;
    let pw = px * sx;
    let ph = px * sy;
    let foot_y = pet.pos.y + half;
    let left = pet.pos.x - GRID * pw / 2.0;
    let top = foot_y - GRID * ph;

    // Centre of a 2×2 eye block given its top-left grid cell.
    let eye_pos = |g: (f32, f32)| Pos2::new(left + (g.0 + 1.0) * pw, top + (g.1 + 1.0) * ph);
    let max_off = Vec2::new(pw * 0.45, ph * 0.45);

    for eye in [LEFT_EYE, RIGHT_EYE] {
        let c = eye_pos(eye);
        if lid > 0.55 {
            // Closed: a happy dark arc across the eye.
            let w = pw * 1.6;
            painter.line_segment(
                [Pos2::new(c.x - w / 2.0, c.y), Pos2::new(c.x + w / 2.0, c.y)],
                Stroke::new(ph * 0.7, dark),
            );
        } else {
            let off = Vec2::new(pet.look.x * max_off.x, pet.look.y * max_off.y);
            let pr = Rect::from_center_size(c + off, egui::vec2(pw * 1.1, ph * 1.1));
            painter.rect_filled(pr, 0.0, dark);
            // Tiny catch-light.
            painter.rect_filled(
                Rect::from_center_size(c + off - Vec2::new(pw * 0.3, ph * 0.3), egui::vec2(pw * 0.4, ph * 0.4)),
                0.0,
                Color32::from_rgb(248, 250, 255),
            );
        }
    }

    // Cat muzzle: a pink nose, a soft "ω" mouth, and whiskers — hidden while blinking so the
    // closed-eye arcs read as a happy squint.
    if lid <= 0.55 {
        let nose_c = Pos2::new(pet.pos.x, top + 9.0 * ph);
        painter.rect_filled(
            Rect::from_center_size(nose_c, egui::vec2(pw * 1.4, ph * 1.0)),
            egui::CornerRadius::same((ph * 0.5) as u8),
            Color32::from_rgb(255, 150, 170),
        );

        // Stem from the nose, then the two humps of the "ω".
        let mouth = Stroke::new((ph * 0.42).max(1.0), dark);
        let my = nose_c.y + ph * 1.1;
        painter.line_segment(
            [Pos2::new(nose_c.x, nose_c.y + ph * 0.4), Pos2::new(nose_c.x, my)],
            mouth,
        );
        painter.line_segment(
            [Pos2::new(nose_c.x, my), Pos2::new(nose_c.x - pw * 1.0, my - ph * 0.4)],
            mouth,
        );
        painter.line_segment(
            [Pos2::new(nose_c.x, my), Pos2::new(nose_c.x + pw * 1.0, my - ph * 0.4)],
            mouth,
        );

        // Whiskers: two thin strokes fanning out from each cheek, with a tiny cursor-driven
        // twitch so they feel alive.
        let wh = Stroke::new((ph * 0.28).max(1.0), blend(dark, accent, 0.35));
        let twitch = pet.look.y * ph * 0.3;
        for side in [-1.0_f32, 1.0] {
            let bx = pet.pos.x + side * pw * 2.2;
            painter.line_segment(
                [
                    Pos2::new(bx, nose_c.y - ph * 0.2),
                    Pos2::new(bx + side * pw * 2.6, nose_c.y - ph * 0.9 + twitch),
                ],
                wh,
            );
            painter.line_segment(
                [
                    Pos2::new(bx, nose_c.y + ph * 0.5),
                    Pos2::new(bx + side * pw * 2.6, nose_c.y + ph * 0.7 + twitch),
                ],
                wh,
            );
        }
    }
}
