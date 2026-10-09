# Changelog

Notable user-visible changes are documented here. The project follows semantic versioning
while pre-1.0 releases may still change workflows and configuration formats.

## 0.5.4 — 2026-10-09

- Open database links: clicking a `postgres://`, `postgresql://`, `mysql://`, `mariadb://`, `sqlserver://` or `mssql://` link in a browser opens plusplus with a prefilled New Connection form. Nothing connects or is saved until you confirm, and a link to a connection you already saved opens that connection. Pasting such a URL into the Host field fills the form too. On Windows the installer offers this as an option.
- Fixed double-click editing on PostgreSQL and SQL Server: cells stay editable while the grid fetches more rows as you scroll, instead of ignoring the click until the fetch finished.
- Resize seams fill the whole gap between panels while hovered or dragged, like VS Code, on every draggable edge including split columns and the seam above the Data bar. The seam's dots no longer show through dialogs.
- The Production, Read-only and SSH tunnel checkboxes in the connection form use the primary colour instead of a bare tick.
- DuckDB connections get their own icon.
- Lower memory use: after 20 idle seconds freed memory goes back to the operating system, and the default result memory budget is 256 MB instead of 512 MB.

## 0.5.3 — 2026-10-07

- Follow a foreign key, then go back: the ‹ button on the Data/Structure bar, Cmd/Ctrl+[, or the mouse back button returns to the tab you came from, or re-opens the table a reused preview tab showed before.
- Much faster results grid with large text: cells lay out at most 256 characters (line breaks shown as spaces, full value in Details), so columns of multi-megabyte TEXT or JSON scroll smoothly instead of stalling.
- Editing a number in the grid keeps it right-aligned in the same font; the editor's text no longer jumps when a cell opens.
- Lower memory use: closed tabs release their editor history, the value viewer frees images when closed, and SSH tunnels stop idle forwards when they close.
- The Update button is a neutral grey "Update" pill, and the update check repeats daily while the app is open, so new releases appear without a restart.
- Untitled query tabs are numbered per connection, so the first one is "Query 1" rather than its position across all connections.
- Autocomplete icons are coloured by kind, as in VS Code: orange tables, blue columns, purple functions, grey keywords.
- Switch Database has a search box for long database lists.
- The side gutters on macOS now match the seams between panels.
- Shorter Settings and dialog text.
- Possible fix for a dark band across the selected row in the Structure editor.

## 0.5.2 — 2026-10-06

- New UI font: IBM Plex Sans in Thin, Regular, Medium, SemiBold and Bold, paired with IBM Plex Sans Thai at the same weights so Thai text matches the Latin text beside it. Inter, Anuphan, Geist and Noto Sans Thai are no longer bundled, which also shrinks the app.
- Fixed saving edits: after a successful save the grid used to keep the cells green and show "Save or discard this tab's changes…" even though the write had succeeded. Staged edits are now cleared and the table reloads.
- Fixed tables that opened read-only until you pressed Cmd+R: a restored or reselected table tab lost its edit source on reload, and a primary key that arrived after the tab opened was never picked up.
- Foreign-key editor popover on the Structure tab, with ON UPDATE as well as ON DELETE and references to tables in other schemas.
- Onyx, a near-black built-in theme with a green primary colour.
- Notched mouse wheels now scroll the results grid with inertia.
- The connection rail and each split column's tab strip are rounded cards with the same gutter as every other panel.
- Details: the search field sits beside the close button, side padding and default width are larger, and edited fields get a green background instead of a green border.
- Section headings use sentence case, controls lose their resting outline, and hover transitions are animated.
- The Edit Connection dialog drops its explanatory text, and Clear only appears once a title-bar colour is set.
- Dragging the gap above a bottom-docked bar resizes the dock below it.

## 0.5.1 — 2026-10-03

v0.5.0 was tagged but its build failed on a lint, so it was never published; everything below ships in 0.5.1.

- Split the workspace into up to four columns. Drop a table or tab on a column to add it there, or on the right edge of the last column to open another. Each column has its own tab strip and width, and the seam between columns is now the same four points as every other card seam.
- Details follows the focused column (press in a column to focus it; an accent line marks its tab strip), names the tab its fields come from, and floats over the focused column from three columns up instead of squeezing every grid. Previously it only ever read the left-most column.
- The Data / Structure / Indexes bar sheds its row summary, button labels and pager arrows as a column narrows, so its parts no longer overlap.
- Activity monitor: lists the running sessions of a PostgreSQL, MySQL/MariaDB or SQL Server connection, with confirmed, audited Cancel and Terminate. Open it from the command palette. Verified against PostgreSQL only.
- macOS now ships one DMG per architecture (`plusplus-<version>-aarch64.dmg` and `-x86_64.dmg`), roughly half the download of the universal DMG, which stays attached to this release so apps on 0.4.17 or older can still update.
- Smaller binary: debug info is stripped from release builds and the unused polars dependency and placeholder analysis crate are gone.
- Restored workspaces keep connection-specific tab selections, and split tab strips only show tabs belonging to their pane's selected connection.
- Separated result-mode tabs from Live log into their own rounded dock, with the resize handle between the tabs and log.
- Column sorting now re-queries the database with ORDER BY before paging, restores the original query when cleared, and uses SVG line-and-arrow icons for ascending, descending, and unsorted states in each header.
- Consolidated result rows, columns and query time into one status-bar summary, with connection and version kept in a separate, consistently spaced group.
- Simplified the connection rail with smaller icons, soft selected backgrounds, a slim active marker, aligned status dots, and names fitted to the available width.
- Successful connection tests highlight fields with a soft green background without a green border.
- Simplified the page-window popover to stacked, equal-width Limit/Offset fields and a Load rows button with an SVG icon.
- New connection names now follow the selected database provider, preserving custom names and names of saved connections when using Change.
- Kept four built-in themes: Carbon (the default), Midnight, Daylight, and Blue Studio. Removed built-in selections fall back to Carbon; Graphite and IntelliJ Light remain installable JSON examples. Custom themes can supply an optional SQL colour palette.
- Kept the Live log heading readable on light backgrounds.
- Empty-state artwork now selects a soft grey-blue tugboat illustration on light themes.
- Matched the filter panel's rounded corners and spacing to the other workspace panels.

## 0.4.17 — 2026-10-02

- The status bar shows more: the last result's rows and time, the SQL editor's line and column, the number of staged changes (click to review and save), a running query's clock with a cancel button, the tab's connection with a READ-ONLY flag, and Count / Sum / Avg of the selected rows' numeric column.
- CI: clippy 1.99 no longer fails the build on the `Database` trait.

## 0.4.16 — 2026-10-02

- New Table / View / Trigger / Function / Procedure each open in a draft tab of their own — open as many as you like — and are listed in the explorer (click one to jump to its tab). Esc, or Discard after Cmd+R, closes the draft.
- A new view, trigger or routine is written in the full SQL editor: highlighting, autocomplete, ghost text, hover, error squiggles, folding, find. Cmd+F now works in function / procedure / trigger definition tabs too.
- Trigger and routine forms use the same compact bar as views.
- Drivers that can't create an object (triggers on DuckDB, routines on SQLite, views on Cassandra/ScyllaDB) show it disabled in the + menu with the reason, instead of failing at Apply.
- The SQL editor underlines tables and columns that don't exist in the connected database, and hovering a name shows its type, key, nullability and foreign keys.
- Ghost text suggests the next clause as well as the first.
- `EXPLAIN` results on PostgreSQL and SQLite open as a plan tree with the costly steps called out.
- Queries run per tab: one tab's query no longer blocks or cancels another's. Cancel a running query from its own tab.
- Quitting, reloading or switching with unsaved edits asks first (including Cmd+Q).
- Fonts: `.ttc` files can be imported; the grid can use a monospace font throughout; the interface type scale is one point larger.
- Removed the database icon above the explorer's table list.

## 0.4.15 — 2026-09-30

- Whole-database backup and restore (connection menu, sidebar table menu, Open Anything), with an audit-trail entry for each run.
- Native macOS menu bar.
- ENUM columns are picked from a list of their allowed values instead of typed; booleans pick true / false / NULL in the grid and the Details panel.
- JSON is shown indented and colour-coded in the value viewer and the cell editor, must stay valid to save, and keeps its key order. Details has a View button for JSON values.
- Double-click a column header to fit it to its content.
- Details title and type labels no longer shout in capitals.
- The SQL Server `GO` scan no longer panics on lines that start with Thai or other multi-byte text.
- The workspace divider stays beneath popovers and dialogs.
- Restoring a SQLite/DuckDB file on Windows waits for the old file to be released instead of failing with "Access is denied". (0.4.14 was never published: its Windows tests failed.)

## 0.4.13 — 2026-09-29

- Choosing another connection no longer points an open table tab at it — its SQL could run against the wrong database; a new query tab opens instead. Tabs with unsaved edits can't switch connection.
- A failed "load more rows" no longer retries in a loop.
- SQL Server DECIMAL/NUMERIC values display correctly (e.g. `-327.10`, not `-327.-10`).
- The syntax check follows each tab's own database and stops flagging valid SQL Server, Postgres, MySQL and SQLite syntax (`[dbo].[t]`, `GO`, procedures, `PRAGMA`, …).
- Grid values align by type: numbers right, booleans centred.
- Schema picker at the bottom of the sidebar for databases with several schemas.
- Consistent checkboxes, result tabs and Beautify popover; interface icons moved to Tabler.

## 0.4.12 — 2026-09-29

- Grid editing: type to edit the cursor cell, Set NULL / Set Empty / Duplicate Row (Cmd/Ctrl+D), multi-row edits across a selection, paste over existing cells, and an expanded multi-line editor (Shift+Enter).
- Opening a cell and leaving it no longer rewrites it; empty text stays `''` instead of silently becoming NULL.
- Column constraints are checked while typing (NOT NULL, declared length), and required columns on new rows are reported before saving.
- Optional "Review changes before saving" setting; production connections and CQL always review.
- SQL Server: non-ASCII text is written as `N'…'`, so Thai text is no longer stored as `?`.
- New floating find/replace widget in the SQL editor with match case, whole word, regex, highlighted matches, and replace-all.
- Refreshed the app icon.

## 0.4.11 — 2026-09-24

- Added Open Anything for quickly finding and opening connections, tables, and tabs.
- Refined workspace panel spacing, rounded corners, resize grips, and the SQL workspace divider.
- Improved SQL Server table-structure loading and restored metadata loading for table tabs.
- Adjusted the Blue Studio editor background so it stands apart from surrounding panels.

## 0.4.4 — 2026-08-31

- Added query-result charts with automatic numeric-series detection, configurable line/bar/scatter views, multi-series hover values, and themed SVG export.
- Added a BLOB value viewer with image decoding and richer binary-value handling.
- Added run-all execution with batch result navigation and clearer query-error reporting.
- Refined the SQL editor toolbar, keyboard shortcuts, settings, and focus management.
- Tightened release and website deployment verification around version tags from `main`.

## 0.4.3 — 2026-08-26

- Added independent, resizable split workspaces with per-pane SQL editors, results, filters, autocomplete, ghost suggestions, diagnostics, and query parameters.
- Added VS Code-style drag-and-drop splitting for schema tables and query tabs, including a lightweight drop-zone preview.
- Added typed `{{name}}` query parameters with dialect-aware SQL literal rendering.
- Added multiple SQL editor cursors with Cmd/Ctrl-click and Cmd/Ctrl+D selection workflows.
- Improved workspace persistence and split-tab lifecycle handling so opening, closing, moving, and restoring panes keeps tab indices safe.

## 0.4.2 — 2026-08-25

- Replaced the Lotus Dusk, Tidal Ledger, and Copper Circuit built-ins with Graphite, a TablePlus-style charcoal palette with a vivid blue accent. The three older palettes remain as installable JSON in `examples/themes/`.
- Stopped the first window frame mixing stock egui light/dark colours with the selected plusplus theme when the OS appearance differs.
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
