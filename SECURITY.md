# Security

Security is a first-class requirement of plusplus, not an afterthought. This document is
the project's security checklist: what is guaranteed, where it is implemented, and how to
verify each claim against the code.

## Checklist

| # | Guarantee | Status |
|---|-----------|--------|
| 1 | Passwords are never stored in files | ✅ |
| 2 | Secrets live in the OS keychain | ✅ |
| 3 | TLS for every server database | ✅ |
| 4 | Signed binary | ⚠️ partial — see below |
| 5 | Signed auto-update | ✅ |
| 6 | Telemetry is opt-in | ✅ (stronger: there is none) |
| 7 | No query ever leaves your machine | ✅ |
| 8 | SSH tunneling with host-key verification | ✅ |
| 9 | Audit log | ✅ |
| 10 | Read-only mode for production databases | ✅ |

## Details

### 1–2. Passwords in the OS keychain, never in files

Saved connections are persisted to `connections.json` **without any secret field** — the
`ConnectionConfig` struct (`crates/core/src/model.rs`) simply has no password member.
Passwords, SSH passphrases, and key passphrases are stored in the OS keychain (macOS
Keychain, Windows Credential Manager, or the Secret Service on Linux) via the `keyring`
crate, keyed by the connection's id (`crates/core/src/secrets.rs`). A per-launch,
memory-only session cache keeps keychain prompts to at most one per secret; it is never
persisted and is evicted on password change/delete.

**Verify:** `grep -i password ~/.config/plusplus/connections.json` — no matches.

### 3. TLS for every server database

Every server backend (PostgreSQL, MySQL/MariaDB, SQL Server) supports the full `sslmode`
vocabulary — Disable / Prefer / Require / Verify CA / Verify Full — with client
certificates (mutual TLS) where the driver supports them. **New connections default to
`Require`**, so traffic is encrypted with no silent plaintext downgrade; the connection
editor shows an inline warning for every non-verifying mode, nudging toward Verify Full.
The TLS stack is rustls with the system trust store (no OpenSSL dependency). SQLite is a
local file and has no transport.

**Verify:** `SslMode` in `crates/core/src/model.rs`; per-backend mapping in
`crates/core/src/backends/{postgres,mysql,mssql}.rs`.

### 4. Signed binary — the one open item

Release DMGs are **minisign-signed for the updater** (see #5), but the macOS app bundle is
**not yet Apple code-signed/notarized** — that requires an Apple Developer ID certificate
(paid program), and faking it is not possible. Consequences until then: Gatekeeper warns on
first launch, and the keychain cannot pin an "always allow" to a stable code signature.
This is the top item on the roadmap before a 1.0. Windows binaries are likewise not yet
Authenticode-signed.

### 5. Signed auto-update

The in-app updater downloads a release package from GitHub and **verifies its minisign
(Ed25519) signature against a public key embedded in the binary before installing —
fail-closed**. An unsigned or tampered package is refused, so a compromised GitHub account
or MITM cannot ship code through the updater. CI signs every release and fails loudly if
the signing secret is missing.

**Verify:** `MINISIGN_PUBLIC_KEY` and the verification step in `crates/ui/src/update.rs`;
signing in `.github/workflows/release.yml`; key handling in `docs/RELEASE_SIGNING.md`.

### 6–7. No telemetry, no query egress

plusplus contains **no telemetry, analytics, or crash reporting of any kind** — there is
nothing to opt into. The application makes exactly one kind of network request of its own:
an update check against the GitHub Releases API at launch, which sends no user data and
can be disabled in **Settings → Privacy → "Check for updates at launch"**. Everything
else on the wire is your own database connections. Queries, results, and schema never
leave your machine; history and audit logs are local files.

**Verify:** the only `reqwest` usage in the tree is `crates/ui/src/update.rs`:
`grep -rl reqwest crates/*/src`.

### 8. SSH tunnel with host-key verification

Server connections can run through an SSH bastion (`crates/core/src/tunnel.rs`). The
bastion's host key is **verified** against `~/.ssh/known_hosts` plus a plusplus-managed
`known_hosts` with accept-new (TOFU) semantics: a matching key connects, an unseen host is
recorded then trusted, and a **changed key is refused** as a potential MITM with a precise
error. Key files and passphrases follow the keychain rules above.

### 9. Audit log

An append-only audit trail (`crates/core/src/audit.rs`) records connections (success and
failure), executed statements, staged-edit commits, and schema migrations — each with
timestamp, connection, target (`user@host:port/db`), outcome, and duration. One JSONL
file per month in `<config>/audit/`; files are **never compacted, rewritten, or clearable
from the app** (unlike the convenience query history). Passwords are never part of any
entry. Statement text can contain data values, so the trail can be disabled in
**Settings → Privacy** for sensitive work. The `action` tags (`connect`, `query`,
`edit_commit`, `schema_apply`) are a stable contract for SIEM ingestion.

### 10. Read-only mode

A per-connection **Read-only** switch (connection editor) that actually blocks writes
rather than asking for confirmation, enforced in two independent layers:

- **Application layer** (`crates/core/src/safety.rs`, `write_statements`): default-deny —
  a statement runs only if it is *provably* a read. Unknown verbs are refused, and
  statements that can smuggle writes are scanned: `WITH x AS (DELETE …) SELECT`,
  `EXPLAIN ANALYZE UPDATE …`, and `SELECT … INTO t` are all blocked. In-grid editing and
  the schema (DDL) editor are refused on read-only connections.
- **Session layer** (authoritative): PostgreSQL sessions set
  `default_transaction_read_only=on`, MySQL/MariaDB sessions run
  `SET SESSION TRANSACTION READ ONLY`, SQLite files are opened read-only at the engine
  level. So even a write hidden where no lexer can see it — `SELECT setval(…)`, a
  side-effecting function — is rejected by the server itself. SQL Server has no session
  equivalent (`ApplicationIntent=ReadOnly` is sent, but only readable secondaries enforce
  it), so on SQL Server the application layer is the effective guard.

Separately, a **Production** flag keeps confirmation-gating for destructive statements
(UPDATE/DELETE/DROP/TRUNCATE/ALTER/MERGE, including CTE-wrapped forms) on connections
where writes are still needed but should never be casual.

## Additional protections

- Generated DML/DDL always quotes identifiers and escapes values
  (`crates/core/src/model.rs`); generated UPDATE/DELETE always carry a WHERE clause.
- Destructive-statement detection is literal-, comment-, and identifier-aware, so a
  keyword inside a string or comment neither triggers nor hides a warning.
- SQL statement logging by the drivers is disabled; errors surface in the UI, secrets are
  never logged.
- The release updater, TLS, and SSH all share one audited crypto stack (rustls/ring).

## Reporting a vulnerability

**Do not open a public issue for a suspected vulnerability.** Use
[GitHub private vulnerability reporting](https://github.com/HakimIno/plusplus/security/advisories/new)
and include the affected version, impact, reproduction, and any suggested mitigation. Remove
real credentials and production data; a minimal synthetic proof of concept is preferred.

The maintainer will acknowledge a report as soon as practical, investigate it privately, and
coordinate disclosure after a fix is available. If the report turns out to be an ordinary bug,
it can then be moved to the public issue tracker without sensitive details.
