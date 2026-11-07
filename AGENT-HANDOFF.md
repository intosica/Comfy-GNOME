# AGENT HANDOFF

_Last updated: session 1 (end)._

Project: **Comfy-GNOME** at `/home/montel/Documents/Comfy-GNOME` (renamed from `GNOME-Comfyu` → `GNOME-ComfyUI` → this).
Crate/binary `comfy-gnome`, app id `io.github.comfygnome.ComfyGnome` (`APP_ID` in `src/main.rs`).

## State
Phase 1 complete (see `ROADMAP.md`). `cargo build` clean (0 warnings), `cargo test` passes (2 tests).
Verified on X11 (`GDK_BACKEND=x11`):
- Splash auto-detects `/home/montel/Documents/ComfyUI`; **Launch ComfyUI** (folder mode) starts
  `venv/bin/python main.py --port 8188`, log streams (ANSI stripped), backend connects.
- Script mode with `start_comfy_hefty.sh`: ComfyUI starts, Thorium replaced by the `sleep infinity` shim.
- Closing the window kills the backend process group (folder mode verified; script-mode group kill verified
  manually with `kill -TERM -- -PGID`, which runs the script's cleanup trap). `kill -9` of the app → PDEATHSIG kills python.
- Skip → workspace, connection polling, node drag, zoom-at-pointer, bundled icons render under the user's `gnome` icon theme.
Screenshots: `docs/screenshots/`.

Not yet verified: Flatpak build/run (`./run.sh --flatpak`); script-mode window-close after the
`connect_close_request` fix (only folder mode re-tested — same code path).

Env: rustc 1.95, GTK 4.14.5, libadwaita 1.5.0 (crates: libadwaita 0.8 `v1_5`, gtk4 0.10 `v4_14`, libc,
build-dep glib-build-tools 0.21). Flatpak: GNOME 50 runtime + rust-stable 25.08 installed.

## User context
- ComfyUI checkout: `/home/montel/Documents/ComfyUI` (has `venv/`). User scripts `*.sh` there mostly start ComfyUI
  then run `thorium-browser --app=...` in the foreground and clean up when it exits. User does NOT want the
  browser → "Block Browser Launch" (default on). Folder mode is the preferred, simplest path.
- Ports: `start_comfy_hefty.sh` 8973; `start_comfy_mcp.sh` `${COMFY_PORT:-8188}` — set the port on the splash.
- User wants Flatpak and native builds to behave the same.

## Known issues / quirks
- Scripts that need an interactive `sudo` (`launch.sh`, `start_comfy_forKrita.sh`) have no TTY → fail. `sudo -n` ones are fine.
- Browser shims only catch bare command names, not absolute paths.
- GTK4 `connect_destroy` on child widgets / window did NOT fire on app quit → backend stop is hooked on
  `window.connect_close_request` (`src/main.rs`). Keep it there.
- Splash config isn't persisted yet (folder is auto-detected each launch).
- Window may open off-screen on the 3840×1080 dual-monitor setup; tests do `xdotool windowmove $W 100 40`.
- Testing tips: screenshot with `scrot -o -a X,Y,W,H file` (not `import`/`scrot -u`); the read tool often
  needs one retry after a fresh screenshot. Use `pkill -x comfy-gnome` / `pgrep -f "[m]ain.py"` —
  plain `pkill -f <name>` kills your own shell. Driving the GTK file chooser with xdotool is unreliable
  (keystrokes land in the wrong dialog) — ask the user to pick files manually instead.

## Next steps (Phase 2)
1. Persist splash config (folder, script, args, host/port, switch) — keyfile in `glib::user_config_dir()`.
2. Backend worker thread: `ureq` + `tungstenite` + `serde_json`, messages over `async_channel`.
3. Fetch `/object_info`, build node registry; replace `add_demo_nodes`.
4. Queue sidebar wired to `/queue` + WS `status`/`progress`.
5. Run `./run.sh --flatpak` once and fix anything sandbox-specific.
