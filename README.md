# plusplus

A native, cross-platform database GUI written entirely in Rust — TablePlus in spirit, with
a fast, virtualized data grid at its core. Browse schemas, run SQL, and (from Phase 2)
analyze result sets with descriptive statistics, grouping, and charts.

First-class Thai support: a Thai-capable font (Noto Sans Thai) is embedded and used as a
fallback everywhere, and all cell/column content is UTF-8 with no truncation.

## Status

**Phase 1 (this build) is complete and runnable:**

- Connection manager: add / edit / save / delete connections. Non-secret fields persist to a
  JSON config; **passwords are stored in the OS keychain, never in plaintext**.
- Connect to **PostgreSQL** and **SQLite**.
- Left panel schema browser: database → tables → columns (with PK / nullability / type) and
  indexes. Filter tables by name.
- SQL editor with a **Run** button and **Cmd/Ctrl+Enter** shortcut.
- Virtualized results grid (`egui_extras::TableBuilder`) that stays smooth at 100k+ rows:
  resizable columns, click-to-sort headers, horizontal + vertical scroll.
- Row count and query time in the status bar; SQL errors shown cleanly without crashing.
- Non-blocking I/O: queries run on a `tokio` runtime and stream back over a channel, so the
  UI stays interactive (spinner while busy).

## Architecture

A Cargo workspace keeps the data/analysis logic decoupled from the GUI and unit-testable
without a window:

```
plusplus/
  Cargo.toml            # workspace + pinned dependency versions
  crates/
    core/               # DB abstraction: connections, schema introspection, query execution
    analysis/           # (Phase 2) Polars: result set -> DataFrame -> stats/aggregations
    ui/                 # egui views, widgets, app state
    app/                # eframe entry point; embeds the Thai font, wires it all together
```

Key design points:

- **`Database` trait** (`core::Database`) abstracts over backends: `kind`, `introspect`,
  `execute`. Implemented for Postgres and SQLite. The rest of the app only ever holds an
  `Arc<dyn Database>`, so adding MySQL/MSSQL later means one new module + one match arm in
  `core::connect` — no UI changes.
- **Backend-agnostic rows**: every backend decodes its native types into a common
  `core::Value` enum (`Null`/`Bool`/`Int`/`Float`/`Text`/`Bytes`), so the UI and analysis
  layers never depend on a specific driver.
- **No blocking on the UI thread**: `update` collects deferred `Action`s from panel closures
  and applies them with full `&mut self`, sidestepping egui borrow conflicts; database work
  is spawned on tokio and returned via `std::sync::mpsc`.
- The internal crate is imported as **`dbcore`** (not `core`) so it doesn't shadow Rust's
  std `core` crate in dependents.

## Build & Run

Requires a recent stable Rust (≥ 1.94; pinned via `rust-toolchain.toml`). SQLite is bundled
(no system library needed).

```bash
# build everything
cargo build

# run the GUI
cargo run -p plusplus-app      # or: cargo run --bin plusplus

# run the (GUI-free) data-layer tests
cargo test -p plusplus-core
```

### Try it with the sample database

A small SQLite database with mixed Thai/English data lives at `examples/sample.sqlite`
(`customers`, `orders`). In the app:

1. Click **➕** next to *Connections*.
2. Set **Type** = SQLite, **Browse…** to `examples/sample.sqlite`, give it a name, **Save**.
3. Click the connection to connect — the schema tree populates on the left.
4. Double-click a table to preview its rows, or run e.g.
   `SELECT city, COUNT(*) FROM customers GROUP BY city;`
5. Click a column header to sort. Thai text renders in headers, cells, and the editor.

### Connecting to PostgreSQL

Use the connection dialog (host / port / user / password / database). The password is saved
to your OS keychain under the service `plusplus`.

## Tech stack

GUI `eframe`/`egui` + `egui_extras` · async `tokio` · DB `sqlx` (postgres + sqlite) ·
secrets `keyring` · file dialogs `rfd` · clipboard `arboard` · errors `thiserror`/`anyhow`.
Versions are pinned in the root `Cargo.toml`. (`polars` and `egui_plot` arrive in Phase 2.)

## What Phase 2 will add

- Load the current result set into a Polars `DataFrame`.
- A **Stats** tab: per-column descriptive statistics (count, nulls, min/max/mean/median,
  distinct count) with correct numeric/text/date handling.
- A **group-by / aggregate** builder (count/sum/avg/min/max).
- **Charts** via `egui_plot`: bar, line, and a histogram for a chosen numeric column.

Phase 3 then brings multiple query tabs, persisted query history, CSV/JSON export, in-cell
editing with previewed `UPDATE`s (opt-in commit), and grid keyboard navigation + copy.
