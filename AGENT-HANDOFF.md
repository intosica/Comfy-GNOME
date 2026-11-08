# AGENT HANDOFF

_Last updated: session 21. Phases 1–3 complete (output gallery still open). Phase 4 live: in-node widgets, subgraph breadcrumb (exit / rename / clear), Templates browser, EreNodes tag widgets, hotkeys. Session 19 fixed launching from app-menu shortcuts (native + Flatpak); session 20 restored live previews (TAESD/Latent2RGB) and added the thin sampler progress strip under the tabs; session 21 puts those previews on the running node (KSampler, SamplerCustom, custom samplers) — see the last sections. Next: undo/redo, copy/paste, output gallery._

Project: **Comfy-GNOME** at `/home/montel/Documents/Comfy-GNOME` (renamed from `GNOME-Comfyu` → `GNOME-ComfyUI` → this).
Crate/binary `comfy-gnome`, app id `io.github.comfygnome.ComfyGnome` (`APP_ID` in `src/main.rs`).

## Standing rule — kill the backend when you are done

**Every agent must stop the ComfyUI backend it started (and any demo stack it left behind) as the last step of
the task, before reporting back.** A backend left listening on `127.0.0.1:8188` (or a mock on 8198) makes the
director's own tests attach to *your* half-configured server — wrong workflow list, wrong settings, stale models —
and the failures look like app bugs.

Checklist before saying "done":
```bash
ss -ltnp | grep -E ':8188|:8198'          # who is listening?
pgrep -af "main.py|mock_backend"          # backend / mock processes
pgrep -x Xvfb                             # test displays you started
```
Kill what you started (`kill <pid>`; the app also kills its own child in folder mode), stop test `Xvfb`
displays, and remove scratch state (`/tmp/demo-config`, `/tmp/demo-cache`, screenshots in `/tmp`).
Never kill a backend you did not start unless the user asks for it.

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
- **Xvfb screenshots lie about repaints.** Without a window manager GTK4/X11 only redraws damaged regions, so a
  freshly mapped window shows leftover pixels from the *previous* client (an old canvas, the splash). Symptom:
  a feature looks missing in a screenshot while the log proves it ran, and two `xwininfo` toplevels at +0+0 fight
  over the same pixels. Force damage first (toggle the sidebar, `xdotool windowsize`, click where content should
  be) or compare against the same build on a real display; never conclude "the UI is broken" from one frame.
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

## Session 5: in-node widgets (Intosica - Edit baseline)
Baseline workflow: `user/default/workflows/Intosica - Edit.json` (subgraph hosts with legacy `proxyWidgets`, nested
subgraphs, rgthree Power Lora Loader / Fast Groups Bypasser, MarkdownNote, LoadImage, custom Image Save). It now
opens, renders real widgets, edits, saves bit-faithfully (open→save with no edit == identical except `extra.ds`) and
runs through `prompt::build`. Screenshots: `docs/screenshots/phase4-widgets-intosica-edit.png`, `phase4-fast-groups-note.png`.
- `src/canvas/widgets.rs`: `WKind`/`Widget`/`Zone`, cairo drawing, popover editors (number SpinButton, searchable
  combo, text/multiline). Canvas holds a *copy* of each value; edits update it in place and fire
  `Canvas::connect_widget(node_id,key,value,structural)`.
- `canvas/mod.rs`: `Node::relayout()` (rows for plain slots, widget rows, shadowed-input sockets drawn on the widget
  row, multiline widgets flexible, image area), `Drag::Widget` (click = step/toggle/popover, horizontal drag scrubs
  numbers), image cache (`missing_images`/`put_image`/`set_node_images`), `set_running` green outline.
- `graph_view::build_widgets` decides widgets per node; keys: numeric idx = `widgets_values[idx]`;
  `p:<innerId>:<idx>` = promoted subgraph widget (written into the inner node in the subgraph def);
  `lora:<idx>`/`lora+` (Power Lora rows, value is an object, Null = remove); `group:<gi>` (Fast Groups Bypasser,
  sets member nodes' mode 0/4, honours "always one"); `upload:<idx>` (FileDialog → `Client::upload_image`).
  `graph_view::apply_widget` is the single write path (model); structural edits re-render via `edited`.
- `prompt::widget_slots` mirrors `map_widgets` but returns positions + roles (Value/Control/Upload).
- Run: `queue_prompt` syncs canvas→model first, sends `wf.to_json()` as extra_pnginfo, then `bump_controls`
  (control_after_generate increment/decrement/randomize on mode-0 nodes in root + subgraph defs).
- WS `Executed` images (root ids only) → per-doc `outputs` → node preview; LoadImage previews via `/view` query.
- `write_graph` now writes `widgets_values` back, keeps stored group JSON (font_size, int bounds) and only touches
  `flags.collapsed` when it is true→false.
- Limits / TODO: (glyph gap fixed in session 6 via Pango) no
  undo for widget edits; `{...}` object widgets of unknown extensions are read-only; subgraph-input widgets of the
  new-style (host `widgets_values`) path are not yet editable; ComfyUI `validate_workflow` flags rgthree virtual nodes
  as unknown (validator limit, frontend-only nodes); a full Run on the baseline was not executed (heavy models).
- Testing: open the workflow, copy to `zz-cgtest.json` for edits (delete afterwards); close the tab / restart app
  before re-copying, an already open tab keeps its stale in-memory doc.

## Session 6: Pango text, tab menu, hotkeys, EreNodes
- **Text**: `src/canvas/text.rs` = pangocairo FFI (`text::width`, `text::show`); all canvas text goes through it so
  emoji / box-drawing / CJK render (fontconfig fallback). Links `pangocairo-1.0`, `pango-1.0`, `gobject-2.0`.
- **Dbl-click empty grid** → node search dialog (`Canvas::connect_activate(&str,(x,y))`, id `""` = empty space).
- **Tab right-click menu** `src/workspace/tabmenu.rs`, mirrors `reference/Tab Right-Click Action*.png`: Rename,
  Duplicate, Save, Save As, Export, Export (API), Add/Remove Bookmark, Clear Workflow, Close Tab, Close Other/Left/
  Right tabs. A *temporary* workflow (title `temp:N:…`, e.g. a Duplicate or a dropped PNG) shows Rename and Add to
  Bookmarks disabled (frontend: `!isPersisted` / `isTemporary`, `WorkflowTab.vue` + `useWorkflowActionsMenu.ts`).
  Close with unsaved changes → AlertDialog Cancel/Discard/Save. Bookmarks = `workflows/.index.json {"favorites":[…]}`;
  sidebar shows only `.json` and a ★ prefix for bookmarks. PNG drop reads workflow from tEXt (`src/pngmeta.rs`).
- **Shift modifier research** (frontend): Shift on close = skip the unsaved prompt (`warnIfUnsaved: !shiftDown`),
  Run+Shift = queue at front, Ctrl+Shift+V = paste keeping input links, litegraph: Shift-drag = add to selection /
  snap, shift-click on sidebar close turns it red (Close-all). NOT every action honours it.
- **Hotkeys** (frontend core keybindings; implemented ones marked ✔): Ctrl+Enter run ✔, Ctrl+Shift+Enter run at
  front ✔, Ctrl+Alt+Enter interrupt ✔, `.` fit view ✔, Esc exit subgraph ✔, Alt+C collapse ✔, Ctrl+S save ✔,
  Ctrl+O open, Ctrl+G group, Ctrl+B bypass, Ctrl+M mute, Ctrl+A all, `p` pin, Alt+=/Alt+- zoom, `r` refresh defs,
  `w`/`n`/`m`/`a` sidebar tabs, Ctrl+Shift+E convert to subgraph, Alt+Shift+M minimap, Ctrl+Shift+K shortcuts,
  Ctrl+` logs, Alt+M linear mode. **Untested in a real session**: new hotkeys, drag-drop, Save As/Rename/Export.
- **EreNodes** (`src/erenodes.rs`, `src/workspace/ere.rs`, `WKind::Cloud` in widgets.rs): Prompt Cloud / Gallery /
  Toggle / MultiSelect / Randomizer / Multiline. Tags live in `properties._tagDataJSON`; the hidden `text` widget
  is regenerated on every edit (`erenodes::build_text`, group tags resolved via `/erenodes/get_tag_group`). Gallery
  tiles load `/erenodes/view/{type}/{name}` (404 = dark tile). Verified live: pill toggle (tab dirty), ≡ menu
  (Toggle/Remove All, Load Tag Group…, Convert to…), Load Tag Group → AIXG (replaces tags with the group's members,
  as the frontend does). The `undefined.json` entry in the group list is a stray file in the user's tag-group dir.
  Bug fixed this session: widget click offset was computed relative to the node twice (`activate_widget`).
- Flatpak = same source tree, `./launch-flatpak.sh --rebuild`; pango/pangocairo/fontconfig come from the GNOME 50
  runtime, nothing extra in the manifest.

## Session 7: subgraph breadcrumb + Templates browser
Request (session 7): exiting a subgraph was impossible and subgraph Rename/Clear were missing; Templates needed
implementing — both against `reference/Subgraph Exit.png`, `reference/Subgraph Rename Action.png`,
`reference/Templates.png`, `reference/Templates - Sidebar Placement.png`.
- **Breadcrumb** (`Workspace::show_trail`, `src/workspace.rs`): `crumbs: gtk::Box` of flat buttons in the header —
home crumb = workflow filename (click → root graph), one crumb per drilled level, active crumb shows `▾` and opens
the same two-item menu as the web UI (`Rename`, `Clear Workflow`; every other workflow action is root-only there).
Back arrow left of it = `path.pop()`. `Esc` also pops. Rename: `Workspace::rename_subgraph` renames the subgraph def
**and** every host node whose `type` is that subgraph id, in the root graph and all other subgraph graphs (mirrors
`forEachSubgraphNode`). Title/label caps: 18/16/30 chars, ellipsized, so long names cannot squeeze the header.
- **Clear inside a subgraph** keeps the `-10`/`-20` pseudo nodes (frontend: `LGraph.clear` breaks the definition),
clears links+groups; the root keeps the old full clear. `tabmenu::clear_workflow` (now `pub(super)`).
- **Templates browser** `src/workspace/templates.rs`, opened from the header (`folder-templates-symbolic`; the web UI
puts it in the left rail, we have no rail yet): left nav = All / Popular / the 12 index categories / Extensions
(modules from `/api/workflow_templates`), right = search entry + All/Node graph/Apps filter + card grid + count.
Data: `/templates/index.json` (635 templates; `isApp` drives the Apps tab; Popular = `usage` desc), thumbs
`/templates/{name}-1.webp`, workflow `/templates/{name}.json` (fetched with `api::view_query`, parsed by
`Workflow::from_json`, opened as a **temporary** tab titled with the template title).
- GTK lessons: a `FlowBox` swallowed every card click (Grid + `gtk::Image` works); `gtk::Picture` reports the texture's
natural width so cards collapsed to one per row — use `Image::pixel_size(132)`; tooltip windows eat the click that
shows them; a bare `gtk::Window` gets no pointer input without a WM (use `adw::Window` + `app.add_window`).
- Deliberate gaps kept in ROADMAP: sort dropdown, model/task filters, thumbnail overlay chips, thumbnails for
extension templates (their `/api/workflow_templates/...webp` 404s), left-rail placement.
- Verified live (Xvfb :99, real backend): crumbs + back + home, Rename dialog → host nodes retitled, Clear keeps
Inputs/Outputs, Esc exit, Templates nav/search/Apps(13)/Popular/module page, thumbnails, opening a template as a tab.
`Intosica - Edit.json` md5 unchanged (`71bff0542d98891f7ac85676e0d41a18`).
- Flatpak: `./launch-flatpak.sh --rebuild` reinstalled from this tree and the sandboxed build shows the crumb, opens
  the Templates browser (635 cards, thumbnails from the host backend) and honours `.` fit — GTK 4.20 in the GNOME 50
  runtime needed no code change. NB the sandbox cannot reach a backend started by a *previous* app instance: those
  get PDEATHSIG'd, so start `venv/bin/python main.py --port 8188` yourself before testing.

## Session 8: queue history, image lightbox, job menus
Request (session 8): the queue has to keep finished runs (`reference/Queue Logic.png`) and clicking an image in the
queue or the preview panel must show it at full size without covering the whole screen (`reference/Image Preview.png`).
- **Queue sidebar now lists completed jobs.** `Workspace::load_history` reads `GET /history?max_items=100` on connect
  (so finished runs survive an app restart — the web UI only keeps the session's), `Workspace::note_finished` appends
  the entry when the `executing:null` event arrives (re-reads the whole list if the server has not written it yet,
  500 ms retry) and the list is grouped by day: `Today` / `Yesterday` / `%e %b %Y` (`day_label`, `glib::DateTime`).
  A row = 44 px thumbnail (`gtk::Image`, cached in `Workspace::thumbs` by `/view` query), the first image's filename,
  and the wall time (`execution_start` → `execution_success`, `HistoryEntry::duration`), e.g. `104.69s`.
- `All | Completed` filter tabs (linked `ToggleButton`s — `Workspace::queue_done`) and a `Clear queue` button
  (`client.clear_queue()` = `POST /queue {"clear":true}`) replace the old standalone Interrupt button; interrupt now
  lives on the running row and in its `⋮` menu.
- **Image lightbox** `Workspace::show_lightbox(&[(caption, "/view" query)], start)`: `adw::Dialog` sized to 90 % of the
  window (dimmed workspace stays visible), a `gtk::Picture` with `ContentFit::Contain` at the file's real resolution,
  `‹`/`›` circular OSD buttons (hidden for a single image), ←/→/PgUp/PgDn keys, `Esc`/✕ to close, caption
  `name · 1/3`. A generation counter drops decodes that finish after the user already flipped.
- Opened from three places: clicking a finished queue row (its images), clicking the Preview panel picture
  (`preview_items`, filled by the `executed` event — a *live* preview frame has no filename, so it enlarges nothing),
  and "Open Image at Full Size" in a job menu.
- **Job menu** (`⋮` on every row): "Open Workflow in New Tab" for running, pending **and** finished jobs —
  `api::Client::job_workflow` pulls `prompt[3].extra_pnginfo.workflow` out of `/history/{id}`, falling back to
  `/queue` while the job has not finished, then opens it with `open_temp` as `Job <id8>`. Finished rows also offer
  "Open Image at Full Size"; running/pending rows offer Interrupt / Remove from queue.
- Bug fixed on the way: the "Queue is empty" placeholder used to appear above finished runs (it now only shows when
  there is nothing at all, and says "No finished runs yet" in the Completed view).
- Verified live (Xvfb :99, real backend): history list with thumbnails + durations + `Today` header, live append of a
  fresh run (0.87 s), row click → lightbox, preview-panel click → lightbox, `⋮` → job menu → new tab `Job f4b5563e`
  with the queued graph. `cargo test --release` = 18 passed / 3 ignored (new `api::tests::parses_history_entry`).
  `Intosica - Edit.json` md5 unchanged (`71bff0542d98891f7ac85676e0d41a18`), no `zz-*` workflow left behind.
- GTK quirk (new): a linked pair of `ToggleButton`s (`set_group`) does change its state when clicked — the click and
  the handler fire — but on X11 the theme's *checked* background does not repaint. Both tabs looked "off" while the
  filter had already switched. Fix: carry the state visually ourselves (`add_css_class("accent")` on the active tab,
  removed on the other) instead of trusting the checked style.
- Flatpak parity: `./launch-flatpak.sh --rebuild` then `flatpak run` on `DISPLAY=:99` (kill the host instance first,
  it is single-instance over D-Bus) — the queue sidebar lists the same history with thumbnails and durations, the tab
  accent follows the filter, and clicking a row opens the same lightbox. No sandbox-specific work was needed.
- Gaps (kept in ROADMAP): job-menu entries beyond open-workflow/open-image (Export workflow, Inspect asset, Add to
  current workflow, Copy job ID), history capped at 100 entries, and the pending-row menu was only seen rendered —
  the 2-node `zz-preview` test jobs live ~10 ms, too short to click.

## Session 9: batch count, DaSiWa parity, backend lifecycle, LoRA Manager

- **Backend lifecycle** (`src/backend.rs`, `src/main.rs`): the app used to `kill -TERM` the process *group* and return; if
  the runner ignored SIGTERM it survived and the next launch happily talked to it, which looked like "my history came back".
  `Backend::stop_hard()` now SIGTERMs the group, polls `kill -0 -- -pgid` for 3 s and SIGKILLs the group, and the app passes it
  through `stop_backend()` from both `close-request` and `app.connect_shutdown()`. When the child dies at once the log says the
  URL is served by an already-running backend — an app that connects to someone else's server never kills it (verified: the
  user's `comfy launch` instance survived our exit).
- **Batch count**: `gtk::SpinButton` (1–100) next to Run, the same range as the web UI's `Comfy.QueueButton.BatchCountLimit`;
  `queue_prompt` builds and queues that many copies in one request loop and logs `[run] queued N prompt(s): …`. Each copy reruns
  the DaSiWa queue hooks, so a Random seed node gets N different seeds. Verified end-to-end: 2 copies → 2 `/history` entries.
- **DaSiWa nodes** (`src/dasiwa.rs` + widgets): behaviour is applied at queue time the way the pack's JS does it — status
  switches mute/bypass everything wired into `target_XX` (3 passes, so chained switches settle) and `seed_control_state`'s
  Random mode rolls a fresh 64-bit seed into `seed_value` with a 10-entry `recent` list. In the node: the raw state string is
  replaced by a `seed mode` row (`🎲 Random` / `📌 Fixed`, click the row to switch, `🎲` on the right to roll now — the roll writes
  `seed_value` immediately), and `DaSiWa_LTX2LoraLoader` shows `stack_data` as Power-Lora-style rows (toggle, name, strength,
  ➕ Add LoRA) while the hidden string and `properties.stack_data` stay in sync. The system monitor already fed the header
  readout (`dasiwa.system_monitor`).
- **Unknown JS UIs** (WildcardPresetPromptBuilder, Minimax H3 director, enhanced video combine) fall back to stock widgets — a
  documented gap, not a crash: `graph_view::build_widgets` only special-cases the two nodes above.
- **LoRA Manager**: `comfyui-lora-manager` serves a normal HTML app at `/loras`; there is no WebKit dev package on the host, so
  the header's puzzle button opens it in the *host* browser's app mode (`thorium-browser --app=URL --new-window`, then
  chromium/chrome/brave/edge, then firefox, then `xdg-open`; inside Flatpak everything is prefixed with `flatpak-spawn --host`).
  Result is a chrome-less window showing only the LoRA Manager page. Its rounded corners are the *browser's* window
  decoration, so they follow the host's window manager — we can only choose the window, not its shape (no webview dependency:
  the host has `libwebkitgtk-6.0.so.4` but no dev headers, so `webkit6` cannot be compiled natively).
- Testing notes: `xdotool windowclose` bypasses `close-request` (it destroys the window), so window-close teardown must be
  exercised under a WM — `openbox` on the Xvfb display, then click the CSD ✕. Clicking before the window is mapped silently
  does nothing. Also: closing a tab left the sidebar row selected, and `row-selected` only fires on a *change* — subsequent
  clicks were inert; `page_detached` now clears the selection.
- `cargo test --release` = 23 passed / 3 ignored (new `dasiwa::tests::ltx2_stack_rows_round_trip`), `Intosica - Edit.json`
  md5 unchanged, no `zz-*` workflow left behind.

## Session 10: extension node UIs, Settings, job-workflow cache

Goal: the pieces of the reference workflow (`reference/AIXG-MOSS-18-28-52_1.png`, extracted to `zz-moss.json`) that our
UI got wrong — the Easy-Use wildcard node, the LoRA Manager loader, pysssss text output, the dashed bypass line — plus a
Settings dialog that lists every extension that registered settings.

- **Dashed bypass wire over node bodies** (`src/canvas/mod.rs`): the mode-4 passthrough pass was drawn *after* the node loop,
  so a bypassed node's line painted on top of whatever it crossed. It now runs with the links, before the nodes, and the
  comment says why ("drawn later they'd show through the node bodies").
- **Easy-Use `easy wildcards`**: the node ships two combos whose server-side `options` are empty — the JS fills them and the
  *saved value* is the placeholder string (`Select to add LoRA`). `build_widgets` now prefers the registry's `options` whenever
  the stored value is that placeholder, and `apply_widget` appends a pick into the prompt instead of replacing the combo value:
  LoRA → `<lora:NAME:1>`, wildcard → `__token__`. 1000+ LoRA options load fine; the combo popover builds a searchable list.
- **`Lora Loader (LoraManager)`**: `widgets_values` here is the array `[state dict, text, parsed list]`, so the old
  `ends_with("(LoraManager)")` → read-only shortcut showed raw JSON. Now: the text is a multiline row (the `<lora:name:str>`
  tags are the truth), each tag is a `WKind::Lora` row (`lm:{i}`) and `➕ Add LoRA` (`lm+`) inserts a tag; `lm_tags` /
  `lm_set_tag` keep the string and the rows in sync. The parsed-list widget stays untouched (the server ignores it).
- **pysssss**: `ShowText|pysssss` stores a *list of lines*; one text row per line (`show:{i}`) instead of `["Swiss,"]`.
  `SaveText` / `PlaySound` need no interpretation (plain widgets).
- **"Open Workflow in New Tab"** for our own jobs no longer depends on the server echoing `extra_pnginfo`: `queue_prompt`
  caches `prompt_id → workflow JSON` in `Workspace::job_wf` (pruned wholesale past 200 entries) and `open_job_workflow`
  consults it before `/history`. A job queued by another client (MCP, the web UI) still has no graph — the log says so
  explicitly now: "carries no embedded workflow (queued by another client)".
- **Settings** (`src/workspace/settings.rs`): gear button in the header (`Ctrl+,`) opens an `adw::Window` with a
  `navigation-sidebar` ListBox (search box, APPLICATION → Comfy/Appearance, SPECIAL SETTINGS → About, EXTENSIONS → one row per
  key prefix) and a content pane that renders each `/api/settings` entry by value type: bool → switch, number → spin (ints step
  1, floats 0.01), string → entry, everything else → read-only. Writes go through `POST /api/settings/{key}` in a worker thread
  and the footer reports "Saved …" / "Could not save …". Search matches keys across all pages and renders a "Matches" page.
  `/api/settings` carries values only (the definitions live in the web frontend's JS), so the layout is derived from key
  prefixes — that is why the extension rows are exactly the packs that registered settings, and not all installed packs.
- Two API traps: `adw::Window` rejects `gtk_window_set_child()` (`set_content(Some(&view))` with `adw::prelude::*`), and a
  hand-rolled close button duplicates the HeaderBar's own controls.
- **Tooltip-eats-click** struck again: a `tooltip_text` on the queue row's `⋮` button put a tooltip window over the very menu
  it opened, so menu clicks were silently swallowed. Buttons that open a `Popover` now carry no tooltip; when testing, move the
  pointer away for a second after opening a menu before clicking an item.
- `cargo test --release` = 26 passed / 3 ignored (`lora_manager_tags_round_trip`, `showtext_lines_flatten`,
  `wildcards_pick_appends_to_prompt` added). Verified on the real backend: settings read/toggle/write-back (the switch flipped
  `pysssss.SnapToGrid` on the server), search, Appearance, the Easy-Use wildcard list with real tokens, LoRA Manager node rows,
  `ShowText` lines, and "Open Workflow in New Tab" opening `Job 82601b18` from the cache.
- Still stock widgets (documented gaps): `DaSiWa_WildcardPresetPromptBuilder`, `minimax_h3_director`, video-combine preview.

## Session 11: settings parity, wildcards, subgraph titles, prompt editor, queue reopen

User report: connection-refused spam while ComfyUI was down, Settings not matching `reference/Settings - 1.png` / `- 2.png`,
`easy wildcards` still wrong, subgraph nodes titled with "random numbers", prompt typing awkward, light polish without GPU
cost, and every queued job (pending / running / finished) must reopen in a new tab.

- **Offline backend**: `Workspace::online` (set by `on_connected` / ws `Connected`, cleared by `Disconnected`). The ws thread
  still retries every 2 s but only the *first* drop is logged ("lost connection … reconnecting…", title subtitle
  "Disconnected · waiting for …"); Run says "backend … is offline" instead of posting; queue / workflow polls are skipped
  while offline. On reconnect the registry, history and workflow list reload (the backend may have restarted with new packs).
- **Settings rebuilt to match the web UI** (`src/settings_defs.rs` new, `src/workspace/settings.rs` rewritten, delegated):
  `/api/settings` has values only, so definitions are scraped at runtime (worker thread, cached per process) from the served
  frontend's chunks (index.html → `assets/index-*.js` → referenced chunks, e.g. `GraphView-*.js`) and from every `/extensions`
  JS file with a tolerant tokenizer (object literals with `id` + `name`/`type`/`category`; `const X = "…"` categories
  resolved; `registerExtension({name})` collected). Category = `category[0] || id.split('.')[0]`, section = `[1]` — same as
  `useSettingUI.ts`. Live: 262 definitions, 399 extensions, ~3 s. Sidebar: ACCOUNT (User → opens platform.comfy.org in the
  browser), APPLICATION SETTINGS (Comfy, Lite Graph, Appearance, Mask Editor, then extension categories in `/extensions`
  order — the web's order depends on async registration), SPECIAL SETTINGS (Keybinding = our hotkeys, read-only; Extension =
  switches editing `Comfy.Extension.Disabled`; About). Editors by definition type (boolean/number/slider/combo/text/color);
  custom-function widgets are read-only. The dialog opens instantly with "Loading…". 7 Adwaita icons added to the gresource.
  Live test: `cargo test settings_defs -- --ignored --nocapture`.
- **Subgraph titles** (`graph_view::scene`): instance nodes have the definition uuid as `type` and usually `title: null`;
  the title now falls back to `wf.subgraphs[ty].name` before the registry display name / raw type.
- **`easy wildcards` / `easy wildcardsMatrix`** (`graph_view::wildcards_append`, `is_wildcards`): now exactly the easy-use JS —
  LoRA pick appends `<lora:NAME>` (".safetensors" stripped, no separator, no strength), wildcard pick appends `, token`
  (comma only when the text is non-empty). The two picker combos are `structural`, so the canvas re-renders from the model:
  the combo snaps back to its placeholder and the text row shows the new prompt immediately (before, the canvas kept the
  picked value and a later `sync_doc` could write it back).
- **Prompt editor** (`canvas::widgets::edit_text`, new `src/canvas/attention.rs`): multiline popover is as wide as the widget
  (420–900 px), margins, 11.5 pt text, char counter and a hint line. **Ctrl+↑/↓** = web `Comfy.EditAttention` port
  (word under cursor / selection / enclosing parens → `(text:1.05)`, weight 1 unwraps, time-like `12:30` is not a weight);
  the edit is a single undoable user action (delete+insert, `set_text` would wipe undo). **Ctrl+Enter** closes; closing in
  any way commits (as before). Tested in `attention::tests`.
- **Picker combos, blanket rule** (`graph_view::picker_placeholder` / `picker_append`, replaces the wildcards-only code):
  a `Combo` with >1 option whose first option contains "select", "add" and "text" is a picker. Its widget key is
  `pick:{idx}:{text_idx}:{placeholder}` (text = STRING widget named `text`, else the first STRING), structural. A pick
  appends to the text — `<lora:NAME>` when the placeholder mentions LoRA (".safetensors" dropped, no separator), else
  `, pick` (no separator if empty / already ending in `,`/space/newline) — and resets the combo to its placeholder.
  Matches 12 installed nodes: easy wildcards / wildcardsMatrix / prompt, ImpactWildcardEncode, WildcardEncode //Inspire,
  MakeBasicPipe //Inspire, (Basic)ToDetailerPipe(SDXL), EditDetailerPipe(SDXL). Known gap: Impact's wildcard list is filled
  by its JS (`/impact/wildcards/list`), the server combo only has the placeholder, so that picker isn't offered;
  easy prompt's JS joins prefix/subject/background with " " — we use ", ".
- **Undo / redo** (`Workspace::checkpoint` / `undo`, `Doc.{undo,redo,snap}`): whole-workflow JSON snapshots (viewport
  `extra.ds` stripped), cap 50, like the web `ChangeTracker`. Single hook: every edit path already ends in
  `set_dirty(doc, true)`, which now syncs canvas→model and pushes a step if the snapshot changed. Ctrl+Z, Ctrl+Shift+Z /
  Ctrl+Y on the workspace `ShortcutController` (bubble phase, so the prompt popover's own text undo still wins while it
  has focus). Restoring keeps the viewport and trims the subgraph path if a drilled-in subgraph vanished. Verified: node
  move → Ctrl+Z → back → Ctrl+Shift+Z → moved again.
- **Model previews on hover, every combo** (`src/model_preview.rs`, `canvas::widgets::set_preview_provider`,
  `workspace::install_model_previews`): no per-node code. A combo whose options look like model files (.safetensors/
  .ckpt/.pt/.pth/.bin/.gguf/.sft/.onnx) reserves a 256 px picture column (resizing an open popover dismisses it);
  hovering an option asks the provider. Provider = index of `/experiment/models` + `/experiment/models/{folder}` (loaded
  on first hover, ~2 s, keyed by name and stem), then LoRA Manager `/api/lm/{loras|checkpoints|embeddings}/preview-url`
  if present (videos skipped), else core `/experiment/models/preview/{folder}/{idx}/{name}`. Textures cached per option,
  concurrent requests coalesced. Verified on a checkpoint list.
- **LoRA Manager button**: Chromium hands a URL to an already-running instance of the same profile, which opened it on
  *that* instance's display (a stray Thorium on a test Xvfb swallowed every window). Now launched with its own profile per
  display (`~/.cache/comfy-gnome/browser-<display>`). Verified on the app's display.
- Testing gotcha: `pkill -f "Xvfb :98"` inside a bash command kills that bash too (its own command line matches).
- `cargo test` = 31 passed / 4 ignored.
- **Polish (all static cairo/CSS, no animation or blur)**: grid with brighter every-5th lines, a dark 5 px casing under wires,
  one offset translucent drop shadow per node, title separator line, titles ellipsized to the node width, brighter title
  text; `load_css()` in `workspace.rs` (one `CssProvider`), workflow list uses `navigation-sidebar` rows.
- **Queue → new tab**: Running/Pending rows are activatable ("Open workflow in a new tab"), finished runs without images are
  listed too ("Job xxxxxxxx", click opens the workflow; image runs still open the lightbox, ⋯ menu has both). Bug fixed in
  `api::Client::job_workflow`: `/queue` entries are `[number, id, prompt, extra_data, outputs]` themselves (only history
  nests them under `prompt`), so the pending/running lookup read `e["prompt"][3]` and never found anything.
  Verified: a `PrimitiveString → PreviewAny` job posted with `extra_pnginfo` showed as "Job 7143e8ed" and opened in a tab.

## Next steps (Phase 4 → 5)
1. **Undo/redo**: snapshot `(nodes, links, groups)` in `connect_change` (cheap clones) into a `VecDeque` capped at
   ~50; `Ctrl+Z`/`Ctrl+Shift+Z`. Every mutation already funnels through `notify_change`.
2. **Save prompt on close**: `tabs.connect_close_page` → `adw::AlertDialog` (Save / Discard / Cancel) returning
   `Propagation::Stop`; same on window close when any tab is dirty.
3. ~~In-node widgets~~ done in session 5; ~~Pango text~~ done in session 6. Remaining: widget edits in undo/redo,
   new-style subgraph host widgets (host `widgets_values` path).
4. **Copy/paste** of nodes (clipboard JSON) and drag-and-drop from a node-library sidebar tab.
5. Error overlay (running-node outline done in session 5).
6. Output gallery panel (node previews + LoadImage upload done).
7. Third-party UIs in priority order (see `docs/research/custom-node-uis.md`): DaSiWa in-node monitor, LC Align
   guides, LoRA Manager button.
8. Flatpak rebuild + click-through after the feature work lands (`launch-flatpak.sh --rebuild`) — rebuilt in
   session 7 after the breadcrumb/templates work (verify the Templates dialog + crumbs inside the sandbox).

## Session 12 — published the repository
- `README.md` rewritten: centred icon (`data/icons/hicolor/scalable/apps/io.github.comfygnome.ComfyGnome.svg`),
  `docs/screenshots/graph.png` + `queue.png` hero shots, what-it-does list, requirements, launch scripts, hotkeys,
  dev layout, licence. No mention of `ROADMAP.md`/`AGENT-HANDOFF.md` any more.
- `tools/demo/mock_backend.py` (fake ComfyUI: `/object_info`, `/userdata`, `/queue`, `/history`, `/api/settings`,
  gradient PNGs from a stdlib encoder, a real RFC6455 `/ws` handshake so the header says "Connected") and
  `tools/demo/start.sh` (Xvfb :96 1280x800 + mock on 8198 + app, clicks "Skip"). Screenshots are produced from it,
  so no model weights are involved.
  - Mock gotchas: a subgraph host node needs `type == <subgraph id>` (`b1f4c2a7-5e3d-4a9c-9b21-77d0c15aa3e1`) to be
    drillable, and `wf["definitions"]["subgraphs"][i]` must hold `nodes`/`links`/`groups` **inline** — `parse_graph`
    reads the definition object itself.
- Published: pushed the old remote head to `frontend-fork-backup`, then force-pushed a single orphan commit
  (`b2b11e2`, 57 files) to `main` of https://github.com/intosica/Comfy-GNOME with repo description + topics.
  Orphan commit built without touching the working tree (`GIT_INDEX_FILE=/tmp/…; git read-tree --empty; git add -A --
  <paths>; git write-tree; git commit-tree`), so local `master` history stays intact.
- Not published: `AGENT-HANDOFF.md`, `ROADMAP.md`, `.pi/`, `reference/`, `flatpak-build/`, old
  `docs/screenshots/phase*`, `docs/research/`. `/home/montel/...` paths scrubbed from `docs/TRANSLATION-NOTES.md`.
- UI bug found while shooting the subgraph screenshot: in a drilled-in subgraph the window content is drawn ~44 px
  too high (title bar clipped, black strip at the bottom) on an 800 px-tall Xvfb screen without a WM — the extra
  breadcrumb row raises the minimum height. Not reproduced on the real session; worth a look if it shows up there.


## Session 13 — subgraph inputs + settings singleton
- `graph_view::scene` appends one bare socket (`Slot { label: "", ty: "" }`) to the Inputs pseudo-node; `compatible()`
  already accepts an empty type against anything, so a wire can be dropped there.
- `graph_view::grow_inputs(wf, id)` runs from `sync_doc` (subgraph branch, after `sync`): every boundary link whose
  `origin_slot` is past `inputs.len()` becomes a `SubgraphIo` named after the target node input it feeds (deduped
  `_2`, `_3`, …), the link's `ty` is filled in, and the same `Input` is pushed onto every node whose `ty == id`
  (root and every definition). `sync_io` runs after, so `linkIds` agree.
- The canvas change callback compares the subgraph input count before/after `set_dirty` and re-renders when it grew —
  that is what makes the new row and the moved add-socket appear immediately.
- Canvas draws the add-socket as a dashed ring *after* `cr.restore()` (radius 6.5, `[3.5, 3.0]` dash) so the node clip
  does not cut it in half; normal sockets keep their filled 5 px dot.
- Settings singleton: `thread_local! { static OPEN: RefCell<Option<glib::WeakRef<adw::Window>>> }` in
  `src/workspace/settings.rs`; `open()` presents the existing window and returns, and stores the weak ref after
  `win.present()`. Verified on Xvfb: two gear clicks → one window, close → handle clears, reopen works.
- Verified end to end against the demo mock: drill into "Face Detailer", drag from the add-ring to KSampler's
  `positive`, and the Inputs node grows a `positive` row plus a wire; the host node shows `image / model / positive`.
- Published these four files with a second commit on top of `b2b11e2` (same alternate-index technique, parent =
  `origin/main`).

## Session 14 — widget sockets, resize, snap to grid
- Every widget row now shows the socket the web UI draws there (`reference/Node Parameter Output.png`): the canvas
  no longer hides shadowed sockets, it draws them faint (3.2 px, 60 % alpha) until a wire lands on them.
- Files only store a widget's input once converted, so `graph_view::scene` appends a `Slot` for every declared
  widget input the node lacks, maps each widget to its socket by label, then `prune_extra_slots` drops the ones a
  special-cased builder never drew (keeps indices consistent).
- rgthree Power Lora rows have no declared inputs (JS-built): `grow_widget_slots` adds a `lora_N` socket per row,
  named the way `power_lora_loader.py` expects. `sync` materialises the real `Input` **only for wired slots**, so
  files stay clean until the user actually links a row.
- New gesture: dragging an unconnected **input** socket starts a wire that looks for an output
  (`LinkDrag { out: false }`), which is what makes the widget dots useful — previously it fell through to a node drag.
- Node resizing: 12 px grip at the body's bottom-right corner (`Drag::Resize`, min 120×44, `relayout` re-grows the
  height); `write_graph` persists `size` when it changed. Grip colour follows selection.
- Snap to grid: `Comfy.SnapToGrid.GridSize` + `pysssss.SnapToGrid` are fetched on connect, mirrored onto every canvas
  (`Workspace::set_snap`, also applied to new tabs) and re-applied when edited in Settings. Snapping happens on drag
  end / resize for Shift or always-snap, using litegraph's `snapPoint` rounding. Verified on the demo: a 40 px drag
  moved 40 px un-snapped, 36 px snapped (463 → 460 canvas units at grid 20, zoom 1.27).
- Settings window can always be dismissed: Escape and Ctrl+W (capture phase, so the search entry can't eat Escape),
  the title-bar ×, and the gear / Ctrl+, toggle. It is **hidden, not destroyed** — a destroyed window cannot be
  presented again because the signal handlers keep `Dlg` (and thus the window object) alive.
- Tests: `canvas::tests::widget_dot_starts_a_wire_that_lands_on_an_output`, `snap_rounds_to_the_grid_and_zero_keeps_the_value`,
  `graph_view::tests::lora_rows_get_sockets_and_a_wire_materialises_an_input` (35 total).
- Demo mock gained a Power Lora Loader node in `harbour_sdxl` and serves both snap settings, so the screenshots and
  manual checks show the feature.

## Session 15 — auto subgraph inputs, widget row menus, queue fix, link render mode

User reports: subgraph parameters lingered after disconnecting, PowerLoraLoader lost its right-click menu, Log/Preview needed a
collapse button, "queueing more than 2 images only queues 2", and the Link Render Mode setting was missing.

- **Auto inputs** (`src/graph_view.rs`): `grow_inputs` records what it invented in `Subgraph::auto`; new `prune_inputs` deletes those
  whose wires are gone, shifts boundary-link `origin_slot`s and drops the mirrored host inputs. `sync_doc` order is
  `sync` → `prune_inputs` → `sync_io`, and `connect_change` re-renders on any input-count change.
- **Widget row menus**: `canvas::widgets::menu` + `Action`, `canvas::Hit::Widget(id, row)`, `Workspace::widget_row_action`. Power Lora
  rows now offer rgthree's Toggle / Move Up / Move Down / Remove. A new widget kind only answers `menu`.
- **Queue**: `set_queue` used `once(true).zip(&q.running).chain(once(false).zip(&q.pending))`, so at most one running and one pending
  row could exist. Now `running.iter().map(|i| (true, i)).chain(pending.iter().map(|i| (false, i)))`; verified with 5 prompts on the
  real backend (1 running + 4 pending).
- **Log/Preview**: `pan-down-symbolic` toggle in the bottom bar hides the dock and remembers the paned position.
- **Link Render Mode** (`Comfy.LinkRenderMode`): `canvas::link_path(mode, …)` for straight/linear/spline, whole link pass skipped for
  `-1`; mirrored from `/api/settings` on connect, applied live by the Settings dialog. All four modes verified against the real backend.
- **Settings scraper**: combo options can be `[{value:Y.STRAIGHT_LINK, text:'Straight'}, …]` (imported alias), so member names resolve
  through a small enum table; object-form `options: {key: label}` keeps the key as the value.
- **"launch-system.sh only opens Settings"**: a second launch of a running instance is a GApplication `activate`, and
  `build_window` used to present `app.active_window()` — which is whichever window got focus last, i.e. the Settings dialog
  (it is registered with the application too). `src/main.rs` now keeps weak refs to the workspace window and `Workspace` in
  `thread_local! { MAIN, WS }`, and activation hides the dialogs (`Workspace::hide_dialogs` → `settings::close_open`) and
  presents the workspace. Both launch scripts exec the same binary, so flatpak behaves the same.
- Demo stack: launch with `DBUS_SESSION_BUS_ADDRESS=disabled:` or the user's own release build steals the GApplication name and the new
  instance exits silently. Mock gained a Power Lora node and now answers `/queue` with the real array shape.
- Tests: 36 passing, 4 ignored (new: `settings_defs::tests::combo_options_resolve_imported_enums`, `graph_view::tests::grow_inputs_adds_slot_and_mirrors_host`,
  `graph_view::tests::lora_rows_get_sockets_and_a_wire_materialises_an_input`, `canvas::tests::widget_dot_starts_a_wire_that_lands_on_an_output`,
  `canvas::tests::snap_rounds_to_the_grid_and_zero_keeps_the_value`).

## Session 16 — Node Library, MTB media browser, droppable images

- `src/workspace/library.rs` (new): the sidebar "Nodes" page — search across `display_name`, node name, category
  and description; empty query shows one lazy `GtkExpander` per top-level category; `add_node` parks the node in the
  middle of the viewport (or at `(120, 120)` for a brand-new tab, whose area is still unallocated).
- `src/workspace/media.rs` (new): the MTB browser. `Media` owns the root ("input"/"output"), the subfolder path, a
  `gio::ListStore` of absolute paths, a `GtkFlowBox` grid, a thumbnail-size slider (56–260 px), a sort combo and a
  texture cache keyed by path with a generation counter so late decodes never paint into recycled cells.
- Trash: `gio::File::trash()` **permanently deleted files here** (no portal, no session bus). The panel writes the
  freedesktop trash by hand instead, and because `output/` lives on another filesystem than `~/.local/share/Trash`
  (EXDEV) it falls back from `rename` to `copy` + `remove_file`. Verified with four fresh generations: all four
  landed in `Trash/files` with correct `.trashinfo` and the grid reloaded without them.
- Bottom panel: the toggled handler now hides only the switcher and the page, never the bar, so the `▼` button that
  collapses Log/Preview is always on screen (`reference/Toggle Bottom Panel.png`).
- Dropping a picture: `tabmenu::open_dropped` reads the PNG's `workflow` chunk; without one it uploads the file and
  inserts a `LoadImage` node already pointing at the upload (`Workspace::insert_image_node_at`). `doc_or_blank`
  opens an empty workflow when nothing is open, so a drop on an empty workspace works too.
- Two GTK lessons worth remembering:
  1. A `GtkGestureDrag` that begins **claims the sequence and cancels every gesture and `DragSource` below it**,
     even if it is later denied; `GtkScrolledWindow` steals drags the same way (kinetic scrolling). The panel now
     uses one `GtkEventControllerLegacy` in the capture phase that never claims, so FlowBox clicks, Ctrl+click and
     drag-to-canvas all coexist.
  2. Raw `GdkEvent` coordinates are **surface** coordinates. Hit tests must translate them into the panel's
     overlay first (`compute_bounds(&root)`), then into the flow box — otherwise every drop lands one row off.
- `GtkFlowBox` cells stretch to the full pane height unless the box is `valign(Start)`; before that fix there was no
  empty space anywhere in the panel, so a rubber band could never start.
- Sidebar got `min_sidebar_width(268)`: with four stacked pages the switcher was clipped and the last tab became
  unclickable.

## Session 17 — upstream-alignment fixes (completed + live-tested in session 18)

User asked to fix 15 items against `ComfyUI_frontend-1.60.0`. Session 17 stopped mid-work; session 18 wired the
Reaper, compiled, and live-tested everything against the real backend on Xvfb (see Session 18 below).

### Finished
1. **Nodes icon** — new `data/icons/scalable/actions/node-graph-symbolic.svg` (three rounded node boxes + wires),
   registered in `data/resources.gresource.xml`; Nodes page uses it (`src/workspace.rs:748`).
2. **"easy wildcards" name** — root cause: V3 `/object_info` reports `display_name: null` for that node, so
   `NODE_DISPLAY_NAME_MAPPINGS` is ignored. `Registry::apply_display_names` (`src/registry.rs`) now overlays
   `/i18n → <lang> → nodeDefs.<id>.display_name`; `load_registry` fetches `/object_info` + `/i18n` in one worker.
   The panel shows **Wildcards**.
3. **MTB selection/drag** — `src/workspace/media.rs` rewritten: `SelectionMode::None`, own selection state
   (plain replace, Ctrl toggle, Shift range, Ctrl+A), thumbnail drag drops a LoadImage onto the canvas, and the
   rubber band starts **with Ctrl** (anywhere since session 18). `.mtb-tile` / `.mtb-selected` CSS in `src/workspace.rs`.
4. **MTB header folder chooser** — the text entry is now a searchable `GtkDropDown` listing every `input`/`output`
   folder as `input - 3D`, `output - realism`, etc. The previous agent's subfolder navigation is kept (double-click a
   folder tile, back button); choosing a dropdown entry jumps to that folder.
5. **Blank workspace on launch** — `on_connected` restores a session, then calls `doc_or_blank("Untitled")` if no
   document is open, so double-clicking empty space can open the node picker from a cold start.
6. **Tab / session persistence** — new `src/workspace/session.rs`: 500 ms debounced `session.json` in
   `~/.config/comfy-gnome/` (atomic write), storing open tab paths + temp flags + dirty + viewport + serialized
   workflow, the active tab, restoring all of it on connect. Gated by `Comfy.Workflow.Persist` (default true),
   which is also read from `/api/settings`. Hooks: `open_doc`, `edited`, canvas change, tab switch, close, save.
7. **Lora Manager text↔rows** — `src/graph_view.rs` now treats the hidden `loras` widget array as authoritative:
   `lm_indices()` finds the text/loras widget indices via `properties.__lm_widget_ids` (fallback 1/2);
   text edits merge/remove/append rows; row toggle flips `active` while keeping the tag; strength/clip strength and
   remove update both text and array. Mirrors upstream `mergeLoras` / `applyLoraValuesToText` semantics and Python
   `get_loras_list` (inactive entries are skipped at execution). This also makes text-erase → row removal work for
   any node exposing the same `AUTOCOMPLETE_TEXT_LORAS` contract.
8. **Node collapse/expand** — one click on the node's top-left corner (litegraph's square) toggles `collapsed`,
   relayouts and syncs; `graph_view::sync` copies `collapsed` into the workflow JSON, so it persists in saved files.
9. **Interrupt / cancel** — `Workspace` keeps the header stop button and queue-side Interrupt button; both enable
   from `QueueState.running` and on successful submit, and call `Client::interrupt` (`POST /interrupt`). A red
   `process-stop-symbolic` button now sits immediately right of Run, matching the web top bar.
10. **Sampler preview methods** — `queue_prompt` sends `extra_data.preview_method` when the setting is not
    `default`; `Workspace.preview_method` is loaded from `Comfy.Execution.PreviewMethod` and updated live from the
    Settings dialog. (Only the transport is wired — see uncertain.)
11. **Sidebar stretch** — `AdwOverlaySplitView` now `min 240`, `max 620`, initial fraction 0.34 (was effectively
    fixed at 268–280). MTB's `GtkFlowBox` already reflows into more rows as the pane widens.
12. **Wire-release node menu** — releasing a wire on empty canvas opens the same node picker as double-click;
    candidates are filtered to nodes with a compatible socket, and the new node is auto-connected to the wire.
    Implemented via `Canvas::connect_link_menu` / `connect_ids`, `finish_link -> bool`, public `compatible`;
    `Workspace::add_node_dialog` accepts an optional `(node, slot, out, type)` requirement.
13. **Autocomplete for prompt/LoRA text** — `src/canvas/widgets.rs` completion popover (debounced 150 ms) for
    `<lora:` names (`/api/lm/loras/relative-paths`) and tag words (`/api/lm/custom-words/search`), with
    Up/Down, Tab/Enter per `loramanager.autocomplete_accept_key`, append-comma/space-underscore prefs, and
    escape-to-close. Wired for every `Text` widget through `Canvas::set_api_base` (set in `open_doc`).
14. **Resource Reaper panel** (wired in session 18) — `src/workspace/reaper.rs` implements the Memory page:
    GPU/RAM meters, Free-memory-now level + button, last automatic pass, "What to close" suggestions with
    confirmation → terminate, biggest processes, and all config switches. Polls `/reaper/status` every 3 s only
    while mapped; POSTs `/reaper/clean`, `/reaper/config`, `/reaper/terminate`.

## Session 18 — finish session 17, live smoke test, new icon

Tested with `target/release/comfy-gnome` on Xvfb :99 (`DBUS_SESSION_BUS_ADDRESS=disabled:`, isolated
`XDG_CONFIG_HOME`) against the real ComfyUI on :8188. `cargo test`: 39 passed, 4 ignored, zero warnings.
Golden (`cargo test golden -- --ignored`, 80 newest outputs): 39/80 exact, nodes 1005/1016; the only mismatches
are `ImpactWildcardProcessor` placeholder/seed values the backend ignores.

### Session 17 to-dos closed
- **Reaper wired** (`mod reaper;`, Memory page after MTB, `.rr-*` CSS). Contract verified against the Python
  source (`custom_nodes/midfr-comfyui-nodes/A-toolbox/ComfyUI-Cleaner/api.py`, `sysinfo.py`, `config.py`): all
  field names in `src/workspace/reaper.rs` match; severities are `critical|warn|info`. Live page works.
- **Warnings** removed (dead `lm_set_tag`, `placeholder`, `navigate`; scratch experiment deleted).
- **"Save history"**: kept as upstream `Comfy.Workflow.Persist` semantics (tabs, drafts, active tab, viewport);
  queue history still comes from `/history`. Session now also saves on widget edits (`set_dirty` → `schedule_session`).
- **Polish**: stop/interrupt disabled + running highlight cleared on WS disconnect, queue refreshed on every
  connect; icon checked at 16/24/32 px.

### Bugs found and fixed during the live test
- Viewport conversion (pre-existing): litegraph `ds` is `screen = (pos + offset) * scale`; we wrote/read it as
  `pos*zoom + off`. Saved views now round-trip with the web UI.
- Blank docs no longer prompt on close (`dirty: false`, like upstream `isModified`).
- Stale `link.target_slot`: new `graph_view::target_slot` uses the input that actually holds the link id.
- Union input types (`"AUTOCOMPLETE_TEXT_PROMPT,STRING"` + `widgetType`) are widgets → fixed "Required input is
  missing: text" on LoraManager Prompt.
- `POST /interrupt` sent a JSON `null` body → server 500. Now `{}`.
- Wildcard `*` outputs are compatible with every input (wire-release picker + linking).
- Add-node search ranks results (exact, prefix, word-start, contains) over name/class/category; Enter adds the top.
- Autocomplete popover closed itself as the suggestion list appeared (popover resize on X11). Editor and list
  now have fixed heights; verified Down+Tab and Enter insert, Ctrl+Enter commits.
- LM prompt text is structural, so committing text re-renders the LoRA rows.
- MTB: Ctrl+drag band starts anywhere (tiles fill the pane); Ctrl+click toggles on release. Folder dropdown
  jumped to the wrong folder (tuple order) and its search didn't filter (no expression) — both fixed.
- Sidebar: drag handle on the split edge (240–900 px); MTB reflows to more columns.
- Splash shows the app icon (bundled in the gresource).

### UI pass (after live test)
- **Sidebar**: icon-only tab strip (`side_stack` driven by linked `ToggleButton`s). Drag handle clamps at the
  width where the icons just touch (`tabs_width()` + 24); dragging >40 px further hides the sidebar
  (`toggle.set_active(false)`). Hidden: 14 px hot zone on the window's left edge (`gtk::Overlay` around the split)
  reveals a small white `go-next-symbolic` pill; click re-shows. Pages sit in a `ScrolledWindow` (External hscroll)
  so the widest page (Memory) cannot prop the sidebar open. Memory dropdown labels shortened for the same reason.
- **MTB dropdown** lists only top-level folders (`input`, `input - 3d`, `output`, `output - AIXG` ...); deeper
  folders are reached by double-click and the dropdown keeps showing the top-level parent.
- **Log/Preview dock**: pill tabs (icon + label inline), flat collapse button, new Clear-log button, 9.5 pt log with
  red/amber lines for errors/warnings, compact "No preview yet" placeholder until the first image.

### New app icon
`data/icons/hicolor/scalable/apps/io.github.comfygnome.ComfyGnome.svg`: GNOME Extensions' green puzzle piece with
ComfyUI's yellow "C" mark. Flatpak rebuilt and installed with it.

### Still unverified
- Preview frames per method: transport matches `execution.py` (`extra_data.preview_method`), but no checkpoint is
  installed, so no sampler run was possible. Same for Lora Manager rows with real LoRA files (`models/loras` empty)
  and the red stop button during a long sampler run (verified with a `Sleep` node instead).
- `__lm_widget_ids` is the list of serializable widget names in `widgets_values` order (checked in LM JS); only
  the LoraManager loaders were looked at.

## Session 19 — launching from app-menu shortcuts (native + Flatpak)

User report: workflows misbehave when started from GNOME application shortcuts for either build; the launch
scripts do run, but "have separate issues at times". Four independent causes, all fixed:

1. **Native shortcut never rebuilt when sources changed.** `launch-system.sh` calls `cargo build --release`, but a
   menu launch inherits gnome-shell's PATH, which lacks `~/.cargo/bin`; the build died and only a `notify-send`
   reported it. The script now prepends `$HOME/.cargo/bin:$HOME/.local/bin` to PATH. Verified with
   `touch src/main.rs` + `./launch-system.sh` under `env -i` carrying the exact gnome-shell PATH: rebuilt in 15.7 s.
2. **Script mode ignored the Port field.** `Backend::spawn` appended `--port` only for folder launches, so
   `start_comfy_hefty.sh` (hardcoded 8973) served 8973 while the app polled the saved 8188 and never connected.
   `--port cfg.port` is now appended for scripts too, before the configured args (scripts forward `"$@"`; the
   user's scripts do). Verified: the stub launch script logged `args: --port 8299`.
3. **Flatpak leaked its host backend.** Inside the sandbox the direct `flatpak-spawn` child is a process-group
   leader, so `exec setsid` forked; `--watch-bus` then killed the already-dead direct child while the real ComfyUI
   kept a new session. `kill -TERM -<sandbox pid>` cannot reach host pids either, and `setsid` + `--watch-bus` was
   confirmed to orphan in experiments. Evidence: the 03:48 Flatpak run left `python main.py --port 8188` (PID
   14546) alive after the app exited, and the next Flatpak launch attached to it. Fix: a host helper
   `$XDG_CACHE_HOME/comfy-gnome/host-exec.sh` (`echo $$ > "$1"; shift; exec "$@"`) is spawned as
   `exec setsid <helper> <pidfile> <target>`; the helper becomes the session leader and execs in place, so the
   pidfile pid == PGID == SID. `Backend` stores the pidfile; `stop()` runs a host-side
   `flatpak-spawn --host sh -c 'kill -TERM -"$p"; …; kill -KILL -"$p"' sh <pid>`, removes the pidfile, then TERMs
   the wrapper. `/bin/sh` is dash, which rejects `kill -- -pid` (`Illegal number: -`), so the host script uses
   `kill -TERM -"$p"`. `stop_hard()` and `is_running()` use the same host group. Folder and script modes are both
   covered; a hard crash before stop still leaks (same as before, noted in the code).
4. **Flatpak menu entry didn't force X11.** `launch-flatpak.sh` sets `GDK_BACKEND=x11` but the exported desktop
   entry did not; the manifest now carries `--env=GDK_BACKEND=x11`, so a menu launch matches the scripts (the
   README only claims X11 support).

Verification: `cargo build --release` clean, `cargo test` 39 passed / 4 ignored. Native shortcut path:
`launch-system.sh` under a GNOME-like `env -i` PATH rebuilt and launched on Xvfb :99 + openbox with an isolated
config; Launch passed `--port 8299`; `wmctrl -c` closed cleanly with the stub's process group gone and 8299 free.
Flatpak: rebuilt (`flatpak-builder --user --install …`, commit eb7f091e), app-menu-style `flatpak run` on :99 with
the same stub config; the host stub had PID=PGID=SID=40986, the pidfile matched, and closing the window removed the
host group, the pidfile and the port. Folder mode was re-tested separately with a fake `main.py`: the host python
had PID=PGID=SID=45492; closing the window again removed the group, the pidfile and the port. This closes session
18's unverified "launching ComfyUI from the Flatpak when no backend is running" and "window-close teardown in
script mode".

Orphan: PID 14546 (`python main.py --port 8188`, started by the 03:48 Flatpak run, PPID systemd) was still
listening during this session; it was killed at the user's request and 8188 is free. It was the backend leaked by
the old Flatpak teardown; the host-pidfile fix prevents a repeat on normal window close.

## Session 20 — live previews (TAESD / Latent2RGB) + sampler progress bar

Two independent gaps in `src/api.rs` / `src/workspace.rs` made TAESD and Latent2RGB look dead:

1. **This fork gates previews behind a WebSocket feature-flag handshake.** The client's *first* message must be
   `{"type":"feature_flags","data":{"supports_preview_metadata":true}}`. `comfy_execution/progress.py:209` then
   sends `PREVIEW_IMAGE_WITH_METADATA` (binary event 4: u32 event, u32 JSON length, JSON metadata, image bytes)
   only when that flag is advertised; that path has no legacy fallback. The app never sent the handshake, so no
   preview frames arrived for any method. `ws_session` now sends it right after connecting.
2. **The parser only knew event 1.** `parse_binary` handled `PREVIEW_IMAGE` (8-byte header) but not event 4; it
   now strips the JSON metadata prefix and feeds the image to the same `gdk::Texture` path. Assertions for both
   frames live in `api::tests::parses_ws_messages`.

Progress bar: a 3 px `GtkProgressBar` (`sampler-progress`) sits between the `AdwTabBar` and `AdwTabView` in
`canvas_pane`, flush under the tabs and edge to edge (CSS zeroes the trough margin and radius). It is hidden by
default and on `executing: null`, `execution_error`, reconnect, and each new `executing` node; `progress` and the
fork's combined `progress_state` both drive it. `progress_state` is reduced to its busiest `running` node
(`running_progress`), so chained samplers do not make the bar jump backwards.

Verification: `cargo build --release` 16.84 s; `cargo test` 39 passed / 4 ignored. UI test with a mock WS server
on 8299 under Xvfb :99: the server received the handshake as its first message, sent `progress_state` (5/10) and a
type-4 PNG; the window showed the 3 px bar under the Untitled tab and the orange "live preview" frame. Mock files
are under `/tmp/cgpreview` (temporary; remove if present). The real ComfyUI needs no special launch flag — previews
follow the existing Preview method setting (`default`/`auto`/`latent2rgb`/`taesd`).

## Session 21 — previews on the running node

The frontend rule is in `ComfyUI_frontend-1.60.0/src/scripts/app.ts:825` (`b_preview_with_metadata`): the frame
carries `displayNodeId`, it is stored under that execution id, then the same image is applied to every ancestor
execution id (`getAncestorExecutionIds`, `src/types/nodeIdentification.ts:248`). The id comes from the backend:
`comfy_execution/progress.py:217` puts `dynprompt.get_display_node_id(node_id)` in the type-4 metadata, and
`comfy_execution/graph.py:64` follows `ephemeral_display` up to the outer host. So SamplerCustom and custom
samplers bubble to the node the user sees, and subgraph levels resolve to the graph node. Rendering is generic:
`src/services/litegraphService.ts:718/833` draws `node.imgs` for any node; there is no KSampler-only path.

Implementation: `WsEvent::Preview` now carries `display_node: Option<String>`; `parse_binary` reads
`display_node_id` (falling back to `node_id`, string or number) from the type-4 JSON. `Workspace::show_live_preview`
stores the frame under `live:<id>` in `run_doc` and calls `canvas.set_node_images`. No ancestor walk is needed —
the backend already names the host node. `canvas/mod.rs` no longer requires `flex == 0` to reserve an image row,
so a node with multiline widgets grows to make room for the image instead of hiding it.

Verification: `cargo build --release` 16.46 s; `cargo test` 39 passed / 4 ignored. `tools/demo/mock_backend.py`
gained a `--preview` mode (proper unmasked WS frames, `itertools.cycle(1..21)`, `executing` + `progress_state` +
type-4 frames with `display_node_id: "5"` every 0.25 s). Run it on 8198 under Xvfb, open `harbour_sdxl`, press
Run (so `run_doc` exists), then `.` to fit: KSampler id 5 shows the live gradient at the bottom of the node, the
3 px progress bar runs under the tabs, and the log pane shows the same frame. `docs/screenshots/graph.png` and
`docs/screenshots/queue.png` were regenerated from that run; `docs/screenshots/icon.png` is the current SVG icon
rendered at 512 px and is what the README now embeds (GitHub's SVG cache shows the old icon otherwise).

Not covered: the preview lives only while the run's document is open and `run_doc` is set; a hard crash still
leaks a host backend (see session 19).
