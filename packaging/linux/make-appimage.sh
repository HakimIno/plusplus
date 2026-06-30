#!/usr/bin/env bash
# Package the release binary and its shared-library dependencies as an x86_64 AppImage.
#
# Usage: packaging/linux/make-appimage.sh
# Run after: cargo build --release --bin plusplus
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=scripts/version.sh
source "${REPO_ROOT}/scripts/version.sh"

cd "$REPO_ROOT"

if [ "$(uname -m)" != "x86_64" ]; then
  echo "AppImage packaging currently supports x86_64 only" >&2
  exit 1
fi

VERSION="$(plusplus_read_version "$REPO_ROOT")"
BINARY="${REPO_ROOT}/target/release/plusplus"
DIST="${REPO_ROOT}/target/dist"
APPDIR="${REPO_ROOT}/target/plusplus.AppDir"
TOOLS="${REPO_ROOT}/target/appimage-tools"
LINUXDEPLOY="${TOOLS}/linuxdeploy-x86_64.AppImage"
OUTPUT="${DIST}/plusplus-${VERSION}-x86_64.AppImage"

[ -x "$BINARY" ] || {
  echo "missing ${BINARY} — run: cargo build --release --bin plusplus" >&2
  exit 1
}

mkdir -p "$TOOLS" "$DIST"
if [ ! -x "$LINUXDEPLOY" ]; then
  echo "→ downloading linuxdeploy"
  curl --fail --location --retry 3 \
    --output "$LINUXDEPLOY" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
  chmod 0755 "$LINUXDEPLOY"
fi

echo "→ assembling AppDir"
rm -rf "$APPDIR"
mkdir -p \
  "${APPDIR}/usr/bin" \
  "${APPDIR}/usr/share/applications" \
  "${APPDIR}/usr/share/icons/hicolor/256x256/apps"
install -m 0755 "$BINARY" "${APPDIR}/usr/bin/plusplus"
install -m 0644 packaging/linux/plusplus.desktop \
  "${APPDIR}/usr/share/applications/plusplus.desktop"
install -m 0644 crates/app/assets/icon/png/icon-256.png \
  "${APPDIR}/usr/share/icons/hicolor/256x256/apps/plusplus.png"

echo "→ bundling shared libraries and creating ${OUTPUT}"
rm -f "$OUTPUT"
ARCH=x86_64 \
LINUXDEPLOY_OUTPUT_VERSION="$VERSION" \
LDAI_OUTPUT="$OUTPUT" \
APPIMAGE_EXTRACT_AND_RUN=1 \
NO_STRIP=1 \
  "$LINUXDEPLOY" --appimage-extract-and-run \
    --appdir "$APPDIR" \
    --output appimage

[ -f "$OUTPUT" ] || {
  echo "linuxdeploy did not create ${OUTPUT}" >&2
  exit 1
}
chmod 0755 "$OUTPUT"

echo "✓ AppImage ready: ${OUTPUT}"
