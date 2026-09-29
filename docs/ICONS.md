# Interface icons — Tabler

`crates/ui/src/icons.rs` is the shared icon registry. Pick icons by meaning,
not by screen: tables use `table()`, columns use `column()`, SQL and keywords
use `code()`, functions and stored procedures use `function()`, and copying
uses `copy()` everywhere.
Autocomplete, navigation, tabs, and context menus use the same assets.

Use [Tabler Icons](https://github.com/tabler/tabler-icons) for all interface
icons. SVGs live in `crates/ui/assets/icons/outline/` and `filled/`, named
after their upstream icon. They retain Tabler's 24 × 24 viewBox, 2-unit
outline strokes, and rounded caps and joins. `currentColor` is replaced
with white so egui can tint the texture. The default display size is 16 points;
compact controls can scale the same asset down. Use theme text colours for
object icons, reserving accent/status colours for interaction and status.
The table list keeps its primary/accent tint (including its drag preview),
with text colour on selected rows for contrast.
Query/object tabs and successful result tabs use the primary/accent icon tint
in both active and inactive states; failed result tabs retain their error tint.
The filled star is the selected variant of the outline star.

Database provider marks in `assets/icondb/` use transparent white artwork,
tinted to the theme's text colour (light on dark themes, dark on light themes).
Use one asset per provider, with tight viewBox padding and no coloured tiles.
The MIT license is preserved in `crates/ui/assets/icons/LICENSE-TABLER.md`.
`crates/ui/assets/icons/tabler.json` records the pinned source revision,
every semantic mapping, and the only transformations applied to the SVGs.

Key mappings:

| Meaning | Tabler icon |
| --- | --- |
| SQL / code / keyword | `tabler:terminal-2` |
| Table | `tabler:table` |
| Column | `tabler:columns-3` |
| Function / stored procedure | `tabler:math-function` |
| View | `tabler:eye` |
| Copy | `tabler:copy` |
| Schema diagram | `tabler:sitemap` |
| Favourite | `tabler:star` / `tabler:star-filled` |

Before adding an icon, check the registry for an existing semantic match.
Choose an upstream Tabler SVG from the pinned revision and update the mapping
manifest; preserve its geometry and standard stroke. Bundle only the icons
used by the app, with no runtime CDN, icon font, or full icon-library dependency.
When removing the last caller, remove both the registration and SVG; do not
suppress dead-code warnings. Preview the complete registry in both themes with:

```sh
UPDATE_SNAPSHOTS=1 cargo test -p plusplus-ui snapshot_icon_gallery --lib -- --ignored --test-threads=1
```
