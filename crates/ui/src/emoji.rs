//! Colour-emoji rendering for grid cells. egui's text engine only rasterizes monochrome
//! outlines, so emoji come out black-and-white (or as tofu). Here we rasterize the OS colour
//! emoji font (Apple Color Emoji on macOS) to RGBA with `swash` and blit the glyphs inline as
//! images, the way TablePlus shows them. On platforms without a known colour-emoji font, the
//! atlas stays disabled and callers fall back to the monochrome text path.

use std::cell::RefCell;
use std::collections::HashMap;

use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::shape::ShapeContext;
use swash::zeno::Format;
use swash::FontRef;
use unicode_segmentation::UnicodeSegmentation;

/// Pixel size the emoji bitmap is rasterized at. Apple's font ships strikes at 20/32/40/64/96/
/// 160 px; 64 is a crisp source for the ~16 px we display, even at 2× (retina), while keeping
/// each cached texture small (64·64·4 ≈ 16 KB).
const RASTER_PX: f32 = 64.0;

/// Where the OS colour-emoji font lives. macOS only for now; other platforms leave the atlas
/// disabled (emoji render monochrome through egui's bundled Noto Emoji fallback).
#[cfg(target_os = "macos")]
const EMOJI_FONT_PATH: &str = "/System/Library/Fonts/Apple Color Emoji.ttc";

/// A cell-text segment: a run of plain text, or one emoji grapheme cluster.
pub enum Run<'a> {
    Text(&'a str),
    Emoji(&'a str),
}

/// Is this scalar part of an emoji? A deliberately loose test over the emoji-bearing Unicode
/// blocks: a false positive only costs one shaping probe that then falls back to text, while
/// plain ASCII/Latin text never matches, so emoji-free cells skip the atlas entirely.
fn is_emoji_scalar(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF   // emoji, supplemental symbols & pictographs, flags (1F1E6–1F1FF)
        | 0x2600..=0x27BF   // misc symbols + dingbats (✨ ☀ ❤ ✂ ✅ ❌ …)
        | 0x2300..=0x23FF   // technical (⌚ ⏰ ⏳ …)
        | 0x2B00..=0x2BFF   // stars & arrows (⭐ ⬆ ⬇)
        | 0x2122 | 0x00A9 | 0x00AE | 0x203C | 0x2049 // ™ © ® ‼ ⁉
        | 0xFE0F            // emoji variation selector (turns a text symbol into emoji)
        | 0x20E3            // keycap combiner (1️⃣)
    )
}

/// Does `s` contain anything worth rendering as a colour emoji? Cheap scan used to keep the
/// common (emoji-free) cell on the plain-label fast path.
pub fn contains_emoji(s: &str) -> bool {
    s.chars().any(is_emoji_scalar)
}

/// Split `s` into runs of plain text and individual emoji grapheme clusters (so ZWJ sequences,
/// skin-tone modifiers, and flags stay whole). Consecutive non-emoji graphemes coalesce into
/// one text run.
pub fn segment(s: &str) -> Vec<Run<'_>> {
    let mut runs = Vec::new();
    let mut text_start: Option<usize> = None;
    let mut idx = 0;
    for g in s.graphemes(true) {
        let is_emoji = g.chars().any(is_emoji_scalar);
        if is_emoji {
            if let Some(start) = text_start.take() {
                runs.push(Run::Text(&s[start..idx]));
            }
            runs.push(Run::Emoji(g));
        } else if text_start.is_none() {
            text_start = Some(idx);
        }
        idx += g.len();
    }
    if let Some(start) = text_start {
        runs.push(Run::Text(&s[start..]));
    }
    runs
}

/// Lazily-loaded colour-emoji rasterizer with a per-grapheme texture cache. Lives on the app
/// (UI thread only), so the interior mutability is plain `RefCell` — no locking.
#[derive(Default)]
pub struct EmojiAtlas {
    /// The mmap'd font file. `None` until the first emoji is seen; `Some(None)` once we've
    /// tried and failed (no font on this platform) so we don't retry every frame.
    font: RefCell<Option<Option<memmap2::Mmap>>>,
    scale: RefCell<ScaleContext>,
    shape: RefCell<ShapeContext>,
    /// grapheme → cached texture (`None` = font has no colour glyph for it; fall back to text).
    cache: RefCell<HashMap<String, Option<egui::TextureHandle>>>,
}

impl EmojiAtlas {
    /// The texture id for `grapheme`'s colour emoji, rasterizing and caching on first use.
    /// `None` when the OS font is unavailable or has no colour glyph for it — render text then.
    pub fn texture(&self, ctx: &egui::Context, grapheme: &str) -> Option<egui::TextureId> {
        if let Some(cached) = self.cache.borrow().get(grapheme) {
            return cached.as_ref().map(|t| t.id());
        }
        let handle = self.rasterize(ctx, grapheme);
        let id = handle.as_ref().map(|t| t.id());
        self.cache.borrow_mut().insert(grapheme.to_owned(), handle);
        id
    }

    /// Rasterize one emoji grapheme to an egui texture, or `None` if it can't be drawn in colour.
    fn rasterize(&self, ctx: &egui::Context, grapheme: &str) -> Option<egui::TextureHandle> {
        let font_guard = self.font_bytes();
        let bytes = font_guard.as_ref()?;
        let font = FontRef::from_index(bytes, 0)?;

        // Shape the whole cluster so ZWJ sequences / skin tones / flags collapse to their single
        // ligated glyph; anything that doesn't is left to the text fallback.
        let gid = {
            let mut shape_ctx = self.shape.borrow_mut();
            let mut shaper = shape_ctx.builder(font).size(RASTER_PX).build();
            shaper.add_str(grapheme);
            let mut glyphs = Vec::new();
            shaper.shape_with(|cluster| {
                for g in cluster.glyphs {
                    glyphs.push(g.id);
                }
            });
            match glyphs.as_slice() {
                [g] if *g != 0 => *g,
                _ => return None,
            }
        };

        let image = {
            let mut scale_ctx = self.scale.borrow_mut();
            let mut scaler = scale_ctx.builder(font).size(RASTER_PX).hint(false).build();
            Render::new(&[
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::ColorOutline(0),
            ])
            .format(Format::Alpha)
            .render(&mut scaler, gid)?
        };
        if image.content != Content::Color
            || image.placement.width == 0
            || image.placement.height == 0
        {
            return None;
        }
        let size = [image.placement.width as usize, image.placement.height as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, &image.data);
        Some(ctx.load_texture(
            format!("emoji-{grapheme}"),
            color,
            egui::TextureOptions::LINEAR,
        ))
    }

    /// Borrow the mmap'd font bytes, loading the OS font on first call. Returns a guard whose
    /// inner `Option<Mmap>` is `None` when no colour-emoji font is available here.
    fn font_bytes(&self) -> std::cell::Ref<'_, Option<memmap2::Mmap>> {
        if self.font.borrow().is_none() {
            *self.font.borrow_mut() = Some(load_emoji_font());
        }
        std::cell::Ref::map(self.font.borrow(), |o| o.as_ref().unwrap())
    }
}

#[cfg(target_os = "macos")]
fn load_emoji_font() -> Option<memmap2::Mmap> {
    let file = std::fs::File::open(EMOJI_FONT_PATH).ok()?;
    // Safety: the system font file is read-only and outlives the process; we only read it.
    unsafe { memmap2::Mmap::map(&file).ok() }
}

#[cfg(not(target_os = "macos"))]
fn load_emoji_font() -> Option<memmap2::Mmap> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_text_and_emoji_runs() {
        let runs = segment("Love 💜 it 😍");
        let shape: Vec<&str> = runs
            .iter()
            .map(|r| match r {
                Run::Text(t) => *t,
                Run::Emoji(e) => *e,
            })
            .collect();
        assert_eq!(shape, ["Love ", "💜", " it ", "😍"]);
        // The text/emoji classification, in order.
        assert!(matches!(runs[0], Run::Text(_)));
        assert!(matches!(runs[1], Run::Emoji(_)));
        assert!(matches!(runs[3], Run::Emoji(_)));
    }

    #[test]
    fn plain_text_has_no_emoji() {
        assert!(!contains_emoji("just plain ascii, 123"));
        assert!(!contains_emoji("ภาษาไทยไม่ใช่ emoji"));
        assert!(contains_emoji("hi 👍"));
    }

    /// Probe (ignored, macOS-only): swash must rasterize Apple Color Emoji (sbix) to coloured
    /// RGBA — the make-or-break assumption behind this whole module.
    #[test]
    #[ignore]
    fn rasterizes_apple_color_emoji() {
        let atlas = EmojiAtlas::default();
        let bytes = atlas.font_bytes();
        let Some(bytes) = bytes.as_ref() else {
            eprintln!("no emoji font on this platform — skipping");
            return;
        };
        let font = FontRef::from_index(bytes, 0).expect("parse font");
        let gid = font.charmap().map('💜');
        assert_ne!(gid, 0);
    }
}
