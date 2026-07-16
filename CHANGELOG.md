# Changelog

Notable user-visible changes are documented here. The project follows semantic versioning
while pre-1.0 releases may still change workflows and configuration formats.

## Unreleased

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
