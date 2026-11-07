#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Launch the Flatpak build of Comfy-GNOME. Builds + installs it (user scope) on first run
# or when called with --rebuild. Needs: flatpak-builder, org.gnome.Sdk//50, rust-stable extension.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
APP_ID=io.github.comfygnome.ComfyGnome
export GDK_BACKEND="${GDK_BACKEND:-x11}"
LOG="${XDG_RUNTIME_DIR:-/tmp}/comfy-gnome-flatpak-build.log"

if [ "${1:-}" = --rebuild ] || ! flatpak info --user "$APP_ID" >/dev/null 2>&1; then
  [ "${1:-}" = --rebuild ] && shift
  notify-send -i applications-graphics "Comfy-GNOME" "Building Flatpak (several minutes)…" 2>/dev/null || true
  echo "Building Flatpak (log: $LOG)…"
  if ! flatpak-builder --user --install --force-clean --install-deps-from=flathub \
        flatpak-build "build-aux/$APP_ID.json" 2>&1 | tee "$LOG"; then
    notify-send -u critical "Comfy-GNOME" "Flatpak build failed — see $LOG" 2>/dev/null || true
    exit 1
  fi
fi
exec flatpak run --env=GDK_BACKEND="$GDK_BACKEND" "$APP_ID" "$@"
