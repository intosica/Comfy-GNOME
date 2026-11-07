# AGENT HANDOFF

_Last updated: session 2 (in progress). Phases 1–2 code complete; Phase 2 still open on the queue panel, previews and third-party UIs._

Project: **Comfy-GNOME** at `/home/montel/Documents/Comfy-GNOME` (renamed from `GNOME-Comfyu` → `GNOME-ComfyUI` → this).
Crate/binary `comfy-gnome`, app id `io.github.comfygnome.ComfyGnome` (`APP_ID` in `src/main.rs`).

## State
Phase 1 complete. Phase 2 code-complete except queue panel / previews / custom-node UI (see `ROADMAP.md`).
`cargo build` clean, `cargo test` passes (6 unit tests; 3 `--ignored` live tests against a running backend).
Verified on X11 (`GDK_BACKEND=x11`):
- Splash auto-detects `/home/montel/Documents/ComfyUI`; **Launch ComfyUI** (folder mode) starts
  `venv/bin/python main.py --port 8188`, log streams (ANSI stripped), backend connects.
- Script mode with `start_comfy_hefty.sh`: ComfyUI starts, Thorium replaced by the `sleep infinity` shim.
- Closing the window kills the backend process group (folder mode verified; script-mode group kill verified
  manually with `kill -TERM -- -PGID`, which runs the script's cleanup trap). `kill -9` of the app → PDEATHSIG kills python.
- Skip → workspace, connection polling, node drag, zoom-at-pointer, bundled icons render under the user's `gnome` icon theme.
Screenshots: `docs/screenshots/`.

Launchers (session 1 end): `launch-system.sh` (rebuilds release only if src/data/Cargo.* newer than the binary,
desktop notification while building) and `launch-flatpak.sh [--rebuild]` (flatpak-builder --user --install).
Menu entry `~/.local/share/applications/comfy-gnome-system.desktop` → launch-system.sh; Flatpak exports its own.
App icon: `data/icons/hicolor/scalable/apps/io.github.comfygnome.ComfyGnome.svg` (original).
Flatpak verified: builds, installs, runs, auto-detects the ComfyUI folder; `flatpak-spawn --host` reaches the host
venv python from inside the sandbox. NOT yet verified: full launch-from-Flatpak click-through (user was using the
desktop), script-mode window-close after the `connect_close_request` fix.
Why the user "couldn't launch": old `run.sh` did a silent multi-minute first release build — looked like nothing happened.

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
- Testing tips: screenshot with `scrot -o -a X,Y,W,H file`; the read tool often needs one retry after a fresh
  screenshot. **`~/.local/bin/import` on this machine is a stub that exits 0 and writes nothing** — do not use it.
  Best isolation: `Xvfb :99 -screen 0 1600x1000x24`, run the app with `DISPLAY=:99`, screenshot with
  `DISPLAY=:99 scrot -o shot.png` (import/ImageMagick `import` is hijacked). Click via `DISPLAY=:99 xdotool`.
  Do NOT wrap the app in `dbus-run-session` (window stays 1×1); after `pkill -x comfy-gnome` wait ~3 s before
  relaunching or D-Bus registration fails with "G5024 Remote peer disconnected". Use `pkill -x comfy-gnome` / `pgrep -f "[m]ain.py"` —
  plain `pkill -f <name>` kills your own shell. Driving the GTK file chooser with xdotool is unreliable
  (keystrokes land in the wrong dialog) — ask the user to pick files manually instead.

## Session 2 — backend protocol (done)
- `src/api.rs`: `Client` (`ureq` + `tungstenite`, blocking on `gio::spawn_blocking`) — `get`, `post`,
  `queue_prompt`, `interrupt`, `delete_queued`, `workflows()`, `workflow_json(file)`; `spawn_ws(tx)` reconnects
  every 2 s and emits `WsEvent::{Connected,Disconnected,Status,Progress,Executing,ExecutionStart,ExecutionError,Preview,Other}`.
  `percent_encode` matters: the userdata route is one segment, `workflows%2Fx.json` (literal `/` → 404).
- `src/registry.rs`: `/object_info` → `Registry` (kinds incl. v3 `COMBO`, `COMFY_DYNAMICCOMBO_V3`, custom widget
  detection). 4775 node types / 792 categories in ~3.3 s from the local backend.
- `src/workflow.rs`: saved-workflow JSON (0.4 tuple links, subgraph defs v1 object links, groups, `extra.ds` viewport).
  Node ids are `String` (`"39:38"` exists in real files).
- `src/prompt.rs`: `graphToPrompt` port — subgraph flattening (exec ids `"12:5"`), mute/bypass passthrough,
  promoted widgets, dynamic-combo children, `{"__value__": []}`, extension widgets (rgthree Power Lora Loader,
  LoRA Manager autocomplete+loras). Golden harness vs. real prompts: `cargo test golden -- --ignored --nocapture`
  → **exact 30/60, nodes 805/821** (data in `/tmp/golden`, `/tmp/oi.json`; not in git — private prompts).
- `src/graph_view.rs`: workflow → canvas scene (sizes grown to fit slots+widgets, colors, groups, subgraph glyph).
- Workspace: workflows sidebar (userdata list, filter, reload-on-map, row-selected → load, `&` escaped — AdwActionRow
  titles are Pango markup), fit/reset buttons, Run button + Ctrl+Enter (`prompt::build` → `POST /prompt` with
  `extra_pnginfo.workflow`), subgraph drill-down + back arrow, `extra.ds` viewport restore.
- Verified: list 158 workflows, load a 47-node graph at its saved 72 % zoom, double-click **Face Detailer** →
  subgraph shows Inputs/Outputs pseudo-nodes and its inner node; Run queued a real prompt (executed successfully,
  WS `executed`/`execution_success` logged).

## Session 2b — queue + monitor
- Queue sidebar page: `/queue` snapshot (`api::Queue{running,pending}`, `QueueItem{number,prompt_id,nodes,classes}`),
  rebuilt on every WS `status`/`executing`/`execution_*`/`progress_state` event but **only when the snapshot
  string changed** (`queue_key`), rows = Running/Pending `#number` + short prompt id + node count + up to 3
  class_types; suffix button stops (interrupt) or removes (delete_queued); header row has a global
  destructive **Interrupt** (sensitive only while something runs) and reload. Empty state = "Queue is empty".
- DaSiWa monitor: header MenuButton (icon + `CPU 5% · RAM 72%`), popover with threads, RAM/swap, per-GPU
  utilisation/memory/temperature. Hidden until the first `dasiwa.system_monitor` event, so it silently does
  nothing on installs without DaSiWa nodes. `update_monitor` never logs (1 msg/s).
- Gotcha: `refresh_queue` is fired from several events at once — the JSON-string compare keeps it from
  rebuilding rows constantly. Don't drop it.
- Verified on Xvfb: queue empty state, live Running #26/Pending #27 rows with correct node counts and
  stop/trash buttons, monitor readout updating (screenshot `docs/screenshots/phase2-queue.png`). Queue rows
  only stay pending for a moment, so for UI tests queue several tiny `EmptyImage → PreviewImage` prompts
  via curl (`POST /prompt`) instead of a real generation.

## Next steps (Phase 2 remainder → Phase 3)
1. Previews: WS binary type 1 (JPEG/PNG) + type 4 (preview with metadata), `executed.output.images` → `/view`
   thumbnails in a bottom/gallery panel; parse `progress_state` instead of dumping raw JSON to the log.
3. Node library (sidebar tab): search `/object_info`, drag/insert node → start real graph editing.
4. Third-party UIs (research saved in `docs/research/custom-node-uis.md`): DaSiWa system monitor widget (WS
   `dasiwa.system_monitor` every 1 s — currently filtered out of the log), LC Align guides, LoRA Manager
   button/browser (`/loras` in the default browser: webkitgtk-6.0 is not installed on this host).
5. Click-through test of Launch ComfyUI inside the Flatpak (host spawn + close kills python).
