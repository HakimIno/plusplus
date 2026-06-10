#!/usr/bin/env bash
# Build a versioned release and (optionally) create a matching git tag.
#
# Version lives in the root Cargo.toml ([workspace.package].version). Every release
# gets a git tag `vX.Y.Z` that must match that field, and a DMG named plusplus-X.Y.Z.dmg.
#
# Usage (from repo root):
#   scripts/release.sh              # build + package (host arch)
#   scripts/release.sh --universal  # build both macOS targets, lipo, package
#   scripts/release.sh --tag        # also create annotated git tag vX.Y.Z
#   scripts/release.sh --install    # build, package, replace /Applications/plusplus.app
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/version.sh
source "${REPO_ROOT}/scripts/version.sh"

VERSION="$(plusplus_read_version "$REPO_ROOT")"
TAG="$(plusplus_git_tag "$REPO_ROOT")"
DO_TAG=0
DO_UNIVERSAL=0
DO_INSTALL=0

for arg in "$@"; do
  case "$arg" in
    --tag) DO_TAG=1 ;;
    --universal) DO_UNIVERSAL=1 ;;
    --install) DO_INSTALL=1 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "unknown flag: $arg" >&2
      exit 1
      ;;
  esac
done

cd "$REPO_ROOT"
echo "→ plusplus release v${VERSION} (tag ${TAG})"

if [ "$DO_TAG" -eq 1 ]; then
  if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "  tag ${TAG} already exists — skipping"
  elif ! git diff-index --quiet HEAD -- 2>/dev/null; then
    echo "  warning: working tree has uncommitted changes; tag will point at current HEAD" >&2
    git tag -a "$TAG" -m "Release ${TAG}"
    echo "  created tag ${TAG}"
  else
    git tag -a "$TAG" -m "Release ${TAG}"
    echo "  created tag ${TAG}"
  fi
fi

if [ "$DO_UNIVERSAL" -eq 1 ]; then
  echo "→ cargo build --release (x86_64 + aarch64)"
  cargo build --release --bin plusplus --target x86_64-apple-darwin
  cargo build --release --bin plusplus --target aarch64-apple-darwin
else
  echo "→ cargo build --release (host arch)"
  cargo build --release --bin plusplus
fi

echo "→ packaging .app + .dmg"
bash packaging/macos/make-dmg.sh

if [ "$DO_INSTALL" -eq 1 ]; then
  bash packaging/macos/install.sh
fi

echo "✓ release v${VERSION} ready"
echo "  app: target/dist/plusplus.app"
echo "  dmg: target/dist/plusplus-${VERSION}.dmg"
if [ "$DO_TAG" -eq 1 ]; then
  echo "  tag: ${TAG}  (push with: git push origin ${TAG})"
fi
