# Custom themes

plusplus ships three built-in themes (Carbon, Midnight, Daylight). You can also install
your own — a theme is just a small JSON file of colours, no recompile required. This is the
first plugin "contribution point": more contribution types (snippets, keybindings, WASM
plugins) will follow the same drop-a-file model.

## Installing a theme

1. Find the themes folder (created on demand under your config directory):

   | OS | Path |
   |---|---|
   | Linux / macOS | `~/.config/plusplus/themes/` |
   | Linux (XDG) | `$XDG_CONFIG_HOME/plusplus/themes/` |
   | Windows | `%APPDATA%\plusplus\themes\` |

2. Copy a `*.json` theme file into it. A ready-made example lives at
   [`examples/themes/dracula.json`](../examples/themes/dracula.json).

3. In plusplus, open **Settings → Appearance** and click **Reload themes** (or restart).
   Your theme appears in the picker next to the built-ins.

The file name (without `.json`) is the theme's stable id, persisted to `settings.json`.
If you later delete a selected theme file, plusplus falls back to the default (Carbon).

## Authoring a theme

Copy `examples/themes/dracula.json` and edit the colours. Every field is required and is an
opaque `#rrggbb` (or shorthand `#rgb`) hex string, except `name` (display name), the optional
`author`, and `is_dark` (a boolean that selects egui's dark/light base).

```json
{
  "name": "My Theme",
  "author": "you",
  "is_dark": true,

  "base":          "#0a0a0b",
  "panel":         "#0e0e10",
  "surface":       "#1b1b1e",
  "surface_hover": "#26262a",
  "code_bg":       "#000000",
  "stripe":        "#141416",
  "selection":     "#212e52",
  "border":        "#232327",
  "border_strong": "#34343a",
  "text":          "#e8e8ea",
  "text_weak":     "#9a9aa0",
  "text_faint":    "#5f5f66",
  "accent":        "#6e8eff",
  "accent_hover":  "#849fff",
  "on_accent":     "#f6f8ff",
  "success":       "#4acf8b",
  "danger":        "#ee6a6a",
  "warning":       "#e0af68"
}
```

### What each token does

| Token | Used for |
|---|---|
| `base` | App / window background |
| `panel` | Side and tool panels |
| `surface` / `surface_hover` | Raised controls (buttons, inputs, list items) and their hover |
| `code_bg` | SQL editor and other text wells (the deepest surface) |
| `stripe` | Alternate / striped table rows |
| `selection` | Selected-row / text-selection fill |
| `border` / `border_strong` | Hairlines and stronger separators |
| `text` / `text_weak` / `text_faint` | Primary, secondary, and tertiary text |
| `accent` / `accent_hover` | Primary action colour (buttons, links, focus) |
| `on_accent` | Text/icon colour painted on top of an accent fill |
| `success` / `danger` / `warning` | Semantic status colours |

A malformed or unreadable theme file is skipped silently — it never blocks startup. A file
whose name collides with a built-in key (`carbon`, `midnight`, `daylight`) is ignored so the
defaults are always available.
