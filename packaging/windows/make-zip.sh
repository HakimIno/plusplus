#!/usr/bin/env bash
# Package the Windows release build into a versioned portable zip in target/dist.
#
# Runs under Git Bash on a Windows runner (release.yml). Portable distribution:
# unzip anywhere and run plusplus.exe — config lives in %APPDATA%\plusplus, so the
# install directory stays read-only-safe. An installer (Inno Setup) can join later
# without changing this asset's name, which the in-app updater will key on.
#
# Usage (from repo root, after `cargo build --release -p plusplus-app`):
#   packaging/windows/make-zip.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../../scripts/version.sh
source "${REPO_ROOT}/scripts/version.sh"
VERSION="$(plusplus_read_version "$REPO_ROOT")"

EXE="${REPO_ROOT}/target/release/plusplus.exe"
if [ ! -f "$EXE" ]; then
  echo "error: ${EXE} not found — run 'cargo build --release -p plusplus-app' first" >&2
  exit 1
fi

DIST="${REPO_ROOT}/target/dist"
STAGE_NAME="plusplus-${VERSION}-windows"
STAGE="${DIST}/${STAGE_NAME}"
ZIP="${DIST}/plusplus-${VERSION}-x86_64-windows.zip"

rm -rf "$STAGE"
rm -f "$ZIP"
mkdir -p "$STAGE"
cp "$EXE" "$STAGE/"
cp "${REPO_ROOT}/README.md" "$STAGE/"

# 7z ships on the GitHub windows runners and produces deterministic-enough zips;
# fail loudly rather than silently produce nothing if it's missing.
if ! command -v 7z >/dev/null 2>&1; then
  echo "error: 7z not found on PATH — required to build the zip" >&2
  exit 1
fi
(cd "$DIST" && 7z a -tzip "$(basename "$ZIP")" "$STAGE_NAME" >/dev/null)

rm -rf "$STAGE"
echo "→ $(ls -lh "$ZIP" | awk '{print $5}') ${ZIP}"
