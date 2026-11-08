#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Launch the native (system) build of Comfy-GNOME. Rebuilds when sources changed.
# First build takes a few minutes; a desktop notification tells you it's happening.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
export GDK_BACKEND="${GDK_BACKEND:-x11}"
# App shortcuts inherit the session PATH, which usually lacks ~/.cargo/bin.
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
LOG="${XDG_RUNTIME_DIR:-/tmp}/comfy-gnome-build.log"

BIN=target/release/comfy-gnome
if [ ! -x "$BIN" ] || [ -n "$(find src data build.rs Cargo.toml Cargo.lock -newer "$BIN" -print -quit)" ]; then
  notify-send -i applications-graphics "Comfy-GNOME" "Building (first run takes a few minutes)…" 2>/dev/null || true
  echo "Building Comfy-GNOME (log: $LOG)…"
  if ! cargo build --release 2>&1 | tee "$LOG"; then
    notify-send -u critical "Comfy-GNOME" "Build failed — see $LOG" 2>/dev/null || true
    exit 1
  fi
fi
exec "$BIN" "$@"
