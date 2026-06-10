# plusplus

A native database GUI written entirely in Rust, in the spirit of TablePlus. One window
for everything: browse a database's schema, run SQL, inspect and edit the results — fast,
keyboard-friendly, and without ever blocking the UI on the network.

It speaks **PostgreSQL**, **MySQL / MariaDB**, **SQL Server**, and **SQLite** through one
backend abstraction, so every feature works the same way against any of them.

Thai text is a first-class citizen: a Thai-capable font (Noto Sans Thai) is embedded as a
fallback everywhere, and all content is UTF-8 end to end.

## What it does

**Connections.** Saved connections live in a JSON config, while passwords go to the OS
keychain — never to disk in plaintext. Live connections are pooled and shared, so several
tabs can work against the same database at once.

**Schema browsing.** Connecting introspects the whole database into a sidebar tree:
tables, columns (type, nullability, primary key), and indexes, filterable by name. A
single click previews a table's rows; a double click opens it as a permanent tab.

**Query tabs.** Each tab is an independent SQL editor (with syntax highlighting) bound to
its own connection, with its own result, sort, filter, and edit state. Table previews
reuse one italic *preview* tab so casual browsing doesn't pile up tabs. The open tabs —
their SQL, connection, and source table — persist across restarts.

**The grid.** Results render in a virtualized grid that stays smooth past 100k rows:
resizable columns, click-to-sort headers, a TablePlus-style filter bar
(column / operator / value conditions), and a details panel showing the selected row
field by field.

**Editing.** When a result maps cleanly back to one table (any simple
`SELECT * FROM t …`), cells become editable in place with type-aware editors. Edits are
*staged* — tinted green, not yet written — until **Cmd/Ctrl+S** turns them into primary-key
`UPDATE`s and reloads the grid with what the database actually stored. Anything that
can't be mapped back safely (joins, projections, aggregates) is simply read-only.

**Data / Structure views.** A table tab can switch between its rows (*Data*) and the
table's definition (*Structure*): columns with types, nullability, and keys, plus its
indexes — straight from the introspected schema, no extra queries.

**Themes.** Three built-in color themes (Carbon, Midnight, Daylight), persisted across
runs.

Everything important has a shortcut: **Cmd/Ctrl+Enter** runs, **Cmd/Ctrl+S** saves staged
edits, **Cmd/Ctrl+T / W** opens and closes tabs, **Cmd/Ctrl+F** toggles the filter bar.

## How it's built

A Cargo workspace separates the data layer from the GUI, so the interesting logic is
unit-testable without opening a window:

```
crates/
  core/      # backend abstraction: connections, introspection, query execution
  analysis/  # placeholder for Polars-based result analysis (stats, group-by, charts)
  ui/        # egui views, widgets, app state
  app/       # eframe entry point; embeds the Thai font, wires it all together
```

The design rests on a few ideas:

- **One trait, many backends.** `core::Database` (`kind` / `introspect` / `execute`) is
  all the app knows about a database; each backend decodes its native types into a common
  `Value` enum. Adding a backend means one module plus one match arm — the UI doesn't
  change.
- **The UI thread never waits.** Database work runs on a `tokio` runtime and reports back
  over a channel that the UI drains each frame; a tab id routes every result to the tab
  that asked for it, even if the user has moved on.
- **Editing is safety-first.** Editability is *derived from the SQL itself* on every run,
  values are validated against their column types before any statement is built, and
  generated `UPDATE`s are keyed strictly by primary key.
- The internal crate is imported as `dbcore` (not `core`) so it doesn't shadow Rust's
  std `core`.

Built on `eframe`/`egui` + `egui_extras` for the GUI, `sqlx` and `tiberius` for the
databases, `tokio` for async, and `keyring` for secrets. Versions are pinned in the root
`Cargo.toml`.

## Running it

Requires stable Rust (pinned via `rust-toolchain.toml`); SQLite is bundled, so there's
nothing else to install.

```bash
cargo run --bin plusplus    # build & launch the GUI
cargo test --workspace      # data-layer and headless UI tests
```

A small sample SQLite database with mixed Thai/English data ships at
`examples/sample.sqlite` — add it as a SQLite connection to try the app without a server.

On macOS, `packaging/macos/make-dmg.sh` packages a release build into `plusplus.app` and
a styled drag-to-install `.dmg`. Build both targets first and it produces a universal
(Intel + Apple Silicon) app:

```bash
cargo build --release --bin plusplus --target x86_64-apple-darwin
cargo build --release --bin plusplus --target aarch64-apple-darwin
packaging/macos/make-dmg.sh
```
