# Launch checklist

GitHub Trending cannot be enabled through a repository setting. A launch works when a useful,
credible project receives genuine attention in a short window. Use this checklist only after
the product and installation path are ready.

## Product proof

- [ ] Replace the test-snapshot README hero with `docs/screenshots/hero.png`.
- [ ] Add a 15–25 second demo GIF showing browse → query → stage edit → save.
- [ ] Publish reproducible benchmarks and link every performance claim to them.
- [ ] Verify the sample SQLite workflow on a clean machine.
- [ ] Test each release asset after downloading it from GitHub, not from the build directory.
- [ ] Make every README feature claim match the shipped release.

## Trust and installation

- [ ] All required GitHub Actions checks pass on the release commit.
- [ ] GitHub detects the MIT/Apache-2.0 license after the license files are pushed.
- [ ] macOS Gatekeeper and notarization checks pass, or the unsigned limitation remains prominent.
- [ ] Windows Authenticode verification passes, or the unsigned limitation remains prominent.
- [ ] Minisign verification passes for every release package.
- [ ] Private vulnerability reporting is enabled.

## GitHub storefront

- [ ] Description, topics, homepage, release, and roadmap are current.
- [ ] Upload `.github/readme-banner.jpg` as the repository social preview in
      **Settings → General → Social preview**.
- [ ] Seed the issue tracker with real, bounded work and keep stale issues triaged.
- [ ] Mark approachable work with `good first issue` and explain how to verify it.
- [ ] Pin the launch release and the best technical discussion where GitHub supports it.

## Private beta before public launch

Give the release to 10–20 database users who are not project contributors. Ask each person to:

1. Explain what the product does after looking at the repository for 30 seconds.
2. Install it without help.
3. Open the bundled SQLite sample and complete one edit.
4. Connect a non-production server database and run a query.
5. Name the first confusing or untrustworthy moment.

Fix repeated onboarding failures before seeking a larger audience.

## Public launch window

Publish one substantial release and concentrate honest outreach in the same 24–48 hour window.
Good channels depend on the story, but may include Show HN, Rust Users Forum, relevant database
communities, and a technical article. Follow each community's self-promotion rules and answer
questions rather than repeating promotional copy.

Suggested headline:

> Show HN: plusplus – a production-safe native SQL client built in Rust

Suggested one-sentence description:

> plusplus is a small cross-platform client for PostgreSQL, MySQL, SQL Server, and SQLite,
> with staged edits, session-level read-only mode, no Electron, and no telemetry.

Do not buy stars, trade stars, or automate unsolicited posts. Measure successful installs,
repeat users, actionable issues, and contributors—not only the star count.
