#!/usr/bin/env bash
# Install (or replace) plusplus.app in /Applications.
#
# Usage: packaging/macos/install.sh
#   Run after packaging/macos/make-dmg.sh or scripts/release.sh.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=scripts/version.sh
source "${REPO_ROOT}/scripts/version.sh"

APP_NAME="plusplus"
VERSION="$(plusplus_read_version "$REPO_ROOT")"
SOURCE="${REPO_ROOT}/target/dist/${APP_NAME}.app"
DEST="/Applications/${APP_NAME}.app"

[ -d "$SOURCE" ] || {
  echo "missing ${SOURCE}" >&2
  echo "build first: scripts/release.sh   or   packaging/macos/make-dmg.sh" >&2
  exit 1
}

if [ -d "$DEST" ]; then
  echo "→ removing old ${DEST}"
  rm -rf "$DEST"
fi

echo "→ installing v${VERSION} → ${DEST}"
cp -R "$SOURCE" "$DEST"

echo "✓ done — launch from Applications or: open -a ${APP_NAME}"
