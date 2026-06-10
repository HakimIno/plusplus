#!/usr/bin/env bash
# Remove plusplus.app from /Applications.
#
# Usage: packaging/macos/uninstall.sh
#
set -euo pipefail

APP_NAME="plusplus"
DEST="/Applications/${APP_NAME}.app"

if [ ! -d "$DEST" ]; then
  echo "not installed (${DEST} not found)"
  exit 0
fi

echo "→ removing ${DEST}"
rm -rf "$DEST"
echo "✓ uninstalled"
