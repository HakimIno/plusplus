#!/usr/bin/env bash
# Regenerate every platform icon artifact from the single source of truth, icon.svg.
#
# Requires: rsvg-convert (brew install librsvg), python3 + Pillow.
# Uses iconutil on macOS when available, with Pillow as a fallback.
# Outputs:
#   icon.icns            — macOS app bundle icon
#   icon.ico             — Windows app/exe icon (16..256 packed)
#   png/icon-<size>.png  — Linux/hicolor + window icon source
#   website/public/app-icon.png — website brand icon
set -euo pipefail
cd "$(dirname "$0")"

SVG=icon.svg
PNG_SIZES=(16 24 32 48 64 128 256 512 1024)

echo "→ rasterizing PNGs from $SVG"
mkdir -p png
for s in "${PNG_SIZES[@]}"; do
  rsvg-convert -w "$s" -h "$s" "$SVG" -o "png/icon-${s}.png"
done

echo "→ building icon.icns (macOS)"
ICONSET=icon.iconset
rm -rf "$ICONSET"; mkdir -p "$ICONSET"
cp png/icon-16.png    "$ICONSET/icon_16x16.png"
cp png/icon-32.png    "$ICONSET/icon_16x16@2x.png"
cp png/icon-32.png    "$ICONSET/icon_32x32.png"
cp png/icon-64.png    "$ICONSET/icon_32x32@2x.png"
cp png/icon-128.png   "$ICONSET/icon_128x128.png"
cp png/icon-256.png   "$ICONSET/icon_128x128@2x.png"
cp png/icon-256.png   "$ICONSET/icon_256x256.png"
cp png/icon-512.png   "$ICONSET/icon_256x256@2x.png"
cp png/icon-512.png   "$ICONSET/icon_512x512.png"
cp png/icon-1024.png  "$ICONSET/icon_512x512@2x.png"
if command -v iconutil >/dev/null 2>&1 && iconutil -c icns "$ICONSET" -o icon.icns; then
  echo "  wrote icon.icns with iconutil"
else
  python3 - <<'PY'
from PIL import Image
Image.open("png/icon-1024.png").save("icon.icns", format="ICNS")
print("  wrote icon.icns with Pillow")
PY
fi
rm -rf "$ICONSET"

echo "→ building icon.ico (Windows)"
python3 - <<'PY'
from PIL import Image
sizes = [16, 24, 32, 48, 64, 128, 256]
# Base must be the largest frame; Pillow downscales it to each requested size.
base = Image.open("png/icon-256.png").convert("RGBA")
base.save("icon.ico", format="ICO", sizes=[(s, s) for s in sizes])
got = sorted(Image.open("icon.ico").info["sizes"])
print("  wrote icon.ico:", ", ".join(f"{w}px" for w, _ in got))
PY

echo "→ syncing website icon"
cp png/icon-512.png ../../../../website/public/app-icon.png

echo "✓ done"
