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

## Phase 2 — Backend Protocol
- [ ] Persist splash config (last folder/script/args/host/port)
- [ ] HTTP client (`/object_info`, `/prompt`, `/queue`, `/history`, `/view`, `/upload/image`, `/interrupt`) on a worker thread
- [ ] WebSocket `/ws?clientId=` (status, progress, executing, executed, binary preview frames) → GTK via `async_channel`
- [ ] Node definition registry from `/object_info` (types, inputs, widgets, categories)
- [ ] Queue sidebar: live queue, progress, cancel/interrupt
- Crate candidates: `ureq` or `reqwest` (blocking) + `tungstenite`, `serde_json`

## Phase 3 — Real Graph Editing
- [ ] Load/save ComfyUI workflow JSON (UI format, litegraph `nodes`/`links`/`groups`) and API format export
- [ ] Slot type colors, link creation/removal by dragging between slots, type checking
- [ ] Node add menu/search (from `/object_info`), delete, multi-select, rubber-band, copy/paste
- [ ] Workflows sidebar (user dir + `/userdata` API), undo/redo
- [ ] Pango text rendering, spatial index for hit-testing, viewport culling

## Phase 4 — Widgets & Execution
- [ ] In-node widgets: number/slider, combo, text/multiline, toggle, seed control (drawn on canvas, GTK popovers for editing)
- [ ] Queue Prompt, execution highlighting, per-node progress, errors overlay
- [ ] Image preview/output nodes, LoadImage upload, gallery panel

## Phase 5 — Performance & Polish
- [ ] GPU path (`gsk` render nodes or snapshot cache per node) if cairo profiling demands it; target 60 fps @ 500 nodes on X11
- [ ] Groups, reroutes, minimap, keyboard shortcuts, preferences (persist splash config via GSettings/keyfile)
- [ ] Custom-node frontend extensions: document unsupported cases, graceful fallbacks

## Phase 6 — Packaging & Release
- [ ] Offline Flatpak sources (`flatpak-cargo-generator` → `cargo-sources.json`), AppStream metainfo, icon
- [ ] Meson or `cargo install` + data install for native packages; CI build
