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


**Link colours** come from the litegraph palette (`LGraphCanvas.link_type_colors`, ported to
`canvas::type_color`): MODEL `#b39ddb`, CLIP `#ffd500`, VAE `#ff6e6e`, CONDITIONING `#ffa931`,
LATENT `#ff9cf9`, IMAGE `#64b5f6`, MASK `#81c784`, and a hashed fallback for unknown types.

**Viewport restore vs auto-fit:** `set_graph` marks `pending_fit`; the resize callback fits once
the widget is allocated. `set_view` (saved `extra.ds`) clears that flag, so a restored viewport
always wins and auto-fit only happens for graphs without one.

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
