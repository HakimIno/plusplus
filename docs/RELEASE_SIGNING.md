# Release signing (required for in-app updates)

plusplus verifies every downloaded update before installing it. The in-app updater
downloads a release package (DMG on macOS, AppImage on Linux) **and** its detached
[minisign](https://jedisct1.github.io/minisign/) signature (`<package>.minisig`), then
checks the signature against a public key compiled into the app (`MINISIGN_PUBLIC_KEY` in
`crates/ui/src/update.rs`). An update that is unsigned,
tampered with, or signed by any other key is **refused** — even if a release or the GitHub
account is compromised, an attacker can't forge a signature without the private key.

Trust is therefore rooted in a private key the maintainer holds **offline**, never in
GitHub. This is fail-closed: until the steps below are done, the updater refuses all
updates (users can still download releases manually).

## One-time setup

### 1. Generate a key pair

```sh
brew install minisign            # or: apt install minisign
minisign -G -p plusplus.pub -s plusplus.key
```

- `plusplus.key` — **secret key. Keep it offline. Never commit it.**
- `plusplus.pub` — public key (safe to share).

Use a strong password when prompted (recommended).

### 2. Bake the public key into the app

`plusplus.pub` looks like:

```
untrusted comment: minisign public key ABCD...
RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3
```

Copy the **second line** (the base64 key, no comment) into `crates/ui/src/update.rs`:

```rust
pub const MINISIGN_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
```

Commit that change. It ships in the binary; clients now trust only this key.

### 3. Add the secret key to CI

In the GitHub repo: **Settings → Secrets and variables → Actions → New repository secret**

| Secret name           | Value                                              |
| --------------------- | -------------------------------------------------- |
| `MINISIGN_SECRET_KEY` | full contents of `plusplus.key`                    |
| `MINISIGN_PASSWORD`   | the key's password (omit if you created one with `-W`) |

The release workflow (`.github/workflows/release.yml`) signs each DMG and AppImage and
uploads every `.minisig` automatically. It fails the release if `MINISIGN_SECRET_KEY` is
missing, so a release can never ship an unsigned update.

## Signing a DMG manually (local release)

`scripts/release.sh` builds the DMG but does not sign it. To sign locally:

```sh
minisign -S -s plusplus.key -m target/dist/plusplus-<version>.dmg
# produces target/dist/plusplus-<version>.dmg.minisig
```

Upload **both** the `.dmg` and the `.dmg.minisig` to the GitHub release.

## Signing a Linux AppImage manually

Build the release binary and package it, then sign the resulting AppImage:

```sh
cargo build --release --bin plusplus
packaging/linux/make-appimage.sh
minisign -S -s plusplus.key -m target/dist/plusplus-<version>-x86_64.AppImage
```

Upload both the `.AppImage` and `.AppImage.minisig` to the GitHub release. Automatic
replacement is available only when plusplus itself is running from that AppImage; distro
packages remain under the control of their package manager.

## Rotating or revoking the key

The public key is pinned per build, so rotating it means shipping an app update with the
new `MINISIGN_PUBLIC_KEY` (signed by the **old** key). Plan rotations one release ahead;
publish each release signed by the key the currently-installed clients expect.
