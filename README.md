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
keychain — never to disk in plaintext. Server connections have a per-connection SSL mode
(disable / prefer / require / verify-ca / verify-full) with an optional custom CA
certificate, falling back to the system trust store, plus a client certificate and key
for mutual TLS (PostgreSQL and MySQL/MariaDB). Live connections are pooled and shared,
so several tabs can work against the same database at once.

A connection can be marked **production**: destructive statements (`UPDATE`, `DELETE`,
`DROP`, `TRUNCATE`, `ALTER`) are then held in a confirmation dialog before they run —
with an extra callout when an `UPDATE`/`DELETE` has no `WHERE` clause at all.

Server connections can also ride an **SSH tunnel**: the app authenticates to a bastion
host (password or private key, with the secret in the OS keychain), forwards a loopback
port through it, and the database driver connects through that — no exposed DB port
needed. One bastion session multiplexes all of a connection's pooled channels, and it
tears down with the connection.

**Query history.** Every executed statement — queries, staged-edit commits, DDL — is
appended to a local audit log (`history.jsonl` in the config dir) with its connection,
time, duration, and outcome. A title-bar button toggles a right-hand history panel that
updates live as queries run; entries can be copied or sent back into the editor, and the
whole log can be cleared. Recording can be switched off in Settings, since SQL text can
contain data values.

**Schema browsing.** Connecting introspects the whole database into a sidebar tree:
tables, columns (type, nullability, primary key), indexes, and foreign keys
(`col → table(col)`, with the referential actions in the tooltip), filterable by name. A
single click previews a table's rows; a double click opens it as a permanent tab.

**Query tabs.** Each tab is an independent SQL editor (with syntax highlighting) bound to
its own connection, with its own result, sort, filter, and edit state. Table previews
reuse one italic *preview* tab so casual browsing doesn't pile up tabs. The open tabs —
their SQL, connection, and source table — persist across restarts.

**The grid.** Results render in a virtualized grid that stays smooth past 100k rows:
resizable columns, click-to-sort headers, a TablePlus-style filter bar
(column / operator / value conditions), and a details panel showing the selected row
field by field.

**Big tables.** Million-row tables are browsed server-side, one page at a time. Table
tabs get a pager in the status bar (first/prev/next/last plus a page size) that rewrites
the query's `LIMIT/OFFSET` — or `TOP` / `OFFSET … FETCH` on SQL Server — in place, so the
SQL editor always shows exactly what ran, and a background `COUNT(*)` supplies the
"1–1,000 of 1,234,567" total. Hand-written WHERE / ORDER BY clauses survive page flips.
As a safety net, every query streams rows off the wire and stops materializing at 100k
rows (the result is marked as capped in the status line), so an accidental
`SELECT * FROM huge` can't exhaust memory on any backend.

**Editing.** When a result maps cleanly back to one table (any simple
`SELECT * FROM t …`), cells become editable in place with type-aware editors. You can also
add and remove whole rows: double-click the trailing **＋** strip to add a new row (tinted
green), and select a row and press **Backspace/Delete** to mark it for deletion (tinted
red). Edits are *staged* — not yet written — until **Cmd/Ctrl+S** turns them into primary-key
`UPDATE`s, `INSERT`s (rejected if a new row's primary key is missing or duplicates an
existing one), and `DELETE`s, then reloads the grid with what the database actually stored.
Anything that can't be mapped back safely (joins, projections, aggregates) is read-only.

**Data / Structure views.** A table tab can switch between its rows (*Data*) and the
table's definition (*Structure*): columns with types, nullability, and keys, plus its
indexes and foreign keys (with their ON DELETE / ON UPDATE actions) — straight from the
introspected schema, no extra queries.

**Themes.** Three built-in color themes (Carbon, Midnight, Daylight), persisted across
runs.

Everything important has a shortcut: **Cmd/Ctrl+Enter** runs, **Cmd/Ctrl+S** saves staged
edits, **Cmd/Ctrl+R** reloads the result (dropping unsaved edits), **Esc** discards unsaved
edits, **Backspace/Delete** marks the selected row for deletion, **Cmd/Ctrl+T / W** opens
and closes tabs, **Cmd/Ctrl+F** toggles the filter bar.

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
cargo run --bin plusplus    # build & launch the GUI (dev)
cargo test --workspace      # data-layer and headless UI tests
```

A small sample SQLite database with mixed Thai/English data ships at
`examples/sample.sqlite` — add it as a SQLite connection to try the app without a server.

## Versioning

The release version is defined once in the root `Cargo.toml`:

```toml
[workspace.package]
version = "0.1.0"
```

Every release gets a matching **git tag** `vX.Y.Z` (e.g. `v0.1.0`) and a **DMG**
`target/dist/plusplus-X.Y.Z.dmg`. Bump the version in `Cargo.toml`, then run
`scripts/release.sh --tag` to build, package, and create the tag.

### In-app updates (macOS)

Installed copies check [GitHub Releases](https://github.com/HakimIno/plusplus/releases)
on launch. When a newer `plusplus-X.Y.Z.dmg` is published, a pill button appears on the
query tab bar (**Update vX.Y.Z**). The app downloads the DMG, replaces
`/Applications/plusplus.app`, and relaunches — no manual reinstall.

Publish an update:

```bash
# bump version in Cargo.toml, commit, then:
git push origin v0.2.0   # triggers .github/workflows/release.yml
```

Or build locally and attach the DMG to a GitHub Release manually. The release must include
an asset named `plusplus-<version>.dmg` (produced by `scripts/release.sh`).

## macOS release (build, install, remove)

### Quick dev run

```bash
cargo run --bin plusplus
```

### Release build + `.app` + `.dmg`

```bash
# Host architecture only (fastest)
scripts/release.sh

# Universal binary (Intel + Apple Silicon) — slower, best for distribution
scripts/release.sh --universal

# Build + package + create git tag vX.Y.Z
scripts/release.sh --tag

# Build + package + replace the installed copy in /Applications
scripts/release.sh --install
```

Outputs:

| Artifact | Path |
|---|---|
| App bundle | `target/dist/plusplus.app` |
| Installer DMG | `target/dist/plusplus-<version>.dmg` |

### Install / replace / remove

The install script **removes the old app first**, then copies the new build:

```bash
packaging/macos/install.sh      # replace /Applications/plusplus.app
packaging/macos/uninstall.sh    # remove /Applications/plusplus.app
```

Or manually:

```bash
rm -rf /Applications/plusplus.app
cp -R target/dist/plusplus.app /Applications/
open -a plusplus
```

### Tag workflow (recommended)

```bash
# 1. Bump version in Cargo.toml (e.g. 0.1.0 → 0.2.0)
# 2. Commit the bump
git add Cargo.toml Cargo.lock
git commit -m "Bump version to 0.2.0"

# 3. Build, package, and tag
scripts/release.sh --universal --tag

# 4. Push the tag (when ready to publish)
git push origin v0.2.0
```

Low-level steps (without the release script):

```bash
cargo build --release --bin plusplus --target x86_64-apple-darwin
cargo build --release --bin plusplus --target aarch64-apple-darwin
packaging/macos/make-dmg.sh
packaging/macos/install.sh
```
