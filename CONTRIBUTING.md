# Contributing to plusplus

Thanks for helping make database work faster and safer. Focused bug fixes, database-specific
test cases, accessibility improvements, themes, documentation, and small UX improvements are
all useful contributions.

## Before opening a change

- Search existing issues and pull requests first.
- Open an issue before a large feature or architecture change so effort is not duplicated.
- Never include database credentials, production data, connection strings, audit logs, or
  screenshots containing sensitive information.
- Report security vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## Development setup

Install the stable Rust toolchain, then run:

```bash
cargo check --workspace --all-targets
cargo test --workspace --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

SQLite is bundled and is the fastest way to exercise the full stack:

```bash
cargo run --bin plusplus
```

Add `examples/sample.sqlite` as a SQLite connection. Platform packaging instructions are in
the main [README](README.md#build-on-your-platform).

Some SSH tunnel tests bind loopback sockets. Sandboxed environments that prohibit local
network listeners cannot run those tests, but they run normally in GitHub Actions.

## Pull requests

Keep changes narrow and explain the user-visible outcome. A good pull request includes:

1. The problem and the database/platform affected.
2. A small implementation without unrelated refactoring.
3. A regression test when behavior changes.
4. A screenshot for visible UI changes.
5. Confirmation that formatting, tests, and Clippy pass.

Use the existing code style and do not edit generated or unrelated snapshot files. UI snapshot
tests are intentionally ignored in the normal suite; update a baseline only when the visual
change is deliberate and describe it in the pull request.

## Themes

Themes are a low-risk first contribution. Copy `examples/themes/dracula.json`, follow the
[theme format](docs/THEMES.md), and include a screenshot with the pull request.

## Licensing

By contributing, you agree that your contribution is licensed under the same choice of
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) terms as the project.
