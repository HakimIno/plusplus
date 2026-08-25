# Changelog

Notable user-visible changes are documented here. The project follows semantic versioning
while pre-1.0 releases may still change workflows and configuration formats.

## 0.4.2 — 2026-08-25

- Redesigned New Connection around a provider picker, then a focused details form with optional appearance, safety, SSL, and SSH controls.
- Added Settings typography so imported OpenType fonts can replace the interface and SQL/grid faces.
- Tightened the SQL autocomplete popup with kind icons, better on-screen placement, and clipped labels.
- Production Guardian no longer interrupts append-only INSERT; REPLACE, ON CONFLICT UPDATE, and ON DUPLICATE KEY still require confirmation.
- Speed up CI and release packaging with a non-optimizing test profile, and start building installers after Linux tests pass while macOS and Windows tests still block publish.

## 0.4.1 — 2026-08-24

- Made the SQL editor, History sidebar, and streaming query results cheaper on every frame by caching fold and layout work, grouping history once, and appending streamed rows instead of rebuilding the whole grid.
- Cached SQL highlighting in History, Saved Queries, and production-guard previews so those panels stay responsive while scrolling.
- Stopped the active-tab water animation from repainting the window continuously after a tab is selected.
- Prepared result-filter conditions once per view instead of re-parsing them for every row.
- Dropped debug info from the DuckDB C++ engine in dev builds so `target/` does not balloon with multi-GB object files.

## 0.4.0 — 2026-08-19

- Restyled the History sidebar to match a date-grouped log: collapsible day folders with
  the same chevron and folder icons as Items and Queries, the time above each statement,
  and wrapped syntax-highlighted SQL. Hovering an entry shows a side callout with an arrow,
  highlighted SQL, and row timing — the same chrome as rename. Run from History now opens a
  Query tab and executes the statement instead of overwriting the current table tab.
- Restyled Saved Queries into a folder tree that lists only query names. New queries land
  in Ungrouped; folders can be created, renamed, reordered by drag-and-drop, and used as
  drop targets to move queries. Clicking a name still opens a Query tab. Hovering a query
  shows a side callout with highlighted SQL and an arrow pointing at the row. Rename uses
  the same callout shape, not an inline editor or a modal.
- Matched the Items schema tree to that same folder/file spacing: Views, Functions,
  Procedures, and Triggers use folder rows with indented file-style children.
- Restyled the production-guard confirm dialog to match the rest of the app: connection
  row with a database icon, statement cards with type and risk badges, a bordered SQL
  preview, a confirmation chip plus themed input, and a danger Run button when a phrase
  is required.
- Schema apply skips the extra Preview Migration dialog: production connections
  review the generated DDL in Production Guardian, and other connections apply it
  immediately.
- Collapsed the five title-bar layout toggles into one Layout icon that opens a
  popover of panel glyphs — click a tile to show or hide that chrome, with no
  checkboxes.
- Moved the result filter toggle from the title bar into the pager cluster next to
  page navigation, and bound Cmd/Ctrl+F to show or hide the filter strip.
- Added an embedded DuckDB backend for local analytical databases and Parquet files, including
  in-memory databases, schema introspection, and dialect-aware SQL.
- Added performance safeguards around query memory, tab eviction, stream byte ceilings, and
  keyset pagination, with a recorded Criterion suite for the hot paths.
- Made connecting cancellable so metadata loading can be stopped without leaving the UI stuck,
  and capped the emoji texture cache to bound memory use.

## 0.3.1 — 2026-08-14

- Made SQL autocomplete feel more immediate by showing unambiguous keyword, table, and column
  completions as inline ghost text accepted with Tab, while preserving the popup for ambiguous
  or quoted matches and adding schema-aware INSERT, UPDATE, and DELETE scaffolds.
- Added exact background row counts and clear visible row ranges to table pagination without
  delaying the first page, and kept the pager available while inspecting table structure or
  indexes.
- Split the core connection and data models into focused modules and moved SQLite workflow
  coverage into integration tests without changing the public database API.

## 0.3.0 — 2026-08-11

- Redesigned table browsing around a faster virtualized grid with content-aware columns,
  background page prefetching, strict row limits, and smoother navigation controls.
- Reworked table Structure and Indexes into compact inline-editable grids, with broader
  dialect-specific data types and keyboard editing consistent with the Data grid.
- Added a persistent, resizable Live Log panel while keeping History focused on activity
  performed inside PlusPlus, grouped by local date and searchable from the sidebar.
- Refined navigation, tabs, menus, icons, empty states, and the product website for a more
  consistent interface across database backends.

## 0.2.26 — 2026-08-05

- Improved result-grid readability with content-aware initial column widths, centered headers,
  and type-aware cell alignment while avoiding unnecessary filter and row-buffer work.
- Made paged table results appear before their potentially expensive row count, and reuse a
  known total while navigating instead of issuing the same `COUNT(*)` for every page.
- Fixed Cassandra and ScyllaDB connections through localhost port forwarding by translating
  advertised peer addresses to the reachable endpoint, with a local ScyllaDB example stack.

## 0.2.25 — 2026-08-03

- Added code folding to the SQL editor: chevrons in the line-number gutter collapse whole
  statements, bracketed groups, `BEGIN`/`CASE` blocks and comment runs into a `⋯ N lines`
  marker, which opens again on a click. The query itself is never rewritten — editing,
  completion and diagnostics keep working against the full text while a region is collapsed.
- Added a subtle water animation behind the active query tab.
- Added per-connection Safety Profiles: Development allows normal work, Staging enables
  Production Guardian, Production enforces hard read-only access, and Custom preserves the
  independent Guardian/read-only controls. Existing saved connections keep their behavior.

## 0.2.24 — 2026-07-31

- Added dialect-aware live SQL syntax diagnostics: after a short typing pause the editor
  underlines the first invalid token, explains it on hover, follows the active connection's
  dialect, and stays out of the way while the user is still editing that token.
- Refreshed interface controls with a consistent Hugeicons set, highlighted matched
  autocomplete prefixes, sped up fit-column measurements on large result sets, and fixed
  Cassandra/ScyllaDB logos so they remain visible in dark themes.
- Refined the landing page with database-vendor cards, platform and feature icons, and clearer
  visual grouping while keeping the existing release-download flow.
- Hardened releases by running the full cross-platform CI suite before packaging, rejecting
  tags that disagree with `Cargo.toml`, and documenting the actual Rust 1.94 minimum.

## 0.2.23 — 2026-07-24

- Added Cassandra and ScyllaDB support: one CQL backend serves both wire-compatible engines,
  connecting over the native protocol with TLS (encrypt-only through full verification) and,
  where needed, through an SSH bastion — peer discovery is pinned to the tunnel so it can't
  leak around it.
- Introspects keyspaces, tables, columns (with partition/clustering keys flagged as primary),
  secondary indexes, materialized views, and user-defined functions; keyspaces appear in the
  database switcher and the connection form labels the field "Keyspace".
- Reads stream page by page and stop at the row cap, queries are cancellable, and every CQL
  type decodes — including collections, tuples, UDTs, decimals, varints, and durations, shown
  as read-only CQL literals in the grid.
- Adapted schema editing to CQL's shape: `CREATE TABLE`/`ALTER`/index/`TRUNCATE`/rename emit
  valid CQL, single-row `INSERT` and `TRUE`/`FALSE` booleans are used, and operations CQL
  lacks (foreign keys, joins, views, triggers, routines, table cloning, transactions) are
  hidden or refused rather than generating statements the server rejects.

## 0.2.22 — 2026-07-21

- Fixed intermittent query failures caused by concurrent runs racing each other: starting a
  query now supersedes (cancels) the one still in flight, late results from superseded runs
  can no longer overwrite fresh rows, steal a tab's editability, or corrupt the busy state,
  and Cmd/Ctrl+Enter while busy shows a clear status hint instead of silently double-running.
- Made multi-statement scripts work on MySQL/MariaDB by running them statement by statement
  (the driver cannot send a `;`-separated batch); the grid shows the last result set, rows
  affected are summed, and a failure reports its statement number.
- Fixed `INSERT/UPDATE/DELETE … RETURNING` and `CALL` showing an empty result: they are now
  routed through the row-returning path instead of silently dropping their rows.
- Taught the statement splitter Postgres dollar-quoting, so `CREATE FUNCTION … $$ … ; … $$`
  bodies are no longer split at inner semicolons by Production Guardian and batch analysis.
- Turned full-schema ER diagrams into portable designers: tables, columns, indexes, and foreign
  keys can be edited, exported/imported as versioned `.plusplus-er.json` files (including canvas
  layout), and forward-engineered through the existing migration preview into PostgreSQL,
  MySQL/MariaDB, SQL Server, or SQLite.
- Added portable type translation, schema remapping, relationship validation, and two-phase
  foreign-key creation so one design can safely target different database connections.

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
