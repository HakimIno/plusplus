# Private beta protocol

Run this protocol against a release candidate downloaded from GitHub Releases, not a local build.
Keep one anonymized row per participant in the issue tracker or a private spreadsheet; never ask
for database credentials, SQL containing customer data, or audit logs.

## Cohort

- Recruit 10–20 people who did not contribute to plusplus.
- Include at least one macOS, Windows, and Linux user where those release assets are available.
- Include SQLite plus at least one supported server database. All server work must use a
  non-production database and a least-privilege account.

## Script and record

For each participant, record pass/fail, elapsed time, assistance required, and the first
confusing or untrustworthy moment for each task:

1. Describe the product after viewing the repository for 30 seconds.
2. Download, verify, and install the release asset without help.
3. Open `examples/sample.sqlite`, change one value, save it, and confirm the change.
4. Connect to a non-production server, run a query, and inspect a table.
5. Find and use one safety control: read-only mode or Production Guardian.

Classify findings as P0 (data loss, credential exposure, or arbitrary execution), P1 (blocks a
core task), P2 (misleading or repeatedly confusing), or P3 (minor). Link each non-P3 finding to
a reproducible issue; deduplicate the same failure across participants.

## Exit criteria for 1.0

- At least 10 participants complete the script.
- At least 80% install and open the app without assistance.
- At least 90% complete the SQLite edit and server query tasks without assistance.
- No unresolved P0 or P1 finding remains.
- Every P2 observed by at least three participants is fixed, or has a documented product decision
  and workaround in the release notes.
- All release assets used in the beta pass their platform-signing and Minisign verification.

Publish an aggregate summary only: participant count, platforms, database kinds, task completion
rates, and the resolved issues. Do not publish participant identities or connection details.
