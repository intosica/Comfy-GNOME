# ROADMAP

Each phase ends with a runnable build, updated `AGENT-HANDOFF.md`, and screenshots in `docs/screenshots/`.

## Phase 1 — Shell & Prototype ✅ (session 1)
- [x] Cargo project, GPL-3.0 headers, LICENSE, Flatpak manifest + `.desktop`
- [x] Libadwaita splash: ComfyUI folder (auto-detected), optional launch script, extra args, browser-launch blocking, host/port, "Skip — Connect to Running Backend"
- [x] Folder launch (venv auto-activate) and script launch: own process group, log streamed to panel, killed on close/crash; `flatpak-spawn --host` in Flatpak; main-window ✕ quits the app (hidden Settings/Templates windows no longer keep it alive); subgraph double-click no longer collapses; hover-only edge bar reveals the sidebar; sidebar squeezes down to ~130 px; Flatpak palette/font/icons match native (session 23); Add Node search is a frontend port: link-release type filters, root chips, category tree, native icons (session 24); screenshot protocol uses the ComfyUI_00004 workflow in dark mode (session 25)
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
- [x] Pango text rendering (emoji/box-drawing/CJK, `canvas/text.rs`); [ ] spatial index for hit-testing, viewport culling

## Phase 4 — Widgets & Execution
- [x] In-node widgets: number, combo, text/multiline, toggle, seed control, subgraph promoted widgets, rgthree Power Lora / Fast Groups Bypasser (drawn on canvas, GTK popovers for editing) — baseline: Intosica - Edit
- [x] Queue Prompt, per-node progress in the log/header
- [x] Running-node highlight; [ ] errors overlay
- [x] control_after_generate applied after Queue
- [x] Image preview on nodes (LoadImage, executed outputs), LoadImage upload; [ ] gallery panel
- [x] Subgraph breadcrumb 1:1 (`reference/Subgraph Exit.png`, `Subgraph Rename Action.png`): home/ancestor crumbs, back arrow, Esc, active-crumb menu = Rename + Clear Workflow; Clear inside a subgraph keeps the IO nodes; [ ] crumb overflow menu (collapse to last 3)
- [x] Templates browser (`reference/Templates.png`): categories, Popular, Extensions modules, search, All/Node graph/Apps, thumbnails, open as temporary tab; [ ] sort dropdown, model/task filters, overlay chips, extension thumbnails, left-rail button
- [x] Double-click empty grid → node search; tab right-click menu (+ temp-workflow variant, bookmarks, close guard); core hotkeys (Ctrl+Shift+Enter, Ctrl+Alt+Enter, `.`, Esc, Alt+C) — hotkeys/drag-drop/Save As/Export still need a live click-through
- [x] EreNodes (Prompt Cloud/Gallery/Toggle/MultiSelect/Randomizer): tag pills, ≡/+ menus, Load Tag Group (AIXG), gallery tiles with LoRA preview via `/erenodes/view`; [ ] Randomizer 🎲 polish, add-tag pickers untested

## Phase 4b — Queue history & image viewing ✅ (session 8)
- [x] Finished runs in the queue sidebar (`/history`, day groups, durations, thumbnails), `All | Completed` filter,
      `Clear queue`
- [x] Full-size image lightbox (queue rows, preview panel, pixel-accurate dialog at 90 % of the window)
- [x] "Open Workflow in New Tab" for running, pending and finished jobs (`prompt[3].extra_pnginfo.workflow`)
- [ ] Job-menu parity with the web UI: Export workflow, Inspect asset, Add to current workflow, Copy job ID
- [ ] Queue history pagination (currently the newest 100 entries)

## Phase 4c — Batch count, DaSiWa behaviour, backend lifecycle, LoRA Manager ✅ (session 9)
- [x] Batch count spin (1–100) → N prompt copies per Run; `[run] queued N prompt(s)` in the log
- [x] Backend we started is killed with the app (SIGTERM → 3 s → SIGKILL on the group); a backend we did *not* start is never touched
- [x] DaSiWa: seed panel (`🎲 Random` / `📌 Fixed` + roll, `seed_control_state` + `recent`), status switches (mute/bypass targets, chained), LTX2 LoRA stack rows (toggle/name/strength/➕) synced with `stack_data`
- [x] LoRA Manager in a chrome-less host-browser window (`flatpak-spawn --host` inside the sandbox)
- [ ] DaSiWa node UIs still stock: WildcardPresetPromptBuilder, Minimax H3 director, EnhancedVideoCombine video preview
- [ ] LoRA Manager window is the browser's app mode — no in-app webview (host has no WebKit dev headers)

## Phase 4d — Extension node UIs, Settings, job-workflow cache ✅ (session 10)
- [x] Bypassed-node passthrough drawn under node bodies (dashed wire no longer crosses nodes)
- [x] `easy wildcards`: LoRA/Wildcard pickers populated from the registry, picks appended to the prompt
- [x] `Lora Loader (LoraManager)`: prompt text with `<lora:name:str>` tags as Lora rows + ➕ Add LoRA
- [x] `ShowText|pysssss`: one text row per stored line
- [x] "Open Workflow in New Tab" works for our own jobs from a local `prompt_id → workflow` cache; foreign jobs say why not
- [x] Settings dialog (`Ctrl+,`): search, APPLICATION (Comfy/Appearance), SPECIAL SETTINGS (About), one EXTENSIONS section per
      settings prefix, editors by value type, write-back via `POST /api/settings/{key}`
- [x] ~~Settings labels key-derived~~ → real definitions since session 11 (still no "restore default" button)
- [ ] DaSiWa WildcardPresetPromptBuilder / Minimax director UIs, video-combine preview (stock widgets)

## Phase 4e — Settings parity, prompt editing, queue reopen ✅ (session 11)
- [x] Settings match the web UI: definitions scraped from frontend + extension JS, ACCOUNT / APPLICATION / SPECIAL sections, Keybinding / Extension / About pages
- [x] Undo / redo (Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y), snapshot-based
- [x] Model image preview on hover in any model combo (LoRA Manager → core preview fallback)
- [x] LoRA Manager button opens on the app's display (own browser profile)
- [x] Offline backend: one log line per outage, Run refuses with a clear message, reload on reconnect
- [x] Subgraph instance nodes titled by their definition name
- [x] `easy wildcards(Matrix)` pickers behave exactly like the easy-use JS
- [x] Prompt popover: wide editor, Ctrl+↑/↓ attention weighting (web port), Ctrl+Enter, char count
- [x] Light canvas polish (grid majors, wire casing, node shadow) — no extra GPU work
- [x] Every queue row (pending / running / finished, with or without images) reopens its workflow in a new tab
- [x] Picker combos generalised: any combo whose first option reads "Select … add … text" appends to the node's text widget (easy wildcards/wildcardsMatrix/prompt, Impact & Inspire wildcard encoders, detailer pipes)

## Phase 4f — Public repository ✅ (session 12)
- [x] README rewritten for a public audience: icon header, screenshots, features, requirements, run, hotkeys
- [x] `tools/demo/` mock backend + `start.sh` so the screenshots (and UI work) need no models
- [x] Published as a single clean commit to `intosica/Comfy-GNOME`; the old ComfyUI_frontend fork head kept as
      branch `frontend-fork-backup`. Not published: `AGENT-HANDOFF.md`, `ROADMAP.md`, `reference/`, `.pi/`, old
      screenshots; personal paths scrubbed out of `docs/TRANSLATION-NOTES.md`
- [ ] Screenshots for the settings dialog and the LoRA Manager still come from real runs (mock has no frontend JS)

## Phase 5 — Performance & Polish
- [ ] GPU path (`gsk` render nodes or snapshot cache per node) if cairo profiling demands it; target 60 fps @ 500 nodes on X11
- [ ] Reroutes, minimap, preferences (persist splash config via GSettings/keyfile)
- [ ] Custom-node frontend extensions: document unsupported cases, graceful fallbacks

## Phase 6 — Packaging & Release
- [ ] Offline Flatpak sources (`flatpak-cargo-generator` → `cargo-sources.json`), AppStream metainfo, icon
- [ ] Meson or `cargo install` + data install for native packages; CI build

## Phase 4i — Session 15 polish ✅ (session 15)
- [x] Subgraph inputs added by a dropped wire ("auto" inputs) are removed again once their wire is cut, on the Inputs node and the host
- [x] Generic widget-row context menu (`canvas::widgets::menu` + `Workspace::widget_row_action`); Power Lora rows toggle/move/remove
- [x] Log & Preview can be hidden with the `▼` toggle in the bottom bar; the split position is remembered
- [x] Queue list shows every running and pending job (the old `once(..).zip(..)` chain emitted one row per list)
- [x] `Comfy.LinkRenderMode`: straight / linear / spline / hidden wires, applied live from Settings on all open canvases

## Phase 4h — Widget sockets, node resize, snap to grid ✅ (session 14)
- [x] Widget rows carry the web UI's input socket (faint dot, solid once wired); pulling one starts a wire
- [x] Power Lora Loader rows get `lora_N` sockets; the input is written to the model only when a wire lands
- [x] Node resize grip (bottom-right, min 120×44) and `size` persisted on save
- [x] Snap to grid: `Comfy.SnapToGrid.GridSize` + `pysssss.SnapToGrid`, Shift or always-snap, live from Settings
- [x] Settings window always dismissable (Escape / Ctrl+W / ×) and toggleable from the gear or Ctrl+,

## Phase 4g — Subgraph inputs & singleton dialogs ✅ (session 13)
- [x] Subgraph Inputs node ends in a dashed "plug a new input here" socket; dropping a wire on it appends a
      parameter (named after the node input it feeds, `_2` on clashes) and mirrors it onto every host instance
- [x] The Inputs pseudo-node's trailing socket is rendered outside the node clip so the ring stays whole
- [x] Settings is a singleton: a second Ctrl+, / gear click raises the open window; the handle clears on destroy

## Phase 4j — Node Library, MTB media browser, droppable images ✅ (session 16)
- [x] Bottom panel folds into a thin always-present bar (the `▼` toggle lives there, so it can always be undone)
- [x] Sidebar "Nodes" page: search across display name / node name / category / description, lazy category tree when the query is empty, click to add
- [x] Sidebar "MTB" page: `input`/`output` roots, editable path field with subfolder navigation (subfolders are *not* dropdown entries), Modified/Name sort, thumbnail-size slider, paged loading
- [x] GNOME-style selection: Ctrl+click toggles, Ctrl+drag rubber band selects a rectangle, Ctrl+A selects all, click-off clears
- [x] Right-click: "Show Item Location" via `org.freedesktop.FileManager1.ShowItems` (falls back to opening the folder) and "Delete N Selected…" behind an "Are you sure?" dialog that **moves to the Trash** — never unlinks
- [x] Drag a thumbnail onto the canvas: an image with ComfyUI metadata opens its workflow in a new tab, one without it uploads and lands as a preloaded Load Image node at the drop point
- [x] Full-window Ctrl+A no longer steals focus from search fields
