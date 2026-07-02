#!/usr/bin/env bash
# Install native Linux dependencies needed to build and smoke-run plusplus.
#
# Usage:
#   scripts/linux-deps.sh
#
# Supported families:
#   Ubuntu/Debian, Fedora/RHEL-like, Arch, openSUSE
set -euo pipefail

if [ ! -r /etc/os-release ]; then
  echo "cannot detect Linux distribution: /etc/os-release is missing" >&2
  exit 1
fi

# shellcheck disable=SC1091
. /etc/os-release

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    echo "need root privileges to install packages, and sudo is not available" >&2
    exit 1
  fi
}

ids=" ${ID:-} ${ID_LIKE:-} "

case "$ids" in
  *" debian "*|*" ubuntu "*)
    run_as_root apt-get update
    run_as_root apt-get install -y --no-install-recommends \
      ca-certificates curl file tar xz-utils build-essential pkg-config \
      libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxkbcommon-dev libwayland-dev \
      libgl1-mesa-dev libegl1-mesa-dev libfontconfig1-dev libdbus-1-dev \
      libxkbcommon-x11-0 xvfb xauth mesa-utils
    ;;
  *" fedora "*|*" rhel "*|*" centos "*)
    run_as_root dnf install -y \
      ca-certificates curl file tar xz gcc gcc-c++ make pkgconf-pkg-config \
      libX11-devel libXi-devel libXcursor-devel libXrandr-devel libxkbcommon-devel wayland-devel \
      mesa-libGL-devel mesa-libEGL-devel fontconfig-devel dbus-devel \
      libxkbcommon-x11 xorg-x11-server-Xvfb mesa-dri-drivers
    ;;
  *" arch "*)
    run_as_root pacman -Sy --noconfirm --needed \
      ca-certificates curl file tar xz base-devel pkgconf \
      libx11 libxi libxcursor libxrandr libxkbcommon wayland mesa fontconfig dbus \
      xorg-server-xvfb
    ;;
  *" opensuse "*|*" suse "*)
    run_as_root zypper --non-interactive install \
      ca-certificates curl file tar xz gcc gcc-c++ make pkg-config \
      libX11-devel libXi-devel libXcursor-devel libXrandr-devel libxkbcommon-devel wayland-devel \
      Mesa-libGL-devel Mesa-libEGL-devel fontconfig-devel dbus-1-devel \
      xorg-x11-server-extra Mesa-dri
    ;;
  *)
    echo "unsupported Linux distribution: ${PRETTY_NAME:-${ID:-unknown}}" >&2
    echo "install X11, Wayland, OpenGL/EGL, fontconfig, DBus, pkg-config, a C/C++ compiler, and Xvfb" >&2
    exit 1
    ;;
esac
