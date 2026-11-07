#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Launch Comfy-GNOME.
#   ./run.sh            native release build (built on first run / after changes)
#   ./run.sh --debug    native debug build
#   ./run.sh --flatpak  Flatpak (built + installed for the user on first run)
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
APP_ID=io.github.comfygnome.ComfyGnome
export GDK_BACKEND="${GDK_BACKEND:-x11}"   # X11 is the primary target

case "${1:-}" in
  --flatpak)
    flatpak info --user "$APP_ID" >/dev/null 2>&1 ||
      flatpak-builder --user --install --force-clean flatpak-build "build-aux/$APP_ID.json"
    exec flatpak run --env=GDK_BACKEND="$GDK_BACKEND" "$APP_ID" ;;
  --debug)
    exec cargo run ;;
  *)
    cargo build --release --quiet
    exec target/release/comfy-gnome ;;
esac
