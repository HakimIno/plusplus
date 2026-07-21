# Changelog

Notable user-visible changes are documented here. The project follows semantic versioning
while pre-1.0 releases may still change workflows and configuration formats.

## Unreleased

## 0.2.21 — 2026-07-21

- Redesigned the first-run welcome screen as a full-window scene: an accent-tinted layered
  landscape, a speech-bubble intro with the feature list, one-click theme swatches, the
  mascot, and a full-width Get Started action (Enter works too). The window can be dragged
  from the top strip, and Linux/Windows keep their close/maximize/minimize buttons.
- Moved Settings out of a dialog into a full workspace tab with General, Appearance, and
  Privacy sections, sharing the query-tab strip.
- Added three built-in themes — Lotus Dusk, Tidal Ledger, and Copper Circuit — with their
  JSON sources in `examples/themes/` as authoring references.
- Fixed a potential crash on very large or high-DPI displays: the welcome backdrop now
  rasterizes at a fixed size instead of scaling with the window.
- Made the UI test suite hermetic: tests run against an isolated config directory and can no
  longer overwrite the machine's real settings, workspace tabs, or connections.

## 0.2.20 — 2026-07-21

- Added Production Guardian for destructive SQL on production connections, with dialect-aware
  AST analysis, safe row estimates, compact query-plan evidence, risk levels, typed confirmation
  for critical operations, immutable query snapshots, mandatory fail-closed audit events, and
  live preflight verification for PostgreSQL, MySQL, and SQL Server.
- Fixed ER diagram relationship resolution across PostgreSQL schemas, skipped ambiguous fallback
  targets, and prevented diagrams from opening before full relationship metadata is available.
- Let table and schema-object designers use the full tab workspace without unrelated query and
  result controls surrounding the form.

## 0.2.19 — 2026-07-17

- Added full-schema and table-focused ER diagrams in dedicated tabs, with relationship-depth
  controls, refresh, re-layout, zoom-to-fit, and snapshots that remain viewable after disconnecting.
- Reworked ER diagram layout and rendering for clearer left-to-right relationships and responsive
  navigation of large schemas, with new diagram toolbar icons and visual snapshots.
- Kept table and view result controls together with their resizable bottom query editor.

## 0.2.18 — 2026-07-16

- Count paged table rows asynchronously so results render immediately and the pager updates
  from `of ?` to the exact total in real time without blocking the data grid.
- Consolidated deployment into one tag-only Release workflow containing macOS, Linux,
  Windows, and publishing jobs; ordinary commits no longer start runners.

## 0.2.17 — 2026-07-16

- Redesigned query and table workflows with adaptive editor placement, cleaner tabs, saved
  queries, result Data/Message/Chart views, and clearer inline query errors.
- Improved the data grid with full-width scrolling, resizable and content-fitted columns,
  refined headers and column action menus, and more reliable row editing.
- Refreshed database provider icons, the schema explorer, draggable table ordering, and the
  empty-result sheep mascot.

## 0.2.16 — 2026-07-15

- Sped up queries and reconnection across the MySQL, PostgreSQL, and SQL Server backends:
  pooled connections no longer run a liveness ping before every query, keep one connection
  warm, and fail an unreachable host in a few seconds instead of stalling.
- Ad-hoc statements now run on the simple/text protocol, saving a network round trip per query
  and letting multi-statement batches run on MySQL and PostgreSQL.

## 0.2.15 — 2026-07-14

- Split the main application implementation into focused workflow modules without changing
  the public application model.
- Standardized form controls and refreshed UI snapshot coverage for imports, menus, schema
  browsing, triggers, and foreign keys.
- Reworked the project landing page and contribution documentation.
- Documented native platform-signing limitations and the public roadmap.
- Added Linux/macOS quality checks and live PostgreSQL, MySQL, and SQL Server smoke tests.
- Prepared optional Apple notarization and Windows Authenticode hooks in the release workflow.

## 0.2.14 — 2026-07-13

- Reduced connection startup time by loading overview metadata before full schema details.
- Improved SQL autocomplete and ghost-text context across aliases and statements.
- Virtualized schema object lists for large databases.
- Published macOS, Windows, and Linux release packages with Minisign signatures.

## Earlier releases

See [GitHub Releases](https://github.com/HakimIno/plusplus/releases) for generated notes and
downloadable assets from 0.1.0 onward.
