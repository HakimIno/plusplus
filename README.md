<p align="center">
  <img src="crates/app/assets/icon/png/icon-256.png" alt="plusplus logo" width="120">
</p>

<p align="center"><strong>plusplus</strong> is a fast, native database GUI in the spirit of TablePlus — written in Rust.
<br />One window for everything: browse your schema, run SQL, and edit results — without ever waiting on the network.</p>

<p align="center"><sub>PostgreSQL · MySQL / MariaDB · SQL Server · SQLite</sub></p>

<p align="center">Grab a build from <a href="https://github.com/HakimIno/plusplus/releases">GitHub Releases</a>, or build from source — see <a href="#try-it">Try it</a> below.</p>

---

## Why it feels fast

plusplus is a single native binary — no Electron, no web view, no runtime to boot.
It opens instantly and stays smooth no matter how large the data gets.

- **The UI never blocks.** Every query, count, and export runs off the main thread.
  You can keep scrolling, typing, and switching tabs while a million rows stream in.
- **Grids that don't choke.** Results render in a virtualized grid that stays smooth
  past 100k rows — resize columns, sort, and filter without a hitch.
- **Million-row tables, one page at a time.** Large tables are paged server-side, so
  you browse `1–1,000 of 1,234,567` without dragging the whole table over the wire.
- **Memory-safe by design.** Queries stream off the wire and stop at a safe cap, so an
  accidental `SELECT * FROM huge` can't take the app down.
- **One interface, four databases.** Postgres, MySQL/MariaDB, SQL Server, and SQLite
  all behave identically — same grid, same editing, same shortcuts.

---

## What you can do

**Browse any database instantly.**
Connect and the whole schema appears in a filterable sidebar — tables, columns,
primary keys, indexes, and foreign keys. Single-click to preview rows, double-click
to open a tab.

**See how everything connects.**
One click opens your database as a live ER diagram: every table a box, every foreign
key a curve, auto-arranged and fully pannable and zoomable. It tracks schema changes
as they happen.

**Edit data right in the grid.**
When a result maps back to a single table, cells become editable in place. Add rows,
delete rows, tweak values — everything is staged and shown in colour until you hit
save, so nothing touches the database until you mean it.

**Export whole tables in seconds.**
Right-click any table, then Export Table, then CSV or JSON. The export streams
server-side, straight to disk, with no row cap — so even multi-million-row tables
export whole while the app stays responsive.

---

## More that's built in

- **Safe on production.** Mark a connection *production* and destructive statements
  (`UPDATE`, `DELETE`, `DROP`, `TRUNCATE`, `ALTER`) pause for confirmation — with a
  clear warning when an `UPDATE` or `DELETE` has no `WHERE`.
- **Connect through anything.** Per-connection SSL (up to verify-full and mutual TLS)
  and optional SSH tunnels through a bastion host. Passwords and keys live in the OS
  keychain, never on disk in plaintext.
- **Query history.** Every statement is logged with its connection, time, duration,
  and outcome — replay or copy any of it from a live side panel.
- **Independent query tabs.** Each tab is its own editor with syntax highlighting, its
  own connection, result, sort, and filter — and they all persist across restarts.
- **Thai-friendly.** A Thai-capable font is embedded everywhere; everything is UTF-8
  end to end.
- **Themes.** Carbon, Midnight, and Daylight built in and remembered across runs —
  plus custom themes: drop a `*.json` palette into the themes folder and pick it in
  Settings. See [docs/THEMES.md](docs/THEMES.md).

### Keyboard-first

| Shortcut | Action |
|---|---|
| `Cmd/Ctrl + Enter` | Run query |
| `Cmd/Ctrl + S` | Save staged edits |
| `Cmd/Ctrl + R` | Reload result |
| `Esc` | Discard unsaved edits |
| `Backspace / Delete` | Mark row for deletion |
| `Cmd/Ctrl + T / W` | Open / close tab |
| `Cmd/Ctrl + F` | Toggle filter bar |

---

## Try it

SQLite is bundled, so there's nothing else to install.

```bash
cargo run --bin plusplus
```

A sample database ships at `examples/sample.sqlite` — add it as a SQLite connection to
explore a small Thai e-commerce shop (linked tables, foreign keys in every flavour,
real order history) and see the schema browser, grid, and ER diagram with real data.

## Install on macOS

```bash
scripts/release.sh --install     # build, package, and replace /Applications/plusplus.app
```

Installed copies check [GitHub Releases](https://github.com/HakimIno/plusplus/releases)
on launch; when a newer build is published, an **Update** button appears in the app and
updates in place — no manual reinstall.

## Run on Linux

Ubuntu, Debian, Fedora, Arch, and openSUSE can use the helper script (CI smoke-tests
this path on Ubuntu, Debian, and Fedora):

```bash
scripts/linux-build.sh --install-deps --install-rust --release --smoke
scripts/linux-build.sh --release --run
```

---

<div align="center">
<sub>Built with Rust · <a href="https://github.com/HakimIno/plusplus">github.com/HakimIno/plusplus</a></sub>
</div>
