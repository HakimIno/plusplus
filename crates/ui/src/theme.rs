//! Theme system: named colour palettes the user can switch between at runtime.
//!
//! A [`Theme`] is a flat set of colour tokens (the same tokens [`crate::style::palette`]
//! exposes). The app keeps one "current" theme in a thread-local cell — egui is
//! single-threaded on the UI side, so a cheap `Cell` is all we need. UI code reads
//! colours through `palette::*` accessors, which forward to [`current`]; switching themes
//! is just [`set_current`] followed by re-applying the style.

use std::cell::Cell;

use egui::Color32;

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
}

/// One of the built-in themes the user can choose from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeId {
    /// Near-black, neutral. The default — deep blacks in the spirit of an OLED editor.
    Carbon,
    /// A calm, slightly-cool dark palette (the original plusplus look).
    Midnight,
    /// A clean light theme for bright environments.
    Daylight,
}

impl ThemeId {
    /// All themes, in the order they should appear in a picker.
    pub const ALL: [ThemeId; 3] = [ThemeId::Carbon, ThemeId::Midnight, ThemeId::Daylight];

    /// The default theme used on first run (and when a saved choice can't be parsed).
    pub const DEFAULT: ThemeId = ThemeId::Carbon;

    /// Human-readable name for the picker.
    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Carbon => "Carbon",
            ThemeId::Midnight => "Midnight",
            ThemeId::Daylight => "Daylight",
        }
    }

    /// Stable identifier used when persisting the choice to disk.
    pub fn key(self) -> &'static str {
        match self {
            ThemeId::Carbon => "carbon",
            ThemeId::Midnight => "midnight",
            ThemeId::Daylight => "daylight",
        }
    }

    /// Parse a persisted [`key`](Self::key) back into a `ThemeId`.
    pub fn from_key(s: &str) -> Option<ThemeId> {
        ThemeId::ALL.into_iter().find(|t| t.key() == s)
    }

    /// The concrete colour set for this theme.
    pub fn theme(self) -> Theme {
        match self {
            ThemeId::Carbon => carbon(),
            ThemeId::Midnight => midnight(),
            ThemeId::Daylight => daylight(),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
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
        border: rgb(0x23, 0x23, 0x27),
        border_strong: rgb(0x34, 0x34, 0x3a),
        text: rgb(0xe8, 0xe8, 0xea),
        text_weak: rgb(0x9a, 0x9a, 0xa0),
        text_faint: rgb(0x5f, 0x5f, 0x66),
        accent: rgb(0x6e, 0x8e, 0xff),
        accent_hover: rgb(0x84, 0x9f, 0xff),
        on_accent: rgb(0xf6, 0xf8, 0xff),
        success: rgb(0x4a, 0xcf, 0x8b),
        danger: rgb(0xee, 0x6a, 0x6a),
        warning: rgb(0xe0, 0xaf, 0x68),
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
        border: rgb(0x2b, 0x2f, 0x38),
        border_strong: rgb(0x3a, 0x40, 0x4c),
        text: rgb(0xe4, 0xe6, 0xea),
        text_weak: rgb(0x99, 0xa0, 0xac),
        text_faint: rgb(0x68, 0x6f, 0x7d),
        accent: rgb(0x6e, 0x8e, 0xff),
        accent_hover: rgb(0x84, 0x9f, 0xff),
        on_accent: rgb(0xf6, 0xf8, 0xff),
        success: rgb(0x4a, 0xcf, 0x8b),
        danger: rgb(0xee, 0x6a, 0x6a),
        warning: rgb(0xe0, 0xaf, 0x68),
    }
}

/// A clean light theme: white wells, soft grey panels, a stronger accent for contrast.
fn daylight() -> Theme {
    Theme {
        is_dark: false,
        base: rgb(0xff, 0xff, 0xff),
        panel: rgb(0xf4, 0xf5, 0xf7),
        surface: rgb(0xf7, 0xf8, 0xfa),
        surface_hover: rgb(0xec, 0xee, 0xf1),
        code_bg: rgb(0xff, 0xff, 0xff),
        stripe: rgb(0xf4, 0xf6, 0xf8),
        selection: rgb(0xd6, 0xe2, 0xff),
        border: rgb(0xe2, 0xe5, 0xea),
        border_strong: rgb(0xc7, 0xcc, 0xd4),
        text: rgb(0x1c, 0x1f, 0x26),
        text_weak: rgb(0x5b, 0x62, 0x6e),
        text_faint: rgb(0x8b, 0x93, 0xa1),
        accent: rgb(0x3b, 0x6f, 0xff),
        accent_hover: rgb(0x2f, 0x60, 0xf0),
        on_accent: rgb(0xff, 0xff, 0xff),
        success: rgb(0x1f, 0x9d, 0x57),
        danger: rgb(0xd8, 0x3a, 0x3a),
        warning: rgb(0xb6, 0x80, 0x2a),
    }
}

thread_local! {
    static CURRENT: Cell<Theme> = Cell::new(ThemeId::DEFAULT.theme());
}

/// The colour set in effect right now. Cheap (a `Cell` read of a `Copy` struct).
pub fn current() -> Theme {
    CURRENT.with(Cell::get)
}

/// Switch the active theme. Callers should re-run [`crate::style::apply`] afterwards so
/// egui's `Visuals` pick up the new colours.
pub fn set_current(id: ThemeId) {
    CURRENT.with(|c| c.set(id.theme()));
}
