#!/usr/bin/env bash
# Package the release binary into plusplus.app and a distributable .dmg.
#
# Usage: packaging/macos/make-dmg.sh   (run from anywhere; paths are resolved from the repo root)
# Assumes `cargo build --release --bin plusplus` has already produced the binary.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

APP_NAME="plusplus"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
BIN="target/release/${APP_NAME}"
ICNS="crates/app/assets/icon/icon.icns"

DIST="target/dist"
APP="${DIST}/${APP_NAME}.app"
DMG="${DIST}/${APP_NAME}-${VERSION}.dmg"

[ -f "$BIN" ]  || { echo "missing $BIN — run: cargo build --release --bin ${APP_NAME}"; exit 1; }
[ -f "$ICNS" ] || { echo "missing $ICNS — run: crates/app/assets/icon/build.sh"; exit 1; }

echo "→ assembling ${APP} (v${VERSION})"
rm -rf "$APP"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
cp "$BIN" "${APP}/Contents/MacOS/${APP_NAME}"
cp "$ICNS" "${APP}/Contents/Resources/icon.icns"

cat > "${APP}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>     <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>      <string>com.${APP_NAME}.app</string>
  <key>CFBundleExecutable</key>      <string>${APP_NAME}</string>
  <key>CFBundleIconFile</key>        <string>icon</string>
  <key>CFBundleVersion</key>         <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key> <string>${VERSION}</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>LSMinimumSystemVersion</key>  <string>10.15</string>
  <key>NSHighResolutionCapable</key> <true/>
</dict>
</plist>
PLIST

# Ad-hoc sign so Gatekeeper treats it as a stable identity (no paid cert needed).
if command -v codesign >/dev/null 2>&1; then
  echo "→ ad-hoc codesigning"
  codesign --force --deep --sign - "$APP" 2>/dev/null || echo "  (codesign skipped)"
fi

echo "→ building ${DMG}"
rm -f "$DMG"
STAGE="$(mktemp -d)"
cp -R "$APP" "${STAGE}/"
ln -s /Applications "${STAGE}/Applications"      # drag-to-install target
hdiutil create -volname "${APP_NAME} ${VERSION}" \
  -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo "✓ done"
echo "  app: ${APP}"
echo "  dmg: ${DMG}"
