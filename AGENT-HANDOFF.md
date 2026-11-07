# AGENT HANDOFF

_Last updated: session 1._

## State
Phase 1 complete (see `ROADMAP.md`). `cargo build` clean (0 warnings), `cargo test` passes (1 test).
Verified on X11 (`GDK_BACKEND=x11`): splash renders, Skip → workspace, log shows connection polling,
node drag, zoom-at-pointer. Screenshots: `docs/screenshots/{splash,workspace,canvas-drag-zoom}.png`.
Not yet verified: launching a real ComfyUI script end-to-end, Flatpak build.

Env: rustc 1.95, GTK 4.14.5, libadwaita 1.5.0 (crates: libadwaita 0.8 `v1_5`, gtk4 0.10 `v4_14`).
Flatpak: GNOME 50 runtime + rust-stable 25.08 installed.

## User context
- User's scripts live in `/home/montel/Documents/ComfyUI/*.sh` (e.g. `start_comfy_simple.sh`,
  `start_comfy_mcp.sh`). Most start ComfyUI then run `thorium-browser --app=...` **in the foreground** and
  clean up ComfyUI when the browser exits. User does NOT want the browser opened → "Block Browser Launch"
  switch (default on) PATH-shims browsers to `sleep infinity` / openers to `exit 0`. See ARCHITECTURE.
- `start_comfy_mcp.sh`/`start_comfy_hefty.sh` use ports 8188 / 8973 — user must set port on the splash.

## Known issues / quirks
- Scripts calling `sudo` (nvidia-smi tuning in `launch.sh`, `start_comfy_forKrita.sh`) have no TTY → sudo fails
  or hangs. Options: pkexec, or tell the user to use NOPASSWD. Not handled.
- Browser shims only catch bare command names, not absolute paths.
- Some symbolic icons (sidebar toggle, entry-row edit) render as broken images under the user's icon theme;
  Adwaita has them — maybe bundle icons via gresource later.
- Window may open partly off-screen on the 3840×1080 dual-monitor setup (WM placement).
- Screenshot tips: `scrot -o -a X,Y,W,H file` (window geometry via `xdotool getwindowgeometry`);
  `import -window` produced empty output; `scrot -u` grabbed the wrong window. Read tool sometimes needs a retry
  right after a write.
- `pkill -f gnome-comfyui` kills your own shell (pattern matches the command line) — use `pkill -x gnome-comfyui`.
- Flatpak manifest uses `--share=network` at build time (not Flathub-acceptable; see Phase 6).

## Next steps (Phase 2)
1. Test "Launch Script" with `/home/montel/Documents/ComfyUI/start_comfy_simple.sh`; confirm no Thorium window,
   log streams, backend connects, closing the window kills python (`pgrep -af main.py`).
2. Add a backend worker thread: `ureq` + `tungstenite` + `serde_json`, messages over `async_channel`.
3. Fetch `/object_info`, build node registry; replace `add_demo_nodes` with a node loaded from it.
4. Queue sidebar wired to `/queue` + WS `status`/`progress`.
5. Persist splash config (last script, args, host/port).
