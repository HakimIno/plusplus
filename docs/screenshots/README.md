# Screenshots

This directory is the home for polished product screenshots and short demo GIFs. The
top-level README currently uses a stable UI snapshot from `crates/ui/tests/snapshots/` so
it always renders. Replace it with a real product capture before public launch; progress is
tracked in [issue #1](https://github.com/HakimIno/plusplus/issues/1).

| File | What to capture |
|---|---|
| `hero.png` | The main window: sidebar schema tree on the left, a result grid in the centre, a query open. The money shot — make it look alive. |
| `demo.gif` | A 15–25 second path: open sample → browse → query → stage edit → save. Keep it small enough to load quickly on GitHub. |
| `grid.png` | A big result in the virtualized grid, with the filter bar open and the pager visible (the "1–1,000 of 1,234,567" total). |
| `erd.png` | The ER diagram after that feature is enabled for release; do not advertise hidden UI. |
| `editing.png` | A table tab mid-edit: a green new row and/or a red row marked for deletion. |
| `export.png` | The sidebar right-click menu open on **Export Table → as CSV / JSON** (optional). |

Tips for clean shots:

- Use the bundled `examples/sample.sqlite` — it has realistic foreign keys and a few
  hundred rows, so the schema tree, query editor, and result grid all have something to show.
- Capture on a Retina display and export at 2× for crisp images on GitHub.
- A consistent theme across shots looks best (the **Carbon** dark theme photographs well).
- Trim window shadows/clutter; ~1600px wide is plenty.
- When a new screenshot is ready, add it to the README with a relative path such as
  `docs/screenshots/hero.png`; do not replace the image with an externally hosted URL.
