# AGENT HANDOFF

_Last updated: session 3. Phases 1–2 complete (only the output gallery is still open). Phase 3 is live: workflow tabs, colour-coded links, wire editing (drag / shift-detach / unplug) and saving via `POST /api/userdata`. Next: undo/redo, node add menu, multi-select, node library tab._

Project: **Comfy-GNOME** at `/home/montel/Documents/Comfy-GNOME` (renamed from `GNOME-Comfyu` → `GNOME-ComfyUI` → this).
Crate/binary `comfy-gnome`, app id `io.github.comfygnome.ComfyGnome` (`APP_ID` in `src/main.rs`).

## State
Phase 1 complete. Phase 2 complete except the output gallery and third-party UI widgets (see `ROADMAP.md`).
Phase 3 core editing works: tabs, link colours, wire drag/detach/unplug, save (`Ctrl+S`), viewport persistence.
`cargo build` clean, `cargo test` passes (10 unit tests; 3 `--ignored` live tests against a running backend).
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
Flatpak verified (session 3, release build of the current tree): `flatpak-builder --user --install` succeeds
(`docs/screenshots/flatpak-phase3.png`), the sandboxed app starts on X11, reads the host's saved launch config
(`--filesystem=home`), connects to a running backend on `127.0.0.1:8188` (shared network), loads `/object_info`
(4775 types) and the 159-workflow list, opens a workflow in a tab and **saves it back bit-identically**.
`flatpak-spawn --host` reaches the host venv python from inside the sandbox. Still unverified: launching ComfyUI
*from* the Flatpak when no backend is already running (untested because :8188 was busy), and window-close teardown
in script mode.
Rebuild after code changes: `./launch-flatpak.sh --rebuild` (or the raw `flatpak-builder` line above); it takes
~3–4 min because it does a release build inside the sandbox.
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

## Session 2c — preview panel
- Bottom pane is now an `adw::ViewStack` (`bottom`) with a Narrow `adw::ViewSwitcher`: **Log** and **Preview**.
  Preview = `gtk::Picture` (Contain, can_shrink) + caption label; the image is decoded straight from bytes
  with `gdk::Texture::from_bytes(&glib::Bytes::from_owned(..))` — no image crate, no temp files.
- Two sources: WS binary `Preview` frames (kind 1, header already stripped) and `WsEvent::Executed{node, images}`
  → `Client::view(&OutputImage)` → `/view?filename=&subfolder=&type=`. Arrival auto-switches the tab.
- **Backend quirk: `executed` is only sent to the client_id that queued the prompt** (`send_sync(..., sid)`),
  so previews of prompts started from the browser never reach us. Test previews by queueing from the app
  (Ctrl+Enter / Run), not with curl.
- Verified: `EmptyImage(256) → PreviewImage` test workflow queued from the app renders the white 256×256 image
  with its `ComfyUI_temp_*.png` caption (`docs/screenshots/phase2-preview.png`). The test workflow lived at
  `user/default/workflows/zz-preview-test.json` and was deleted again.
- Bug found while testing: **the workflows reload button was created but never connected** (only `connect_map`
  refreshed the list, so a workflow saved after start-up never appeared). Fixed — check both refresh buttons
  when the list looks stale.
- Bug: the monitor label's width changed every second, shifting every header button (clicks missed). Fixed with
  `width_chars(17)` + `xalign(0)`; rerun the header-jitter check (two screenshots 1 s apart, compare the blue
  Run button's x-range) after touching the header.

## Session 3 — graph editing, tabs, saving
- **Tabs**: `adw::TabView` + `adw::TabBar` (autohide false, expand false) above the canvas pane;
  `docs: Rc<RefCell<Vec<DocRef>>>`, `struct Doc { file, wf, path, canvas, page, dirty }`. `Ctrl+W` closes the
  active tab, `Ctrl+S` saves it, clicking a workflow already open just switches to its tab. Unsaved marker is a
  `•` appended to the tab title by `set_dirty()` — `set_indicator_icon("media-record-symbolic")` rendered
  *nothing* under this icon theme, do not go back to it.
- **Saving**: `graph_view::sync(&mut graph, nodes, links)` writes canvas state back into the model (positions,
  link endpoints, `inputs[i].link`, `outputs[i].links`; unchanged endpoint pairs keep their old link id, new ones
  get `max+1`), `sync_io(sg)` rebuilds subgraph `inputs[].linkIds`, `workflow::to_json()` splices everything into
  the original `raw` JSON (unknown fields survive), `api::save_workflow` POSTs it. Link endpoints are written as
  numbers again via `idv()` (ids that are not integers, e.g. `"39:7"`, stay strings). `extra.ds` is overwritten
  with the live viewport on save, like the web frontend.
- **Round-trip proof**: opened a real 29-node workflow, saved it unchanged, then compared with a structural diff:
  **0 differences**, and `comfyui validate workflow` reports the identical `converted_node_count: 21` plus the same
  single pre-existing error before and after. Same test on a workflow that *has* a subgraph definition: identical.
- **Three save-fidelity bugs found by that test (all fixed — do not regress them):**
  1. serde_json's default float parser is off by one ulp on long mantissas, so every untouched node position was
     silently rewritten (`104.62510134174657` → `…656`). Fixed with the **`float_roundtrip` feature** on serde_json
     (`Cargo.toml`); regression test `workflow::tests::to_json_keeps_coordinates_bit_exact`.
  2. `v["definitions"]["subgraphs"]` *inserts* a null subtree when the key is absent (serde_json `IndexMut`), so a
     workflow without subgraphs grew `"definitions": {"subgraphs": null}`. Use `get_mut(...).and_then(Value::as_array_mut)`.
  3. `sync` rebuilt `outputs[].links` in HashMap order (`[1717,1738]` → `[1738,1717]`) and `write_graph` rewrote
     `links: null` as `[]`. Both now preserve ComfyUI's shape (ids sorted ascending, `null` kept when empty).
  The save path is now *bit-faithful*: open → `Ctrl+S` → identical file. Use that MCP tool
  (`mcp comfyui_validate_workflow`, arg `workflow_path`) as an external oracle — it converts UI→API server-side,
  so it cross-checks `prompt.rs` and anything our save writes.
- **Wires**: `hit_slot`/`hit_target` with radius `9/zoom` canvas units. Drag from an output → new link; drag from a
  connected **input** → the wire keeps hanging from its origin and follows the cursor (unplug; release over nothing
  deletes it). **shift+drag from a connected output** de-anchors the *far* end: the wire keeps hanging from the input
  it fed and hunts for a new output, so shift-dragging a MODEL wire from loader A onto loader B re-sources the wire
  (`links: [[5,5,0,3,0,"MODEL"]]`) instead of merely redrawing it from the same node. Implemented by making
  `LinkDrag` anchor-agnostic (`out: bool`); the drop targets and hover ring use `hit_target(p, r, ty, !out)`.
  In-progress wire is `#aaffaa`; hovering a type-compatible target draws a ring; incompatible slots are ignored.
- **Bug fixed — viewport restore lost**: `set_graph()` set `pending_fit` and the resize callback refit the graph
  once the widget got a size, silently overriding the `extra.ds` viewport restored right after. `set_view()` now
  clears `pending_fit`, so a saved viewport wins and auto-fit only happens when there is no saved view.
- **Bug fixed — sidebar filter hid everything**: on libadwaita 1.5 `AdwActionRow` *is* a `GtkListBoxRow`, so the
  filter's `row.child().downcast::<adw::ActionRow>()` never matched. Use `row.downcast_ref::<adw::ActionRow>()`
  first (see `row_title`). Rows also carry `set_widget_name(file)`; selection reads `widget_name()` because
  `row.index()` shifts while the list is filtered.

### Testing notes added in session 3
- The Xvfb screen is 1600×1000 but screenshots are shown to the agent downscaled — **do not** read click
  coordinates off the rendered picture. Either scan the PNG (helper logic: connected components of the node body
  colour `#353535` → bboxes; slot dots by their type colour) or compute `screen = canvas_origin + ds_offset + pos*zoom`
  after restoring a known `extra.ds`.
- xdotool needs dwell time inside a drag: `xdotool mousemove X Y sleep 0.3 mousedown 1 sleep 0.3 mousemove X2 Y2
  sleep 0.4 mouseup 1`. A single fast chained call registers as a pan. Holding the button across *separate*
  xdotool invocations works, so you can screenshot mid-drag.
- Canvas widget origin is (285, 138) at a 1280×800 window with the tab bar (x includes the 285 px sidebar).
- To validate a saved file with the oracle: `mcp({tool:"comfyui_validate_workflow", args:{workflow_path:"/path.json"}})`.

## Session 4 — node/group editing, context menus, bypass

Ask (m01975): "continue working on the UI, functions and UI functions, for nodes, subgraphs, and groups. Also
'bypass' logic, for bypassing node on right-click (along with other right-click options of course)."

Shipped:
- **Selection model** in `canvas` (`State.sel`): shift-click toggle, plain click keeps or replaces the selection,
  `Ctrl`+drag rubber band, `Ctrl+A`, group drag, `Delete`/`Ctrl+D`/`Ctrl+B`/`Ctrl+M`/`Ctrl+G` shortcuts.
- **Context menus** (node / group / empty canvas) built as plain `gtk::Popover` + buttons — see
  `ARCHITECTURE.md` for why GMenuModel actions are banned here.
- **Bypass (mode 4) and mute (mode 2)** toggles, magenta/dimmed rendering, and dashed passthrough lines from each
  connected input to the first type-compatible output, drawn *after* the node bodies.
- **Collapse** (`flags.collapsed`), custom titles, **duplicate**, **delete with link pruning**, **groups** (create
  from a selection or at a point, move by dragging the title bar, rename, 8-colour picker, fit, delete).
- **Add Node dialog**: search over `/object_info`; new nodes come from `workflow::Node::from_def` with
  `prompt::default_widgets` (defaults + `control_after_generate`) and are written into the file by
  `fresh_node_json` on save.
- **Save path reconciled** — this was the main bug class of the session: data that exists only in the model (new
  nodes, new/deleted groups, `mode`, `title`, `flags`) is now authored by `write_graph` instead of being read back
  from the loaded JSON. Verified: add node → 30 nodes on disk; add group → `groups[].bounding` written; bypass
  inside a subgraph → `definitions.subgraphs[0].nodes[0].mode == 4`; group colour/rename/delete persist.
- Subgraph boundary pseudo-nodes are view-only: edit operations filter them out and their right-click shows the
  canvas menu.

Screenshots: `docs/screenshots/phase4-node-menu.png`, `phase4-bypass.png`, `phase4-add-node.png`.

**Flatpak rebuilt and verified with these features** (`flatpak-builder --user --install --force-clean
--install-deps-from=flathub flatpak-build build-aux/io.github.comfygnome.ComfyGnome.json`, ~1m45s): sandboxed app
connects to the host backend, lists 158 workflows, opens one in a tab, canvas menu + node menu work, and `Enter` on
the focused "Bypass Node" item turns the node magenta with the dashed passthrough. Nothing was saved, so the test
workflow stayed untouched (md5 unchanged). `cargo test` = 10 passed / 3 ignored.

### Testing notes added in session 4
- **The reload button (↻) is at (260, 79)** in the sidebar; the list is fetched on connect and on demand only, so a
  workflow uploaded with `curl` while the app runs is invisible until you press it. The search entry is at
  (130, 73), first row at (130, 120)–(130,140).
- Menu items in the button-based popover are only clickable **on their label text** (the buttons do not expand to
  the row width): e.g. node menu items sit at x≈665-720 and are 34 px apart; the group menu's "Rename Group…" label
  is at (515, 258) while the swatch row is at y≈294.
- Dialog windows (`Add Node`, `Group Title`) open at 0,0 *under* the main window without a WM: find them with
  `xdotool search --name "^Add Node"` / `"^Group Title"`, then `windowraise` + `windowfocus` before typing.
- A click that only focuses the window does not focus a widget: press `windowfocus` first, then click the search
  entry, or `Ctrl+A`/typing goes to the canvas (select-all + no text input).
- Workflow files for tests: upload to `POST /api/userdata/workflows%2Fzz-test.json` (single-segment, percent-encoded
  — the two-segment form is 405), delete with `DELETE` on the same URL (204). Always delete them afterwards;
  `anima_simple_generate.json` is the safe read-only fixture (md5 `0552a9a3f1d07218ba0711b7263c1689`).
- `fit_view` picks a smaller zoom than expected on tall graphs (uses the widget height minus margins); the header
  fit button and `Ctrl`-wheel zoom both work, so it is cosmetic.

## Next steps (Phase 4 → 5)
1. **Undo/redo**: snapshot `(nodes, links, groups)` in `connect_change` (cheap clones) into a `VecDeque` capped at
   ~50; `Ctrl+Z`/`Ctrl+Shift+Z`. Every mutation already funnels through `notify_change`.
2. **Save prompt on close**: `tabs.connect_close_page` → `adw::AlertDialog` (Save / Discard / Cancel) returning
   `Propagation::Stop`; same on window close when any tab is dirty.
3. **In-node widgets** (the big Phase 4 item): draw editable values, open GTK popovers for text/number/combo/seed,
   write back into `widgets_values` in the right order (use `prompt::widget_map` in reverse).
4. **Copy/paste** of nodes (clipboard JSON) and drag-and-drop from a node-library sidebar tab.
5. Execution highlighting: progress events already carry `node`, so tint the running node; show the error overlay.
6. Output gallery (last Phase 2 leftover) and LoadImage upload.
7. Third-party UIs in priority order (see `docs/research/custom-node-uis.md`): DaSiWa in-node monitor, LC Align
   guides, LoRA Manager button.
8. Flatpak rebuild + click-through after the feature work lands (`launch-flatpak.sh --rebuild`).
