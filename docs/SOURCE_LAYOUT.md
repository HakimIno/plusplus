# Source layout

Start with the feature's folder, then open its `mod.rs` for the module list.
`lib.rs` keeps the established module paths available, so existing callers and the
public `dbcore` API continue to work after files move.

## UI — `crates/ui/src`

| Folder | Responsibility | Common entry files |
| --- | --- | --- |
| `app/` | Application state, actions and workflow coordination | `actions.rs`, `query.rs`, `connection.rs` |
| `app/panels/` | Panel rendering and interactions | See the map in `app/panels.rs` |
| `editor/` | SQL editing, completion, folding and diagnostics | `editor_tools.rs`, `autocomplete.rs`, `highlight.rs` |
| `results/` | Result display, filtering and cell editing | `grid.rs`, `edit.rs`, `chart.rs`, `value_viewer.rs` |
| `catalog/` | Schema editing and relationship diagrams | `schema.rs`, `erd.rs` |
| `appearance/` | Themes, fonts, icons and shared visual styles | `theme.rs`, `style.rs`, `fonts.rs`, `icons.rs` |
| `components/` | Reusable UI controls | `button.rs`, `input.rs`, `dialog.rs` |
| `platform/` | Window chrome, update delivery and app illustrations | `title_bar.rs`, `update.rs`, `pet.rs` |

`app.rs` owns the application state and `lib.rs` exposes `DbGuiApp` and font setup.
Embedded assets remain under `crates/ui/assets` and `crates/app/assets`.

## Data layer — `crates/core/src`

| Folder | Responsibility | Common entry files |
| --- | --- | --- |
| `connections/` | Database interface, connection setup and SSH tunnels | `database.rs`, `connection.rs`, `tunnel.rs` |
| `backends/` | Database-specific implementations | `postgres.rs`, `mysql.rs`, `sqlite.rs` |
| `model/` | Shared database metadata and SQL construction | `catalog.rs`, `ddl.rs`, `sql.rs` |
| `query/` | Parameter substitution, syntax and safety checks | `parameters.rs`, `syntax.rs`, `safety.rs` |
| `data/` | Values, coercion and staged row edits | `value.rs`, `coerce.rs`, `edits.rs`, `edits/tests.rs` |
| `storage/` | Local settings, secrets and saved activity | `config.rs`, `secrets.rs`, `history.rs`, `favorites.rs` |
| `transfer/` | Clipboard, import and export | `clipboard.rs`, `import.rs`, `export.rs` |

`error.rs` defines shared errors; `erd.rs` defines the relationship-design model.
Integration tests remain in `crates/core/tests`, and benchmarks in `crates/core/benches`.

## Other workspace crates

- `crates/app`: executable entry point, build integration and bundled fonts.

## Adding code

Place a module beside the feature it serves and declare it in that folder's
`mod.rs`. Keep reusable widgets in `components`, app coordination in `app`, and
backend-independent data operations in `core`. Add a root re-export only when
needed by the existing module facade or public API. Tests stay beside their
implementation, except end-to-end backend workflows in `crates/core/tests`.
