#!/usr/bin/env bash
# Build plusplus on Linux, with optional dependency installation and GUI smoke run.
#
# Usage:
#   scripts/linux-build.sh
#   scripts/linux-build.sh --install-deps
#   scripts/linux-build.sh --install-rust
#   scripts/linux-build.sh --smoke
#   scripts/linux-build.sh --install-deps --install-rust --smoke
#   scripts/linux-build.sh --debug --run
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE_FLAG=(--release)
PROFILE_DIR="release"
INSTALL_DEPS=0
INSTALL_RUST=0
RUN_APP=0
SMOKE=0

for arg in "$@"; do
  case "$arg" in
    --install-deps) INSTALL_DEPS=1 ;;
    --install-rust) INSTALL_RUST=1 ;;
    --run) RUN_APP=1 ;;
    --smoke) SMOKE=1 ;;
    --release)
      PROFILE_FLAG=(--release)
      PROFILE_DIR="release"
      ;;
    --debug)
      PROFILE_FLAG=()
      PROFILE_DIR="debug"
      ;;
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

if [ "$INSTALL_DEPS" -eq 1 ]; then
  scripts/linux-deps.sh
fi

if [ "$INSTALL_RUST" -eq 1 ] && ! command -v cargo >/dev/null 2>&1; then
  if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required for --install-rust; run scripts/linux-build.sh --install-deps first" >&2
    exit 1
  fi
  echo "-> installing Rust with rustup"
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1090
  source "${CARGO_HOME:-$HOME/.cargo}/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is not installed; install Rust or rerun with --install-rust" >&2
  exit 1
fi

echo "-> cargo build ${PROFILE_FLAG[*]} --bin plusplus"
cargo build "${PROFILE_FLAG[@]}" --bin plusplus

BINARY="target/${PROFILE_DIR}/plusplus"

if [ "$SMOKE" -eq 1 ]; then
  if ! command -v xvfb-run >/dev/null 2>&1; then
    echo "xvfb-run is required for --smoke; run scripts/linux-build.sh --install-deps first" >&2
    exit 1
  fi
  if ! command -v timeout >/dev/null 2>&1; then
    echo "timeout is required for --smoke" >&2
    exit 1
  fi

  echo "-> smoke run ${BINARY} under Xvfb"
  set +e
  xvfb-run -a timeout 8s "$BINARY"
  status=$?
  set -e

  # A successful GUI launch is expected to keep running until timeout stops it.
  # Exit 124 is GNU timeout's code for "command timed out".
  if [ "$status" -ne 0 ] && [ "$status" -ne 124 ]; then
    echo "smoke run failed with exit code ${status}" >&2
    exit "$status"
  fi
fi

if [ "$RUN_APP" -eq 1 ]; then
  echo "-> running ${BINARY}"
  exec "$BINARY"
fi

echo "ok: ${BINARY}"
