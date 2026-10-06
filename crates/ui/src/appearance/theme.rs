//! Theme system: named colour palettes the user can switch between at runtime.
//!
//! A [`Theme`] is a flat set of colour tokens (the same tokens [`crate::style::palette`]
//! exposes). The app keeps one "current" theme in a thread-local cell — egui is
//! single-threaded on the UI side, so a cheap `Cell` is all we need. UI code reads
//! colours through `palette::*` accessors, which forward to [`current`]; switching themes
//! is just [`set_current`] followed by re-applying the style.
//!
//! Themes come from two places, both surfaced through a [`ThemeRegistry`]:
//!   * the built-in palettes returned by [`builtins`], compiled in; and
//!   * user-installed `*.json` files dropped into [`dbcore::config::themes_dir`] — these
//!     deserialize into a [`ThemeFile`] (hex colours) and let anyone ship a theme without
//!     touching the binary. This is the first plugin "contribution point".

use std::cell::Cell;
use std::path::Path;

use egui::Color32;
use serde::{Deserialize, Serialize};

/// A complete set of colour tokens. `Copy` so it can live in a `Cell` and be read freely.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Whether this is a dark theme — selects egui's `Visuals::dark()`/`light()` base.
    pub is_dark: bool,

    // --- surfaces (darkest → lightest, for a dark theme) ---
    pub base: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub code_bg: Color32,
    pub stripe: Color32,
    pub selection: Color32,

    // --- borders ---
    pub border: Color32,
    pub border_strong: Color32,

    // --- text ---
    pub text: Color32,
    pub text_weak: Color32,
    pub text_faint: Color32,

    // --- accent ---
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,

    // --- semantic ---
    pub success: Color32,
    pub danger: Color32,
    pub warning: Color32,

    /// Optional editor colours; absent palettes derive SQL colours from the UI tokens.
    pub syntax: Option<SyntaxTheme>,
}

/// SQL editor colours, independent of the workspace's action and status colours.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct SyntaxTheme {
    #[serde(with = "hex")]
    pub keyword: Color32,
    #[serde(with = "hex")]
    pub string: Color32,
    #[serde(with = "hex")]
    pub number: Color32,
    #[serde(with = "hex")]
    pub comment: Color32,
    #[serde(with = "hex")]
    pub punctuation: Color32,
    #[serde(with = "hex")]
    pub identifier: Color32,
    /// Names following a dot, such as `title` in `f.title`.
    #[serde(with = "hex")]
    pub qualified_identifier: Color32,
}

/// The on-disk form of a [`Theme`]: a human-authored JSON file with `#rrggbb` colours, a
/// display `name`, and an optional `author`. Mirrors [`Theme`] field-for-field so a custom
/// theme is just data — no code, no recompile. Drop one in [`dbcore::config::themes_dir`].
#[derive(Clone, Serialize, Deserialize)]
pub struct ThemeFile {
    /// Display name shown in the picker, e.g. `"Dracula"`.
    pub name: String,
    /// Optional attribution, shown as a hint in the picker.
    #[serde(default)]
    pub author: Option<String>,
    /// Dark vs light base — picks egui's `Visuals::dark()`/`light()`.
    pub is_dark: bool,

    #[serde(with = "hex")]
    pub base: Color32,
    #[serde(with = "hex")]
    pub panel: Color32,
    #[serde(with = "hex")]
    pub surface: Color32,
    #[serde(with = "hex")]
    pub surface_hover: Color32,
    #[serde(with = "hex")]
    pub code_bg: Color32,
    #[serde(with = "hex")]
    pub stripe: Color32,
    #[serde(with = "hex")]
    pub selection: Color32,
    #[serde(with = "hex")]
    pub border: Color32,
    #[serde(with = "hex")]
    pub border_strong: Color32,
    #[serde(with = "hex")]
    pub text: Color32,
    #[serde(with = "hex")]
    pub text_weak: Color32,
    #[serde(with = "hex")]
    pub text_faint: Color32,
    #[serde(with = "hex")]
    pub accent: Color32,
    #[serde(with = "hex")]
    pub accent_hover: Color32,
    #[serde(with = "hex")]
    pub on_accent: Color32,
    #[serde(with = "hex")]
    pub success: Color32,
    #[serde(with = "hex")]
    pub danger: Color32,
    #[serde(with = "hex")]
    pub warning: Color32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<SyntaxTheme>,
}

impl ThemeFile {
    /// Flatten into the runtime [`Theme`] the rest of the app reads.
    pub(crate) fn to_theme(&self) -> Theme {
        Theme {
            is_dark: self.is_dark,
            base: self.base,
            panel: self.panel,
            surface: self.surface,
            surface_hover: self.surface_hover,
            code_bg: self.code_bg,
            stripe: self.stripe,
            selection: self.selection,
            border: self.border,
            border_strong: self.border_strong,
            text: self.text,
            text_weak: self.text_weak,
            text_faint: self.text_faint,
            accent: self.accent,
            accent_hover: self.accent_hover,
            on_accent: self.on_accent,
            success: self.success,
            danger: self.danger,
            warning: self.warning,
            syntax: self.syntax,
        }
    }
}

/// serde adapter: (de)serialize a [`Color32`]'s RGB as a `#rrggbb` hex string. Alpha is not
/// represented — themes are opaque palettes — so colours round-trip through their RGB only.
mod hex {
    use egui::Color32;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).ok_or_else(|| D::Error::custom(format!("invalid hex colour: {s:?}")))
    }

    /// Parse `#rrggbb` / `rrggbb` / `#rgb` / `rgb` into an opaque [`Color32`].
    fn parse(s: &str) -> Option<Color32> {
        let s = s.strip_prefix('#').unwrap_or(s);
        let (r, g, b) = match s.len() {
            6 => (
                u8::from_str_radix(&s[0..2], 16).ok()?,
                u8::from_str_radix(&s[2..4], 16).ok()?,
                u8::from_str_radix(&s[4..6], 16).ok()?,
            ),
            // Shorthand #rgb expands each nibble (e.g. "f80" -> ff8800).
            3 => {
                let n = |i: usize| u8::from_str_radix(&s[i..=i], 16).ok().map(|v| v * 17);
                (n(0)?, n(1)?, n(2)?)
            }
            _ => return None,
        };
        Some(Color32::from_rgb(r, g, b))
    }
}

/// Stable key of the built-in default theme.
pub const DEFAULT_KEY: &str = "carbon";

/// One selectable theme — a resolved [`Theme`] plus the metadata the picker needs.
#[derive(Clone)]
pub struct ThemeEntry {
    /// Stable identifier persisted to settings.json. Built-ins use fixed keys
    /// (`"carbon"`, …); custom themes use their file stem (e.g. `dracula.json` → `"dracula"`).
    pub key: String,
    /// Human-readable name for the picker.
    pub name: String,
    /// Optional attribution for custom themes (built-ins are `None`).
    pub author: Option<String>,
    /// `true` for compiled-in themes, `false` for ones loaded from disk.
    pub builtin: bool,
    /// The concrete colour set.
    pub theme: Theme,
}

/// All themes available to the picker: the built-ins, followed by any user-installed ones.
///
/// Loaded once at startup (and on demand via [`reload`](Self::reload)). Built-ins always come
/// first and can't be shadowed — a custom file whose stem collides with a built-in key is
/// skipped, so the defaults are always present and stable.
pub struct ThemeRegistry {
    entries: Vec<ThemeEntry>,
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::load()
    }
}

impl ThemeRegistry {
    /// Build the registry: built-ins plus every readable `*.json` in the themes directory.
    pub fn load() -> Self {
        let mut entries = builtins();

        if let Ok(dir) = dbcore::config::themes_dir() {
            let mut customs = load_custom_themes(&dir);
            // Stable, name-sorted order so the picker doesn't jump around between launches.
            customs.sort_by_key(|a| a.name.to_lowercase());
            for entry in customs {
                // Built-ins win on key collision — never let a file hide a default.
                if !entries.iter().any(|e| e.key == entry.key) {
                    entries.push(entry);
                }
            }
        }

        Self { entries }
    }

    /// Re-scan the themes directory. Lets the user install a theme and pick it up without a
    /// restart (the Settings screen exposes this).
    pub fn reload(&mut self) {
        *self = Self::load();
    }

    /// Every selectable theme, in picker order (built-ins first).
    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    /// Look up an entry by its stable key.
    pub fn get(&self, key: &str) -> Option<&ThemeEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// The default entry — the built-in [`DEFAULT_KEY`] theme, which is always present.
    pub fn default_entry(&self) -> &ThemeEntry {
        self.get(DEFAULT_KEY)
            .or_else(|| self.entries.first())
            .expect("registry always has at least the built-in themes")
    }

    /// Resolve a saved key to its colour set, falling back to the default if it's unknown
    /// (e.g. a custom theme file the user has since deleted).
    pub fn theme_of(&self, key: &str) -> Theme {
        self.get(key)
            .map(|e| e.theme)
            .unwrap_or_else(|| self.default_entry().theme)
    }

    /// Resolve a saved key to a valid key — the key itself if known, else [`DEFAULT_KEY`].
    pub fn resolve_key(&self, key: &str) -> String {
        if self.get(key).is_some() {
            key.to_string()
        } else {
            self.default_entry().key.clone()
        }
    }
}

/// Read and parse every `*.json` theme in `dir`. A missing directory or an individual bad
/// file is silently skipped — a broken theme should never stop the app from starting.
fn load_custom_themes(dir: &Path) -> Vec<ThemeEntry> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    read_dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            let bytes = std::fs::read(&path).ok()?;
            let file: ThemeFile = serde_json::from_slice(&bytes).ok()?;
            Some(ThemeEntry {
                key: stem,
                name: file.name.clone(),
                author: file.author.clone(),
                builtin: false,
                theme: file.to_theme(),
            })
        })
        .collect()
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The compiled-in themes, in picker order. Their keys are stable and reserved (custom
/// files can't shadow them).
fn builtins() -> Vec<ThemeEntry> {
    vec![
        ThemeEntry {
            key: "carbon".into(),
            name: "Carbon".into(),
            author: None,
            builtin: true,
            theme: carbon(),
        },
        ThemeEntry {
            key: "midnight".into(),
            name: "Midnight".into(),
            author: None,
            builtin: true,
            theme: midnight(),
        },
        ThemeEntry {
            key: "daylight".into(),
            name: "Daylight".into(),
            author: None,
            builtin: true,
            theme: daylight(),
        },
        ThemeEntry {
            key: "blue-studio".into(),
            name: "Blue Studio".into(),
            author: None,
            builtin: true,
            theme: blue_studio(),
        },
        ThemeEntry {
            key: "onyx".into(),
            name: "Onyx".into(),
            author: None,
            builtin: true,
            theme: onyx(),
        },
    ]
}

/// Near-black, neutral. Editor wells fall all the way to true black; panels lift just
/// enough to separate. One confident blue accent carries through.
fn carbon() -> Theme {
    Theme {
        is_dark: true,
        base: rgb(0x0a, 0x0a, 0x0b),
        panel: rgb(0x0e, 0x0e, 0x10),
        surface: rgb(0x1b, 0x1b, 0x1e),
        surface_hover: rgb(0x26, 0x26, 0x2a),
        code_bg: rgb(0x00, 0x00, 0x00),
        stripe: rgb(0x14, 0x14, 0x16),
        selection: rgb(0x21, 0x2e, 0x52),
        border: rgb(0x18, 0x18, 0x1b),
        border_strong: rgb(0x2a, 0x2a, 0x2f),
        text: rgb(0xe8, 0xe8, 0xea),
        text_weak: rgb(0x9a, 0x9a, 0xa0),
        text_faint: rgb(0x5f, 0x5f, 0x66),
        accent: rgb(0x6e, 0x8e, 0xff),
        accent_hover: rgb(0x84, 0x9f, 0xff),
        on_accent: rgb(0xf6, 0xf8, 0xff),
        success: rgb(0x4a, 0xcf, 0x8b),
        danger: rgb(0xee, 0x6a, 0x6a),
        warning: rgb(0xe0, 0xaf, 0x68),
        syntax: None,
    }
}

/// The original plusplus look: a calm, slightly-cool dark palette (Linear / Raycast).
fn midnight() -> Theme {
    Theme {
        is_dark: true,
        base: rgb(0x15, 0x16, 0x1a),
        panel: rgb(0x1a, 0x1c, 0x21),
        surface: rgb(0x23, 0x26, 0x2d),
        surface_hover: rgb(0x2c, 0x30, 0x39),
        code_bg: rgb(0x11, 0x12, 0x16),
        stripe: rgb(0x1e, 0x21, 0x27),
        selection: rgb(0x2a, 0x37, 0x5e),
        border: rgb(0x20, 0x23, 0x2a),
        border_strong: rgb(0x30, 0x35, 0x40),
        text: rgb(0xe4, 0xe6, 0xea),
        text_weak: rgb(0x99, 0xa0, 0xac),
        text_faint: rgb(0x68, 0x6f, 0x7d),
        accent: rgb(0x6e, 0x8e, 0xff),
        accent_hover: rgb(0x84, 0x9f, 0xff),
        on_accent: rgb(0xf6, 0xf8, 0xff),
        success: rgb(0x4a, 0xcf, 0x8b),
        danger: rgb(0xee, 0x6a, 0x6a),
        warning: rgb(0xe0, 0xaf, 0x68),
        syntax: None,
    }
}

/// A clean light theme: white wells, soft grey panels, a stronger accent for contrast.
/// Its surface/border steps mirror the dark themes' spacing (surface clearly darker
/// than panel, borders clearly darker than surface) — with near-white steps, tabs,
/// buttons, and box borders all washed into the background.
fn daylight() -> Theme {
    Theme {
        is_dark: false,
        base: rgb(0xff, 0xff, 0xff),
        panel: rgb(0xf4, 0xf5, 0xf7),
        surface: rgb(0xea, 0xec, 0xf1),
        surface_hover: rgb(0xdf, 0xe3, 0xe9),
        code_bg: rgb(0xff, 0xff, 0xff),
        stripe: rgb(0xee, 0xf0, 0xf4),
        selection: rgb(0xd6, 0xe2, 0xff),
        border: rgb(0xe2, 0xe5, 0xeb),
        border_strong: rgb(0xc8, 0xce, 0xd8),
        text: rgb(0x1c, 0x1f, 0x26),
        text_weak: rgb(0x4a, 0x52, 0x60),
        text_faint: rgb(0x6b, 0x74, 0x82),
        accent: rgb(0x3b, 0x6f, 0xff),
        accent_hover: rgb(0x2f, 0x60, 0xf0),
        on_accent: rgb(0xff, 0xff, 0xff),
        success: rgb(0x1f, 0x9d, 0x57),
        danger: rgb(0xd8, 0x3a, 0x3a),
        warning: rgb(0xb6, 0x80, 0x2a),
        syntax: None,
    }
}

/// Focused editor charcoal inspired by professional code workspaces. Neutral surfaces keep
/// dense data quiet while one crisp blue carries selection, active tabs, links, and SQL keywords.
fn blue_studio() -> Theme {
    Theme {
        is_dark: true,
        base: rgb(0x18, 0x18, 0x18),
        panel: rgb(0x20, 0x20, 0x20),
        surface: rgb(0x25, 0x25, 0x26),
        surface_hover: rgb(0x2a, 0x2d, 0x2e),
        code_bg: rgb(0x15, 0x15, 0x15),
        stripe: rgb(0x24, 0x24, 0x24),
        selection: rgb(0x26, 0x4f, 0x78),
        border: rgb(0x2b, 0x2b, 0x2b),
        border_strong: rgb(0x3c, 0x3c, 0x3c),
        text: rgb(0xd4, 0xd4, 0xd4),
        text_weak: rgb(0x9d, 0x9d, 0x9d),
        text_faint: rgb(0x6e, 0x6e, 0x6e),
        accent: rgb(0x37, 0x94, 0xff),
        accent_hover: rgb(0x5a, 0xa7, 0xff),
        on_accent: rgb(0xff, 0xff, 0xff),
        success: rgb(0x89, 0xd1, 0x85),
        danger: rgb(0xf4, 0x87, 0x71),
        warning: rgb(0xcc, 0xa7, 0x00),
        syntax: None,
    }
}

/// Near-black editor workspace with green actions and neutral selections. SQL keeps the
/// reference's blue, green, and pink highlights independently of the primary action colour.
fn onyx() -> Theme {
    Theme {
        is_dark: true,
        base: rgb(0x0b, 0x0b, 0x0b),
        panel: rgb(0x1b, 0x1b, 0x1b),
        surface: rgb(0x1b, 0x1b, 0x1b),
        surface_hover: rgb(0x28, 0x28, 0x28),
        code_bg: rgb(0x11, 0x11, 0x11),
        stripe: rgb(0x16, 0x16, 0x16),
        selection: rgb(0x24, 0x24, 0x24),
        border: rgb(0x2a, 0x2a, 0x2a),
        border_strong: rgb(0x3b, 0x3b, 0x3b),
        text: rgb(0xb8, 0xb8, 0xb8),
        text_weak: rgb(0x92, 0x92, 0x92),
        text_faint: rgb(0x68, 0x68, 0x68),
        accent: rgb(0x86, 0xaa, 0x3d),
        accent_hover: rgb(0xa5, 0xc8, 0x68),
        on_accent: rgb(0x11, 0x11, 0x11),
        success: rgb(0x86, 0xaa, 0x3d),
        danger: rgb(0xcc, 0x42, 0x6b),
        warning: rgb(0xc5, 0x9b, 0x39),
        syntax: Some(SyntaxTheme {
            keyword: rgb(0x4f, 0x7f, 0xc6),
            string: rgb(0x86, 0xaa, 0x3d),
            number: rgb(0xc5, 0x9b, 0x39),
            comment: rgb(0x68, 0x68, 0x68),
            punctuation: rgb(0x92, 0x92, 0x92),
            identifier: rgb(0xb8, 0xb8, 0xb8),
            qualified_identifier: rgb(0xcc, 0x42, 0x6b),
        }),
    }
}

thread_local! {
    static CURRENT: Cell<Theme> = Cell::new(carbon());
}

/// The colour set in effect right now. Cheap (a `Cell` read of a `Copy` struct).
pub fn current() -> Theme {
    CURRENT.with(Cell::get)
}

/// Switch the active theme to a resolved colour set. Callers should re-run
/// [`crate::style::apply`] afterwards so egui's `Visuals` pick up the new colours.
pub fn set_current(theme: Theme) {
    CURRENT.with(|c| c.set(theme));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default key always resolves to a built-in entry.
    #[test]
    fn default_key_is_a_builtin() {
        let reg = ThemeRegistry {
            entries: builtins(),
        };
        let entry = reg.get(DEFAULT_KEY).expect("default present");
        assert!(entry.builtin);
        assert_eq!(entry.key, DEFAULT_KEY);
    }

    #[test]
    fn bundled_presets_are_built_in() {
        let reg = ThemeRegistry {
            entries: builtins(),
        };
        assert_eq!(
            reg.entries()
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["carbon", "midnight", "daylight", "blue-studio", "onyx"]
        );
        assert!(reg.entries().iter().all(|entry| entry.builtin));
        for key in ["midnight-conversational", "graphite", "intellij-light"] {
            assert!(reg.get(key).is_none());
            assert_eq!(reg.resolve_key(key), "carbon");
        }
    }

    /// An unknown key falls back to the default colour set, never panics.
    #[test]
    fn unknown_key_falls_back_to_default() {
        let reg = ThemeRegistry {
            entries: builtins(),
        };
        assert_eq!(reg.resolve_key("does-not-exist"), DEFAULT_KEY);
        // theme_of returns the default's colours (compare a token).
        assert_eq!(
            reg.theme_of("does-not-exist").base,
            reg.default_entry().theme.base
        );
    }

    /// A ThemeFile round-trips through JSON with hex colours intact.
    #[test]
    fn theme_file_round_trips_through_json() {
        // Author a minimal theme by serializing a built-in via ThemeFile.
        let json = r##"{
            "name": "Test",
            "author": "me",
            "is_dark": true,
            "base": "#0a0a0b",
            "panel": "#0e0e10",
            "surface": "#1b1b1e",
            "surface_hover": "#26262a",
            "code_bg": "#000000",
            "stripe": "#141416",
            "selection": "#212e52",
            "border": "#232327",
            "border_strong": "#34343a",
            "text": "#e8e8ea",
            "text_weak": "#9a9aa0",
            "text_faint": "#5f5f66",
            "accent": "#6e8eff",
            "accent_hover": "#849fff",
            "on_accent": "#f6f8ff",
            "success": "#4acf8b",
            "danger": "#ee6a6a",
            "warning": "#e0af68"
        }"##;
        let file: ThemeFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.name, "Test");
        assert_eq!(file.author.as_deref(), Some("me"));
        let theme = file.to_theme();
        assert_eq!(theme.accent, Color32::from_rgb(0x6e, 0x8e, 0xff));
        assert_eq!(theme.code_bg, Color32::from_rgb(0, 0, 0));
        assert!(theme.syntax.is_none());

        // Re-serialize and parse again; colours survive.
        let back: ThemeFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(back.accent, file.accent);
    }

    #[test]
    fn optional_syntax_palette_round_trips() {
        let json = include_str!("../../../../examples/themes/intellij-light.json");
        let file: ThemeFile = serde_json::from_str(json).unwrap();
        let back: ThemeFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(
            back.to_theme().syntax.unwrap().qualified_identifier,
            file.syntax.unwrap().qualified_identifier
        );
    }

    /// `#rgb` shorthand expands to the full byte form.
    #[test]
    fn short_hex_expands() {
        let json = r##"{ "c": "#f80" }"##;
        #[derive(serde::Deserialize)]
        struct W {
            #[serde(with = "hex")]
            c: Color32,
        }
        let w: W = serde_json::from_str(json).unwrap();
        assert_eq!(w.c, Color32::from_rgb(0xff, 0x88, 0x00));
    }
}
