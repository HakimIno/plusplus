#!/usr/bin/env bash
# Regenerate background.tiff (the DMG window background, 1x + retina 2x) from
# dmg-background.svg. Run after editing the SVG; the .tiff is committed so the
# normal packaging flow doesn't need rsvg-convert.
#
# Requires: rsvg-convert (brew install librsvg) and tiffutil (macOS built-in).
set -euo pipefail
cd "$(dirname "$0")"

command -v rsvg-convert >/dev/null || { echo "missing rsvg-convert — brew install librsvg"; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

rsvg-convert -w 660  -h 400 dmg-background.svg -o "${TMP}/bg.png"
rsvg-convert -w 1320 -h 800 dmg-background.svg -o "${TMP}/bg@2x.png"
# -cathidpicheck folds both scales into one TIFF that Finder picks the right one from.
tiffutil -cathidpicheck "${TMP}/bg.png" "${TMP}/bg@2x.png" -out background.tiff >/dev/null 2>&1

echo "✓ background.tiff"
