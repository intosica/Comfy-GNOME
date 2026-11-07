# ROADMAP

Each phase ends with a runnable build, updated `AGENT-HANDOFF.md`, and screenshots in `docs/screenshots/`.

## Phase 1 — Shell & Prototype ✅ (session 1)
- [x] Cargo project, GPL-3.0 headers, LICENSE, Flatpak manifest + `.desktop`
- [x] Libadwaita splash: ComfyUI folder (auto-detected), optional launch script, extra args, browser-launch blocking, host/port, "Skip — Connect to Running Backend"
- [x] Folder launch (venv auto-activate) and script launch: own process group, log streamed to panel, killed on close/crash; `flatpak-spawn --host` in Flatpak
- [x] Bundled symbolic icons (gresource), `launch-system.sh` / `launch-flatpak.sh` launchers
- [x] Backend reachability poll (`GET /system_stats`)
- [x] Workspace: sidebar (Workflows/Queue placeholders), canvas, bottom log panel
- [x] Immediate-mode cairo canvas: zoom-at-pointer, pan, node drag + raise, Bezier wires, unit-tested transform

## Phase 2 — Backend Protocol (mostly done, session 2)
- [x] Persist splash config (last folder/script/args/host/port) — keyfile in `~/.config/comfy-gnome/launch.ini`
- [x] HTTP client (`/object_info`, `/prompt`, `/queue`, `/history`, `/view`, `/interrupt`, `/userdata`) on worker threads (`ureq`)
- [x] WebSocket `/ws?clientId=` (status, progress, executing, binary preview frames) → GTK via `async_channel`
- [x] Node definition registry from `/object_info` (types, inputs, widgets, categories, v3 dynamic combos)
- [x] Saved-workflow JSON parsing (0.4 + subgraphs) and workflow → API prompt conversion (`graphToPrompt` port),
      validated against 60 real prompts from output PNGs (exact 30/60, 805/821 nodes)
- [x] Workflows sidebar (`/userdata`), open a workflow, restore its saved viewport, drill into subgraphs
- [x] Run button (+Ctrl+Enter): queue the open workflow, log `prompt_id`, errors surface in the log
- [x] Queue sidebar: live queue from `/queue` + WS status, per-item stop/remove, global interrupt
- [x] DaSiWa system monitor readout in the header (WS `dasiwa.system_monitor`; hidden when absent)
- [x] Preview panel (bottom tab): WS binary preview frames + `executed.output.images` via `/view`
- [ ] Gallery/history of past outputs (only the newest image is shown today)

## Phase 3 — Real Graph Editing (essentially done, session 4)
- [x] Load ComfyUI workflow JSON (UI format, litegraph `nodes`/`links`/`groups`)
- [x] Workflow **tabs**: one `adw::TabView` per open workflow, switch/close (`Ctrl+W`), unsaved marker `•`
- [x] Save (`Ctrl+S`) via `POST /api/userdata/<percent-encoded file>`; positions, wiring, `extra.ds` viewport
      written back; unknown JSON fields preserved and **bit-faithful** (open → save → the file is unchanged, verified
      on a real 29-node graph and on one with a subgraph definition; `converted_node_count` unchanged by
      `comfyui validate workflow`; needs serde_json's `float_roundtrip`)
- [x] Slot **type colours** (16-entry palette from `LGraphCanvas.link_type_colors`)
- [x] Link editing: drag from an output/input to wire, drag off an input to unplug, **shift+drag** an output to
      de-anchor the far end (wire keeps hanging from its input, drop it on another output to re-source it), hover ring
      on type-compatible targets only
- [x] Node context menu: Bypass / Mute / Collapse / Edit Title / Duplicate / Delete, plus "Open Subgraph" on
      instances; empty-space menu: Add Node…, Add Group, Select All, Fit, Reset
- [x] Behaviour **bypass** (`mode: 4`) and **mute** (`mode: 2`): magenta / dimmed node, dashed passthrough drawn
      from each connected input to the first type-compatible output, persisted on save
- [x] Add Node dialog: search across all `/object_info` types, new nodes get registry defaults (incl.
      `control_after_generate`) and serialise like the web frontend (`properties`, `slot_index`, widget shadows)
- [x] Multi-select: shift-click, rubber band (`Ctrl`+drag), `Ctrl+A`, group drag of the selection; `Delete`,
      `Ctrl+D` duplicate, `Ctrl+B` bypass, `Ctrl+M` mute, `Ctrl+G` group selection
- [x] Groups: create from selection or at a point, drag by the title bar (moves contained nodes), rename,
      8-colour picker, fit to group, delete — all persisted to `groups[]`
- [x] Node **collapse** (`flags.collapsed`), custom titles, all saved back
- [ ] Undo/redo history
- [ ] Copy/paste of nodes, node library sidebar tab (search `/object_info`, drag to insert)
- [ ] Pango text rendering, spatial index for hit-testing, viewport culling

## Phase 4 — Widgets & Execution
- [ ] In-node widgets: number/slider, combo, text/multiline, toggle, seed control (drawn on canvas, GTK popovers for editing)
- [x] Queue Prompt, per-node progress in the log/header
- [ ] Execution highlighting on the canvas (progress events already drive the queue panel), errors overlay
- [ ] Image preview/output nodes, LoadImage upload, gallery panel

## Phase 5 — Performance & Polish
- [ ] GPU path (`gsk` render nodes or snapshot cache per node) if cairo profiling demands it; target 60 fps @ 500 nodes on X11
- [ ] Reroutes, minimap, preferences (persist splash config via GSettings/keyfile)
- [ ] Custom-node frontend extensions: document unsupported cases, graceful fallbacks

## Phase 6 — Packaging & Release
- [ ] Offline Flatpak sources (`flatpak-cargo-generator` → `cargo-sources.json`), AppStream metainfo, icon
- [ ] Meson or `cargo install` + data install for native packages; CI build
