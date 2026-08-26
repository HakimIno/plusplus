# Competitive Gap Analysis: PlusPlus vs DbGate and TablePlus

Last reviewed: 2026-08-26

This document records the current product gaps between PlusPlus, DbGate, and
TablePlus. It is a point-in-time assessment based on the PlusPlus codebase and
the competitors' official product documentation. Competitor features and the
PlusPlus implementation will change, so this document should be reviewed when
planning a major release.

DbGate's website presents both Community and Premium features. A feature listed
for DbGate below is not necessarily available in its free edition.

## Executive summary

PlusPlus already covers most of the core daily database workflow: browsing
schemas, writing and running queries, paging and filtering data, staging edits,
editing database objects, importing and exporting data, and working with ER
designs. Its main gap is no longer the basic data grid or visual appearance.

The largest differences from DbGate and TablePlus are now:

1. Product maturity and consistent behavior across every supported backend.
2. Fast keyboard-first navigation across connections and database objects.
3. User-facing query diagnostics and database operational tools.
4. Schema comparison, migration generation, and deployment workflows.
5. Rich result analysis, charts, dashboards, and special-value viewers.
6. Broader import/export, database-driver, and extension ecosystems.

The recommended positioning is not to copy every competitor feature. PlusPlus
should first become a safer, large-data-friendly alternative to TablePlus while
preserving its local-first, native Rust architecture.

## Current comparison

| Area | PlusPlus today | Important gap |
| --- | --- | --- |
| Data grid, filters, and pagination | Virtualized grid, multi-condition filtering, server-side filtering for supported table/view reads, row counts, keyset/offset pagination, and bounded memory | Validate performance and correctness with multi-million-row data and every supported dialect |
| Data editing | Inline and details editing, staged insert/update/delete, undo/redo, SQL preview, save/discard workflow | Rich editors and previewers for JSON, BLOB, binary, images, arrays, and long text |
| SQL editor | Syntax highlighting, completion, formatting, folding, suggestions, favorites, history, cancellation, and restored tabs | Query parameters, multiple carets, split editors, and more configurable editor behavior |
| Navigation | Searchable schema, favorites, history, bookmarks, and multiple tabs | A global **Open Anything** or command palette for objects, connections, tabs, and actions |
| Query diagnostics | Production Guardian can perform bounded count and EXPLAIN preflight checks | A user-facing EXPLAIN/ANALYZE viewer, plan visualization, profiling, and process/session monitor |
| Schema management | Editors for tables, views, routines, and triggers; ER designer; dialect-specific forward-engineering preview | Compare two live schemas, generate a migration diff, review it, and safely deploy it |
| Import/export | CSV and JSON import; complete streaming CSV/JSON export | SQL dump, Excel, saved/re-runnable transfer jobs, URL sources, scripting, and richer format options |
| Analysis and visualization | A Chart result tab and an analysis crate are wired into the application | The Chart view and analysis APIs are placeholders; no aggregation UI, dashboard, or map view yet |
| Database operations | Production safeguards, TLS/SSH, local secrets, query cancellation | Backup/restore hand-off, users/roles, server processes, maintenance, and administrative views |
| Database coverage | PostgreSQL, MySQL, MariaDB, SQL Server, SQLite, DuckDB, Cassandra, and ScyllaDB are represented by the current backend model | Oracle, Redis, MongoDB, ClickHouse, Snowflake, Redshift, CockroachDB, Firebird, and other competitor-supported engines |
| Extensibility | Drop-in themes and imported fonts | Stable plugin API, driver SDK, contribution points, and plugin discovery |
| Collaboration/deployment | Native local-first desktop application with no required account or telemetry | DbGate-style web/self-hosted deployment, shared scripts/connections, and team workspaces; these may remain deliberate non-goals |

## Recommended implementation order

### 1. Open Anything and command palette

Add a single keyboard-driven search surface, such as `Cmd/Ctrl+P`, covering:

- connections and databases;
- schemas, tables, views, functions, procedures, and triggers;
- open and recently used tabs;
- saved queries and history;
- application actions and settings.

This has high daily UX value without requiring a new database subsystem.

### 2. Query parameters, multiple carets, and split editors

Treat these as one SQL-editor workflow milestone, but ship them in three small,
independently testable layers.

**Query parameters**

- detect named placeholders in executable SQL while ignoring strings, quoted
  identifiers, and comments;
- show a compact parameter panel with one row per unique name, inferred or
  explicitly selected types, validation, and a clear missing-value state;
- bind values through each driver's parameter API where the backend supports it
  instead of interpolating raw text;
- provide an exact execution preview or a dialect-aware escaped-literal fallback
  for statements that cannot use bound values;
- keep parameter names and non-sensitive defaults with a saved query, but do not
  persist secret values unless the user explicitly opts in to keychain storage;
- apply the same values to Run, Production Guardian, history metadata, and saved
  queries so the reviewed statement and the executed statement cannot diverge.

Start with named scalar values for a whole query tab. Positional parameters,
lists, environment profiles, and reusable parameterized snippets can follow
after binding and history redaction are reliable across all supported backends.

**Multiple carets**

- support Cmd/Ctrl-click to add or remove a caret and Cmd/Ctrl+D to select the
  next occurrence;
- apply typing, paste, delete, indent, and line-comment commands to every caret
  as one undoable edit;
- keep edits deterministic by applying non-overlapping ranges from the end of
  the document toward the beginning and coalescing overlapping selections;
- let one primary caret own autocomplete, ghost text, diagnostics, and the
  visible anchor while secondary carets remain lightweight;
- verify behavior around Unicode text, folded regions, undo/redo, and SQL
  formatting before adding more selection commands.

**Split editors**

- let a query tab split horizontally or vertically into two independent SQL
  buffers, initialized from the current query so the panes can be compared;
- make the focused pane the target for autocomplete, editor commands, and Run;
- provide a visible split/close control, a draggable divider, and keyboard focus
  movement between panes;
- persist the split direction and divider ratio with the workspace, but collapse
  safely to one pane when a tab is restored in a window that is too small;
- keep edits, undo stacks, parameter values, and history entries independent per
  pane while the split is open.

Suggested delivery order is query parameters first, multiple carets second, and
split editors third. Parameters add the most database-specific risk and user
value; the latter two should share a deliberate multi-view editor state model
rather than layering more state onto a single text widget.

### 3. EXPLAIN/ANALYZE viewer

Promote the existing internal EXPLAIN capability into a user-facing tool:

- raw and structured plan views;
- estimated versus actual rows;
- cost, duration, loops, scan type, and memory where available;
- warnings for sequential scans, estimate errors, expensive sorts, and missing indexes;
- dialect-specific parsing with a raw fallback when a plan cannot be normalized;
- explicit confirmation before using an ANALYZE mode that executes a statement.

### 4. JSON, BLOB, image, and long-value Quick Look

Add a focused value inspector with:

- formatted and tree-based JSON;
- raw text and hex modes;
- image preview for recognized binary formats;
- copy/save actions;
- size limits and lazy loading so large values do not freeze the UI.

The current core value model deliberately renders binary data as a placeholder,
so this should be implemented as an explicit safe viewer rather than silently
decoding arbitrary bytes.

### 5. Schema compare and migration workflow

Build on the existing schema introspection, DDL builders, and ER design model:

1. Select source and target schemas or connections.
2. Normalize objects into a comparable model.
3. Show added, removed, and changed objects.
4. Generate dialect-specific migration SQL.
5. Mark destructive or lossy steps clearly.
6. Preview, copy, save, or apply through Production Guardian.

The first version should support tables, columns, primary keys, foreign keys,
and indexes. Views, routines, and triggers can follow after the core diff model
is reliable.

### 6. Safe backup and restore hand-off

Do not reimplement database dump formats. Detect and invoke native tools such as
`pg_dump`, `pg_restore`, `mysqldump`, or their supported equivalents, while
PlusPlus owns:

- target and option selection;
- sanitized command preview;
- progress and cancellable background execution;
- logs and clear error reporting;
- production confirmation and credential-safe process invocation.

This is consistent with the existing roadmap's intent to provide safe native-tool
hand-offs rather than replace database-native administration tools.

### 7. Charts and result analysis

Finish the existing Chart tab incrementally:

1. Detect numeric, category, and time columns.
2. Offer table summary and descriptive statistics.
3. Add line, bar, area, scatter, and pie charts only where the result shape is valid.
4. Allow explicit X, Y, grouping, and aggregation selection.
5. Save chart configuration with a query tab or favorite.
6. Consider dashboards and scheduled refresh only after a single-query chart is stable.

### 8. Pre-1.0 reliability

Before expanding the driver list substantially:

- add live integration coverage for every primary backend;
- test filters, counts, pagination, edits, transactions, cancellation, and exports against large real datasets;
- document a backend capability matrix;
- verify crash recovery and workspace restoration;
- complete platform signing and packaging;
- ensure unsupported operations are disabled with a reason rather than failing after execution.

This work is less visible than a new feature, but it is the main difference
between a promising client and one users trust with production databases.

## Lower-priority opportunities

These are useful differentiators but should not displace the eight priorities
above:

- configurable keybindings and reusable parameterized snippets;
- connection import/export and encrypted sharing;
- SQL or visual query builder;
- database administration for users and roles;
- plugin/driver SDK after core APIs stabilize;
- optional AI-assisted SQL with explicit schema-sharing consent;
- web/self-hosted mode and cloud collaboration;
- additional drivers selected from actual user demand.

## Features not worth chasing immediately

### Every database driver

Broad driver coverage looks good on a checklist but creates permanent work in
introspection, types, TLS, editing, pagination, DDL, testing, and support. Add a
driver only when the target audience needs it and its core workflow can be
supported properly.

### AI assistant

AI-generated SQL does not compensate for weak diagnostics, migrations, or
reliability. It also conflicts with local-first expectations unless users have
clear control over which schema and query data leave the machine.

### Cloud collaboration and web deployment

DbGate benefits from these capabilities, but they introduce accounts,
authentication, secret management, multi-user authorization, synchronization,
and hosting concerns. PlusPlus can intentionally remain local-first until there
is clear demand for a separate team product.

## Existing PlusPlus strengths to preserve

- Production profiles and Production Guardian.
- Application- and session-level read-only protection where supported.
- Staged edits with preview, save, discard, undo, and redo.
- Virtualized results, bounded memory, and keyset pagination where safe.
- Complete streaming exports rather than exporting only the visible page.
- Native Rust desktop architecture without Electron.
- TLS, mutual TLS where supported, and verified SSH tunnels.
- OS-keychain credential storage, local query history, and no required telemetry.
- Portable ER designs with dialect-specific DDL preview.

These features provide a credible product identity: a fast local database client
that makes production mistakes harder.

## Sources

### PlusPlus

- [`README.md`](../README.md)
- [`ROADMAP.md`](../ROADMAP.md)
- [`crates/core/src/database.rs`](../crates/core/src/database.rs)
- [`crates/core/src/model.rs`](../crates/core/src/model.rs)
- [`crates/analysis/src/lib.rs`](../crates/analysis/src/lib.rs)

### TablePlus

- [TablePlus overview and supported databases](https://docs.tableplus.com/)
- [TablePlus product features](https://tableplus.com/)
- [Getting Started: Open Anything, editing, filters, and query editor](https://docs.tableplus.com/getting-started)
- [Import and Export](https://docs.tableplus.com/gui-tools/import-and-export)
- [Backup and Restore](https://docs.tableplus.com/gui-tools/backup-and-restore)
- [Metrics Board](https://docs.tableplus.com/gui-tools/metrics-board)
- [Working with rows and Quick Look](https://docs.tableplus.com/gui-tools/working-with-table/row)

### DbGate

- [DbGate Community and Premium feature overview](https://dbgate.org/)
- [DbGate screenshots and workflow examples](https://dbgate.org/screenshots/)
- [DbGate 5.5 import/export and ClickHouse release notes](https://dbgate.org/blog/2024-09-03-5.5.0/)
- [DbGate AI Assistant](https://dbgate.org/features/premium-ai-assistant/)
