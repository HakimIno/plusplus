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

# --- styled installer DMG ----------------------------------------------------
# Built in two steps: a read-write image first, so Finder (via AppleScript) can
# lay out the window — background picture, icon view, icon positions — which it
# persists into the volume's .DS_Store; then compressed into the final UDZO.
BACKGROUND="packaging/macos/assets/background.tiff"
VOL_NAME="${APP_NAME}"

echo "→ staging volume"
rm -f "$DMG"
STAGE="$(mktemp -d)"
cp -R "$APP" "${STAGE}/"
ln -s /Applications "${STAGE}/Applications"      # drag-to-install target
if [ -f "$BACKGROUND" ]; then
  mkdir "${STAGE}/.background"
  cp "$BACKGROUND" "${STAGE}/.background/background.tiff"
fi
cp "$ICNS" "${STAGE}/.VolumeIcon.icns"           # disk icon while mounted

RW_DMG="${DIST}/${APP_NAME}-rw.dmg"
rm -f "$RW_DMG"
hdiutil create -volname "$VOL_NAME" -srcfolder "$STAGE" -ov \
  -format UDRW -fs HFS+ "$RW_DMG" >/dev/null
rm -rf "$STAGE"

echo "→ styling installer window"
MOUNT="/Volumes/${VOL_NAME}"
# A stale volume with the same name would make Finder script the wrong window.
hdiutil detach "$MOUNT" >/dev/null 2>&1 || true
hdiutil attach "$RW_DMG" -mountpoint "$MOUNT" >/dev/null
SetFile -a C "$MOUNT" 2>/dev/null || true        # honour .VolumeIcon.icns

osascript <<OSA
tell application "Finder"
  tell disk "${VOL_NAME}"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    -- 660x400 content area; matches the background picture's design.
    set the bounds of container window to {200, 120, 860, 548}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 110
    set text size of opts to 13
    try
      set background picture of opts to file ".background:background.tiff"
    end try
    -- The two slots the background's arrow bridges.
    set position of item "${APP_NAME}.app" of container window to {165, 185}
    set position of item "Applications" of container window to {495, 185}
    close
    open
    delay 1
    close
  end tell
end tell
OSA

# Give Finder a moment to flush .DS_Store before unmounting.
sync
hdiutil detach "$MOUNT" >/dev/null

echo "→ compressing ${DMG}"
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -f "$RW_DMG"

echo "✓ done"
echo "  app: ${APP}"
echo "  dmg: ${DMG}"
