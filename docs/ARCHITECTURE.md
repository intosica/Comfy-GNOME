# Architecture

## Layout
```
src/main.rs            resources, adw::Application, window, Stack(splash → workspace), stop backend on close
src/splash.rs          launch configuration page → LaunchConfig
src/backend.rs         LaunchConfig, find_comfy_dir(), probe(), spawn() (+ browser shims), Backend (Drop = stop)
src/workspace.rs       OverlaySplitView: sidebar | header + Paned(canvas / log)
src/canvas/mod.rs      Canvas: DrawingArea, State, input controllers, cairo drawing
src/canvas/transform.rs View: zoom/pan math (unit-tested)
build.rs               compiles data/resources.gresource.xml (bundled Adwaita symbolic icons)
build-aux/*.json       Flatpak manifest;  data/*.desktop;  launch-system.sh / launch-flatpak.sh
```

## Rendering choice
Helvum makes every node and port a real GTK child widget. ComfyUI graphs routinely have hundreds of nodes
with many widgets each; a widget tree that size costs a full measure/allocate pass on every pan/zoom and
multiplies X11 input-region work. So the canvas is **immediate mode**: one `gtk::DrawingArea`, plain Rust
structs in `Rc<RefCell<State>>`, everything redrawn by cairo in `draw()`; input is hit-tested manually.
No GObject subclasses. Text uses the cairo toy API for now (switch to pango in Phase 3).

Draw order: background `#202020` → grid (20 canvas units, skipped below 6 px) → links → nodes in
`State.order` (z-order; clicking raises) → HUD.

## Coordinate math (`canvas/transform.rs`)
```
screen = canvas * zoom + offset
canvas = (screen - offset) / zoom
```
Zoom anchored at pointer `a` by factor `f` (keeps the canvas point under `a` fixed):
```
c = to_canvas(a);  zoom' = clamp(zoom*f, 0.1, 4.0);  offset' = a - c*zoom'
```
Scroll: `f = 1.1^(-dy)`. Pan: `offset += screen delta`. Node drag: `node.pos = drag_start_pos + delta/zoom`.
The cairo context is set up with `translate(offset)` + `scale(zoom)` so all node drawing is in canvas units.

## Node geometry (litegraph-compatible)
`Node.pos` is the **body** top-left; the title bar (30) sits above it, as in litegraph so ComfyUI workflow
JSON positions can be used as-is. Slot `i` centre: `y = pos.y + (i + 0.7) * 20`, x = left/right edge.

## Wires
Cubic Bezier from Helvum (see TRANSLATION-NOTES): control points offset horizontally by half the x distance,
lifted by `max(0, 25 - |dy|)` when the wire goes backwards.

## Backend process
- **Skip**: no process; just poll `GET /system_stats` (std `TcpStream`, raw HTTP/1.0, off-thread via
  `gio::spawn_blocking`) every 2 s until 200.
- **Launch ComfyUI** — two modes, one code path (`backend::spawn`), run with `gio::SubprocessLauncher`
  (stdout+stderr merged, read line-by-line async on the main loop into the log; ANSI escapes stripped):
  - **Folder** (default; auto-detected by `find_comfy_dir`, validated by `main.py` + `comfy/`):
    `sh -c 'cd "$1" && shift && <activate venv|.venv> PYTHONUNBUFFERED=1 PATH=<shims>:$PATH exec setsid python main.py "$@"' python DIR --port N args…`
    (`--port` from the splash goes before user args so theirs win).
  - **Script** (optional, overrides folder): same wrapper with `exec setsid SCRIPT "$@"`, cwd = script's directory.
  - `setsid` → own process group; `Backend` `Drop` sends SIGTERM to `-pgid` (python + script children die; scripts'
    cleanup traps run). Triggered from `window.connect_close_request` (destroy signals don't fire on app quit).
  - `PR_SET_PDEATHSIG=SIGTERM` in the child (set via `set_child_setup`) covers app crashes / `kill -9`.
  - `PYTHONUNBUFFERED=1` or the log only updates every few KiB.
  - **Browser blocking**: user scripts open Thorium/Chromium/xdg-open after starting ComfyUI. With the
    switch on, a shim dir under `$XDG_RUNTIME_DIR` is prepended to `PATH`: browser names → `exec sleep infinity`
    (scripts that run the browser in the foreground and kill ComfyUI when it exits keep waiting), openers
    (`xdg-open`, `gio`, …) → `exit 0`. Only works for bare-name invocations, not absolute paths.
  - Flatpak: prefixed with `flatpak-spawn --host --watch-bus` so the host's python/venv/CUDA are used (seamless
    with native); shim dir is `$XDG_RUNTIME_DIR/app/<APP_ID>`, which is the same path on the host.
    `--filesystem=home` makes picked paths real host paths.

## Icons
The user's icon theme (`gnome`) lacks most `*-symbolic` icons, so the Adwaita SVGs used by the app (and
libadwaita's `document-edit-symbolic`) are compiled into the binary (`build.rs` → gresource). GTK automatically
adds `<resource-base-path>/icons` (derived from the app id) to the icon theme. New icon = copy SVG into
`data/icons/scalable/actions/` + add a line to `data/resources.gresource.xml`.

## Threading
GTK main thread owns all UI and state. Blocking I/O goes through `gio::spawn_blocking` (Phase 1) and will move to a
dedicated backend thread + `async_channel` in Phase 2 (Helvum's GTK↔worker message pattern).

## Session 2 additions

### Modules
```
src/api.rs        REST + WS client (blocking ureq/tungstenite on gio::spawn_blocking)
src/registry.rs   /object_info → NodeDef/Kind (incl. v3 COMBO + COMFY_DYNAMICCOMBO_V3)
src/workflow.rs   saved workflow JSON (0.4 tuple links, subgraph defs, groups, extra.ds)
src/prompt.rs     workflow → API prompt (graphToPrompt port, subgraph flattening)
src/graph_view.rs workflow graph → canvas scene (nodes/links/groups)
```
Data flows one way: `workflow::Workflow` (parse) → `prompt::build` (execute) and
`graph_view::scene` (draw). Nothing in `workflow.rs`/`prompt.rs` touches GTK.

### Coordinates and viewport
Canvas space is litegraph space: node `pos` is the top-left of the **body**, the title bar
occupies `[pos.y-30, pos.y]`. `screen = canvas * zoom + offset` (`src/canvas/transform.rs`).
Saved workflows carry the viewport as `extra.ds = {scale, offset:[x, y]}`; we restore it on
load (zoom clamped to 0.1..4.0) instead of fitting, which reproduces what the user saw in the
browser. "Fit" and "Reset" buttons cover the rest.

### Rendering budget
- Node geometry comes from the file; heights are *grown* (`max(saved, slots*20 + widgets*24 + 8)`)
  because extension widgets may have been added since the file was saved. Never shrunk.
- Everything is culled against the visible canvas rect: nodes, groups and each link's
  bounding box (control points stay inside `x ± half_distance`, `y - 30`).
- All text is skipped below 0.4 zoom (cairo toy API, ~13 px). No pango yet.
- Widget values are drawn as read-only pills from `prompt::widget_map`, i.e. exactly the
  values that would be sent to the backend.

### Images
`gdk::Texture::from_bytes` decodes PNG/JPEG through gdk-pixbuf, so previews need no `image` crate and no temp
files; the bytes come either from WS binary frames (type 1, 8-byte header dropped) or from `/view` after an
`executed` event (which the server only sends to the client that queued the prompt).

## Session 3 additions: documents, tabs, editing, saving

```
src/workspace.rs  Workspace { docs: Vec<DocRef>, tabs: adw::TabView, ... }
                  Doc { file, wf: Workflow, path: Vec<String> (subgraph drill-down),
                        canvas: Canvas, page: adw::TabPage, dirty }
```
One `Doc` == one open workflow == one `TabView` page whose child is that doc's `DrawingArea`.
`path` is the subgraph stack: empty = root graph, `["uuid"]` = that definition, and the back
arrow pops it. Every tab keeps its own `Canvas`, so viewports are independent; the model
(`Workflow`) is only re-read from the canvas when the tab is rendered or saved.

**Editing pipeline (canvas → model → server):**
1. Canvas mutations (node move, link create/detach) are purely visual.
2. `connect_change` fires once per completed gesture → `set_dirty(doc, true)` (tab title gains `•`).
3. `Ctrl+S` → `sync_doc` → `graph_view::sync(graph, nodes, links)` rebuilds `graph.links`
   (keeping the id of an unchanged endpoint quad, otherwise `max+1`) and rewrites every
   `inputs[i].link` / `outputs[i].links`; `sync_io` refreshes subgraph `linkIds`.
4. `workflow::to_json()` = `raw.clone()` with `pos`, `links`, wiring and `extra.ds` spliced in,
   so fields we do not model survive untouched.
5. `api::save_workflow` → `POST /api/userdata/<percent-encoded "workflows/<file>">`.

**Link editing semantics** (`canvas::hit_slot` / `hit_target`, radius `9/zoom` canvas units). A dragged wire is
anchored at exactly one end (`LinkDrag { node, slot, out, ty, to, changed }`); `out` names the side the anchor sits
on and the drop test therefore looks at the *opposite* side — `hit_target(p, r, ty, !out)`, which also picks the
right comparison direction because `compatible()` is asymmetric (only the input may list alternatives like
`"IMAGE,MASK"`):
| gesture | result |
|---|---|
| drag from output | new link from that output |
| drag from connected input | link unplugged, still attached to the cursor; drop on nothing deletes |
| shift+drag from connected output | link **de-anchored at that output**: the wire keeps hanging from the input it fed and now hunts for a new output (drop on nothing deletes it) |
| drop on incompatible type | ignored (hover ring only on compatible targets) |

**Saving is bit-faithful.** `Ctrl+S` runs `canvas → graph_view::sync → Workflow::to_json → POST /api/userdata`.
`to_json` clones the workflow's original `raw` JSON and only splices positions/wiring/`ds` into it, so extension
fields, unknown node properties and `definitions` survive untouched. Two invariants keep the file unchanged when
nothing was edited: serde_json is built with **`float_roundtrip`** (its default float parser is off by an ulp on long
mantissas and would rewrite every node position), and `write_graph` never invents keys — `definitions` is looked up
with `get_mut`, an output's `links` stay sorted ascending, and `links: null` stays `null` instead of becoming `[]`.
Verified by saving untouched copies of a 29-node workflow and of one with a subgraph definition: both compare equal.

## Session 4 additions: selection, manipulation, menus

**Selection lives in the canvas** (`State.sel: HashSet<String>` of node ids), so it survives re-renders and is what
every group operation acts on. Rules, mirroring litegraph: shift-click toggles membership; a plain click inside the
current selection keeps it (so a multi-node drag does not collapse to one node); a plain click outside selects just
that node; `Ctrl`+drag on empty canvas rubber-bands; `Ctrl+A` selects all; clicking empty canvas or a group header
clears it.

`enum Drag` names the gesture that owns the pointer:

| variant | started by | end effect |
|---|---|---|
| `Nodes { idx, grab, start }` | left-drag on a node | move every selected node by the same delta; `start` enables a cheap change test |
| `Group { idx, start, nodes, grab }` | left-drag in a group's **title bar** | move the group rect and every node whose **centre** is inside it |
| `Band { start, end }` | `Ctrl`+left-drag on empty canvas | select all intersecting node rects |
| `Link(LinkDrag)` | left-drag on a socket | wire creation / unplug (see session 3 table) |
| `Pan { start_offset }` | left- or middle-drag on empty canvas | translate the viewport |

**Context menus are plain `gtk::Popover`s full of `gtk::Button`s**, built per invocation as
`Entry::Item { label, danger, f }` / `Line` / `Swatches(vec)`. This is deliberate: `GtkPopoverMenu` + `gio::Menu`
with `win.*`-style actions silently never activated under this build (items highlighted, the popover closed, no
warning), while buttons work. Menu entries map 1:1 onto model operations, so keyboard shortcuts (`Ctrl+B`, `Delete`,
…) and the menu share one code path (`with_selection` → `bypass`/`mute`/`delete_nodes`/…).

**Model-only data must be written by the editor, not read from the file.** `write_graph` reconciles three lists that
the loaded JSON cannot know about: (1) nodes — entries whose id vanished from the model are dropped and model nodes
without a JSON entry are appended via `fresh_node_json` (which reproduces the frontend's own node shape: `properties
{"Node name for S&R"}`, `outputs[].slot_index`, `flags`, `mode`, and `"widget": {"name": …}` on widget-shadowed
inputs); (2) groups — rebuilt from `Canvas`/model rects with ids preserved by index; (3) per-node `mode`, `title` and
`flags.collapsed`. Without this, "create a node, save" and "add a group, save" both wrote nothing (found by testing).

**Bypass rendering.** A `mode: 4` node is painted magenta and each *connected* input gains a dashed white line to the
first output whose type is `compatible()`, falling back to output 0 — the same "passthrough" the web frontend draws,
so a bypassed node still reads as part of the graph. The dashes are drawn **after** node bodies (drawing them before
means the body paints over them). Muted (`mode: 2`) nodes are drawn at 40 % alpha.

**Subgraph boundary pseudo-nodes** get ids that do not exist in the model, so editing them is filtered out in
`with_selection` and their right-click falls back to the canvas menu; double-clicking a subgraph *instance* pushes its
`uuid` onto `Doc.path` and re-renders from `definitions.subgraphs`.

The Inputs node ends in one bare socket (`label` and `ty` empty, which `compatible()` accepts against anything):
that is the "plug a new input here" dot. A wire dropped there makes `sync_doc` call `graph_view::grow_inputs`, which
appends a `SubgraphIo` named after the node input it feeds (deduplicated `_2`, `_3`, …), fixes the link's `type`, and
pushes the same `Input` onto every host instance, so the parameter shows up on the subgraph node too. The change
callback re-renders once the input count grows, and the add-socket stays the last row.


**Widget sockets, resize, snapping.** Every widget row carries the socket the web UI draws there, so a
parameter can be turned into a link without a convert step (1.60 keeps sockets and widgets side by side). Files
only store those inputs once converted, so `graph_view::scene` appends a socket for every declared widget input
that the file lacks (`prune_extra_slots` drops the ones a special-cased builder never drew); `sync` writes the
real `Input` only once a wire lands on one, so untouched nodes save unchanged. rgthree's Power Lora rows are built
in JS and have no declared inputs at all: `grow_widget_slots` names one `lora_N` socket per row (the name the
backend expects) and the same sync path materialises it. Pulling an unconnected input socket starts a wire looking
for an output (`LinkDrag { out: false }`), which is how a widget dot becomes a link. Nodes resize from a grip at the
body's bottom-right corner (`Drag::Resize`, minimum 120×44, height re-grown by `relayout`), and `write_graph` now
persists `size` when it differs from the file. Dragging or resizing snaps to `Comfy.SnapToGrid.GridSize` when the
gesture holds Shift or `pysssss.SnapToGrid` is on (litegraph's `snapPoint`, `snap_to`); the workspace fetches both
settings on connect and mirrors them onto every canvas (new tabs included), and settings edits apply live.

**Link colours** come from the litegraph palette (`LGraphCanvas.link_type_colors`, ported to
`canvas::type_color`): MODEL `#b39ddb`, CLIP `#ffd500`, VAE `#ff6e6e`, CONDITIONING `#ffa931`,
LATENT `#ff9cf9`, IMAGE `#64b5f6`, MASK `#81c784`, and a hashed fallback for unknown types.

**Viewport restore vs auto-fit:** `set_graph` marks `pending_fit`; the resize callback fits once
the widget is allocated. `set_view` (saved `extra.ds`) clears that flag, so a restored viewport
always wins and auto-fit only happens for graphs without one.

## Session 5 additions: widgets
- `canvas/widgets.rs` owns widget kinds, cairo painting and popover editors; `Node` carries `widgets`, cached row
  layout (`relayout`) and image keys. Pointer path: `drag_begin` → slot, then widget row (`Drag::Widget`), then node.
- The canvas never writes the model. Edits fire `connect_widget`; the workspace calls `graph_view::apply_widget`
  (the only write path: root widgets, promoted subgraph widgets, Power Lora rows, Fast Groups toggles).
  Structural edits sync canvas→model and re-render; value edits only mark the doc dirty.
- Widget slots come from the registry via `prompt::widget_slots` (same positional rules as `prompt::map_widgets`,
  so display, editing and prompt building cannot diverge).
- Images: async `/view` fetch → `gdk::Texture` → cairo `ImageSurface` cache keyed by view query string.

## Prompt conversion invariants (learned from real captures)
1. Node ids are strings; subgraph execution ids are `<host path>:<node id>`.
2. A subgraph instance is not emitted; its inner nodes are, using the host's prefix.
3. Links from `-10` continue at the host's input with the same **name** (not index); an
   unlinked host input with a `widget` provides a promoted value.
4. Muted (2) nodes emit nothing and their outgoing links vanish; bypassed (4) nodes pass the
   value through a type-compatible input.
5. Widget values map **positionally** onto widget-capable inputs, plus UI-only slots
   (control_after_generate, upload buttons, autocomplete metadata) that are skipped.
6. Arrays are wrapped as `{"__value__": [...]}` or the backend reads them as links.
7. Anything unknown to `/object_info` is dropped with a warning (frontend "virtual" nodes).

## Text, tab menu, EreNodes (session 6)- `canvas/text.rs`: pangocairo FFI (`width`, `show`); every canvas string goes through it for glyph fallback.
- `workspace/tabmenu.rs`: tab context menu (plain Popover buttons), temp-doc variant, close guard; `pngmeta.rs` reads PNG workflow metadata.
- `erenodes.rs` + `workspace/ere.rs` + `WKind::Cloud` (`widgets.rs`): tag widgets backed by `properties._tagDataJSON`; ops that need the
  workspace (load group, add tag, convert) are emitted from the canvas as `"ere"` objects `{op, at}`.

## Breadcrumb and Templates (session 7)
- `Workspace::show_trail` rebuilds `crumbs: gtk::Box` (flat buttons) on every render: home = workflow stem, then one
  button per `Doc::path` level, active one suffixed `▾` and opening a two-item popover (`Rename`, `Clear Workflow`).
  All navigation is `Doc::path.truncate(n)` + `render_doc` — the canvas never owns subgraph state.
- `Workspace::rename_subgraph` writes the name into `workflow::Subgraph.name` and into `Node.title` of every node
  whose `type == subgraph id` (root **and** other subgraph graphs), which is what makes the host node label change.
- `tabmenu::clear_workflow` branches on `!doc.path.is_empty()`: subgraph clear keeps `SUBGRAPH_INPUT_ID` /
  `SUBGRAPH_OUTPUT_ID` nodes and drops links+groups, root clear drops everything.
- `workspace/templates.rs` is self-contained UI over three endpoints (`/templates/index.json`,
  `/api/workflow_templates`, `/templates/{name}.json` + `/templates/{name}-1.webp`); it only talks to the rest of the
  app through `api::view_query` and `Workspace::open_temp`, so it can be replaced without touching the canvas.

## Queue history, lightbox, job menus (session 8)
- `api::HistoryEntry` (parsed by `HistoryEntry::parse`) mirrors one `GET /history` entry: `prompt[0]` = queue number,
  `outputs.<node>.images[]` = results, `status.messages` = `execution_start` / `execution_success` epoch-ms stamps
  (`duration()`).
- `Workspace` keeps the ledger: `history: Rc<RefCell<Vec<api::HistoryEntry>>>`, `queue_done: Cell<bool>` (filter),
  `thumbs` (queue-row textures), `preview_items` (what the Preview tab is showing). `set_queue` renders live rows when
  the All filter is on, then day headers + finished rows newest-first; `queue_key` includes the history length so an
  unchanged `/queue` still re-renders when a run finishes.
- `Workspace::show_lightbox` is the only image viewer: `adw::Dialog` (90 % of the window, dim background), one
  `gtk::Picture` at natural resolution, OSD nav buttons + arrow keys, and a decode generation counter.
- `api::Client::job_workflow(prompt_id)` is the single source for "the graph this job ran": it looks in
  `/history/{id}` first (`prompt[3].extra_pnginfo.workflow`, the same blob `queue_prompt` sends as `extra_data`) and
  falls back to `/queue` for jobs that are still running or pending.

## Batch count, DaSiWa hooks, backend lifecycle (session 9)

- `Workspace::queue_prompt` loops `copies = count.value()` times: `dasiwa::before_queue(&mut wf)` → `prompt::build` →
  `client.queue_prompt(..., front)` → `graph_view::bump_controls`, then one log line with every returned prompt id.
  `dasiwa::before_queue` is where extension behaviour lives (status switches, seed rolls) — nothing else in the pipeline
  knows about custom packs.
- `src/dasiwa.rs` also owns the two widget surfaces' data: `SeedState` (`{mode,last_seed,recent}` ⇄ `seed_control_state`) and
  `stack_rows`/`stack_set` (`stack_data` JSON string ⇄ `WKind::Lora` rows). `graph_view::apply_widget` writes `stack:` /
  `stack+` keys back into `widgets_values[2]` **and** `properties.stack_data`, mirroring the pack's JS.
- `WKind::Seed` is a click-only row (left: mode, right: 🎲) so the canvas never has to edit raw JSON in place.
- `Backend::stop_hard()` is the only teardown path: TERM the group, wait, KILL the group; `main.rs` calls it from
  `close-request` and `connect_shutdown`. A spawn that dies immediately is reported as "served by an already-running backend",
  and such a backend is never signalled.

## Extension node UIs and Settings (session 10)

- **Widget interpretation lives in one place.** `graph_view::build_widgets` switches on `node.ty` for the packs whose JS
  replaces the stock widget set, and `graph_view::apply_widget` mirrors the writes back:
  - `easy wildcards` — the two pickers keep their placeholder as the stored value; we show the registry's real options and
    append a pick to the prompt (`<lora:NAME:1>` / `__token__`) instead of writing it into the combo.
  - `Lora Loader (LoraManager)` — `widgets_values = [state, text, parsed]`; `lm_tags`/`lm_set_tag` parse and rewrite the
    `<lora:name:strength>` tags in `text`, and each tag is a Lora row (`lm:{i}`) with `➕ Add LoRA` (`lm+`).
  - `ShowText|pysssss` — the value is a list of lines; one text row per line (`show:{i}`).
  - `DaSiWa_SeedControl` / `DaSiWa_LTX2LoraLoader` — see session 9.
  Anything else with an array or object value that is not a known shape still falls back to a read-only row, never to a crash.
- **Job → workflow.** `queue_prompt` stores the exact graph it sent in `Workspace::job_wf` keyed by prompt id; the server's
  `prompt[3].extra_pnginfo.workflow` is only a fallback (`api::Client::job_workflow`). That is the difference between "our job"
  and "someone else's job", and it is what makes *Open Workflow in New Tab* reliable.
- **Settings** (`src/workspace/settings.rs` + `src/settings_defs.rs`): values from `GET /api/settings`, definitions scraped
  from the served frontend's JS chunks and every `/extensions` file (tolerant JS tokenizer, cached per process, loaded with
  `gio::spawn_blocking` so the dialog opens instantly). Grouping mirrors `useSettingUI.ts` (`category || id.split('.')`:
  [0] sidebar entry, [1] section). Writes are optimistic, `POST /api/settings/{key}`; Extension page edits
  `Comfy.Extension.Disabled`; settings with JS-function widgets are read-only.
- **Popover input trap (X11):** a tooltip window can sit above a `gtk::Popover` and swallow the click that should hit a menu
  item. Menu-opening buttons therefore carry no tooltip; if a menu item ever "does nothing", check for a tooltip first.
- **Prompt editing** (session 11): `canvas/attention.rs` is a pure port of the web `editAttention.ts` on char offsets;
  `edit_text` wires it to Ctrl+↑/↓ through a capture-phase key controller and applies it as one delete+insert user action
  so GTK's built-in undo covers it. `/queue` entries are the bare `[number, id, prompt, extra_data, outputs]` array —
  only `/history` nests it under `prompt`.
- **Picker combos** are detected generically from the option list (first option "Select … add … text"), not by node
  type; their widget key `pick:idx:text_idx:placeholder` carries everything `apply_widget` needs, since it has no registry.
- **Undo** is snapshot-based and hooks `set_dirty(doc, true)` — the one call every edit path already makes. New edit
  paths get undo for free as long as they mark the doc dirty.
- **Combo previews** are a plug-in: `canvas::widgets` exposes `set_preview_provider(Rc<dyn Fn(option, callback)>)` and
  knows nothing about models; the workspace installs a provider backed by `model_preview` (backend model index +
  LoRA Manager / core preview endpoints). Any combo listing model files gets previews, whatever node it belongs to.
- **Bypass passthrough** is part of the link pass: mode-4 nodes draw a dashed wire from the matching input to the output
  *before* the node loop, so node bodies always cover it.


## Session 15 additions: auto inputs, widget row menus, queue list, link modes

- **Auto inputs are pruned.** `graph_view::grow_inputs` records the names it invents in `Subgraph::auto`, and `graph_view::prune_inputs`
  removes any of them whose `link_ids` is empty, shifting the `origin_slot` of the surviving boundary links down and dropping the
  mirrored `Input` from every host instance. `sync_doc` runs prune → sync_io, and the `connect_change` closure re-renders when the
  input count changes either way, so cutting a wire makes the parameter vanish from the Inputs node *and* the host node.
- **Widget row menus are generic.** `canvas::widgets::menu(ws, i)` returns the row's context entries plus an `Action`
  (`Toggle`, `MoveUp`, `MoveDown`, `Remove`); `Hit::Widget(id, row)` carries the row from the canvas to `Workspace::widget_row_action`,
  which applies them through the same `graph_view::apply_widget` path the canvas uses. `WKind::Lora` is the first implementation
  (rgthree's Power Lora Loader menu); any new widget kind only has to answer `menu`.
- **Queue rows are one iterator.** `set_queue` builds `running`/`pending` with `chain`ed `map`s — the old `once(true).zip(...).chain(...)`
  could only ever emit one row per list, which is why only two queued jobs showed.
- **Link Render Mode** (`Comfy.LinkRenderMode`, litegraph's `LinkRenderType`): `canvas::link_path(mode, …)` draws Straight (jut 10 +
  orthogonal H), Linear (jut 15 + diagonal) or Spline (`curve_to`, control distance `max(30, hypot*0.25)`), and `-1` skips the whole
  link pass. The value is mirrored onto every canvas from `/api/settings`, re-applied on connect and live from the Settings dialog.
- **Settings scraper and imported enums.** Combo options in the built bundle can be `[{value:Y.STRAIGHT_LINK,text:'Straight'}, …]`, where
  `Y` is an import alias, so `settings_defs` resolves such member names through a small table (`HIDDEN_LINK` → −1 … `SPLINE_LINK` → 2);
  `options: {key: label}` objects keep the key as the stored value.
- **Collapsible Log/Preview.** A `pan-down-symbolic` toggle at the right of the bottom switcher hides the whole dock (switcher + content)
  so the canvas gets the space; the last `Paned` position above 40 px is remembered and restored when it comes back.

## Session 16 additions: Node Library, MTB browser, panel strip

- `src/workspace/library.rs` builds the sidebar "Nodes" page from `Registry` (4 700+ definitions): a search entry
  filtered on `display_name`, node name, category and description, a flat capped result list, and one lazily filled
  `GtkExpander` per first category segment when the query is empty. Picking a node calls `Workspace::create_node`
  at the centre of the visible viewport.
- `src/workspace/media.rs` is the MTB panel: root (`input`/`output`) + subfolder path → absolute directory, a
  `Gio.ListStore` of paths (folders first), a `GtkFlowBox` of tiles (`Gtk.Image` with `pixel_size`, so a cell is the
  thumbnail size and the widget does not steal drags), a size slider and a sort combo. Thumbnails decode on a
  blocking pool and are dropped when the page generation moves on.
- Pointer handling is a single `GtkEventControllerLegacy` in `PropagationPhase::Capture` that never claims:
  Ctrl+press on empty space drags a rubber band (`select_rect` intersects every `FlowBoxChild`'s bounds), a press on
  a tile followed by a move of more than 12 px drops that file on the canvas. Event coordinates are surface
  coordinates, so every hit test translates through the overlay (`compute_bounds(&root)`) and then into the flow box.
- `Workspace::open_dropped_at` decides what a drop means: a PNG with a `workflow` chunk opens as a temporary tab,
  anything else is uploaded (`/upload/image`) and becomes a `LoadImage` node with its `image` widget already set.
  `doc_or_blank` creates an empty workflow when the drop (or a library click) arrives with no open tab.
- The trash is written by hand (`Trash/files` + `Trash/info`), because gio's implementation unlinks when it cannot
  reach a portal; `rename` is retried as `copy` + `remove_file` when the target is on another filesystem.
- The bottom `GtkPaned` end child hides only its switcher and page: the bar holding the `▼` toggle stays, which is
  what makes the panel re-openable.
