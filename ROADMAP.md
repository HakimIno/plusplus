# Roadmap

plusplus is working toward a trustworthy 1.0 database client. The order below reflects risk
and user value, not a promise of dates. GitHub issues are the source of truth for individual
work items.

## Now — trustworthy beta

- Add live integration coverage for PostgreSQL, MySQL/MariaDB, and SQL Server.
- Publish a reproducible performance note for startup, memory use, large grids, and exports ([#2](https://github.com/HakimIno/plusplus/issues/2)).
- Complete Apple code signing and notarization ([#3](https://github.com/HakimIno/plusplus/issues/3)).
- Complete Windows Authenticode signing ([#4](https://github.com/HakimIno/plusplus/issues/4)).
- Improve onboarding and publish a short end-to-end product demo ([#1](https://github.com/HakimIno/plusplus/issues/1)).
- Stabilize the connection → query → inspect → edit workflow across all supported databases.

## Next — workflow depth

- Finish and enable the ER diagram after interaction and layout quality meet the release bar ([#5](https://github.com/HakimIno/plusplus/issues/5)).
- Improve explain-plan inspection and database-specific query diagnostics.
- Expand keyboard navigation and accessibility coverage ([#6](https://github.com/HakimIno/plusplus/issues/6)).
- Add safe backup/restore hand-offs where the native database tools are available.
- Package for additional Linux distribution channels.

## Later — extensibility

- Result analysis and chart-ready summaries.
- Reusable snippets and configurable keybindings.
- A documented extension model after the core APIs are stable.

## Explicit non-goals for 1.0

- A hosted database proxy or cloud account requirement.
- Telemetry, advertising, or uploading queries/results to a third party.
- Replacing database-native administration and backup tools.
- Shipping unfinished features solely to match a competitor checklist.

Want to help? Open a focused proposal or choose an issue labeled
[`help wanted`](https://github.com/HakimIno/plusplus/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22).
