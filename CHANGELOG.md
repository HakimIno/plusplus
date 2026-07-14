# Changelog

Notable user-visible changes are documented here. The project follows semantic versioning
while pre-1.0 releases may still change workflows and configuration formats.

## Unreleased

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
