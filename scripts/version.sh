#!/usr/bin/env bash
# Read the workspace version from the root Cargo.toml (single source of truth).
# Usage: source scripts/version.sh && echo "$PLUSPLUS_VERSION"
set -euo pipefail

plusplus_repo_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd
}

plusplus_read_version() {
  local root="${1:-$(plusplus_repo_root)}"
  grep -m1 '^version' "${root}/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/'
}

plusplus_git_tag() {
  echo "v$(plusplus_read_version "$@")"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  plusplus_read_version
fi
