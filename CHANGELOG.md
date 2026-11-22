# Changelog

All notable changes to Comfy-GNOME. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The project is in **beta**: the `0.x` versions may change saved behaviour between releases, `1.0.0` is the first
version that promises stability.

## [0.2.0-beta.9] — 2026-11-22

### Fixed
- **Canvas died on workflows with an Impact Wildcard Processor** (middle-drag pan, node drag and left-drag pan all
  stopped): its computed combo list was reported to the app as "changed" on every page sync (hundreds of times,
  each one re-rendering the canvas and cancelling the drag, and marking the tab modified). The list is now sent once.
- Middle-drag pans even when the pointer is over an extension page element (LoRA Manager rows).
- **Text boxes for nodes the backend doesn't provide** (e.g. WAS `Text Multiline` while the pack failed to import):
  saved string values stay visible and editable instead of the node having no widgets.
- **Run rejected with "Value not in list"** for picker combos like Impact's "Select to add Wildcard": the saved pick
  is replaced by the placeholder the web UI sends, so the prompt validates.

- LoRA Manager loaders (and every other page-drawn DOM widget) no longer slide off their rows while panning, zooming or dragging a node: the WebView now lives in a `Shift` widget that, at snapshot time, scales/translates the last page frame the page actually painted onto the current canvas view (like a browser compositor), so it tracks motion in the same GTK frame. During node drags and below zoom 0.5 the canvas draws each row's saved value (prompt / "Search LoRAs to add..." placeholder, LoRA rows / "No LoRAs added") instead; the page re-lays out ~150 ms after motion stops.
- rgthree `Label` nodes draw as free text (fontSize / fontColor / backgroundColor / textAlign / padding) instead of opaque white boxes from their `#fff0` colour.

### Added (unreleased, not yet visually verified)
- **LoRA Manager in the node, tuned:** node text boxes now use the canvas' own font/inset/background instead of the page's; the LoRA autocomplete dropdown and its preview card show inside the graph (still at page scale, not zoomed); scrubbing a LoRA strength no longer re-renders the canvas on every mouse move, so it stops stuttering and no stale number is left behind.
- **`ComfyGNOME.LoraRows`** (Settings → Comfy-GNOME → Canvas): `page` keeps the extension's own editor in Lora Loader (LoraManager) like native ComfyUI; `canvas` restores Comfy-GNOME's simpler LoRA row list.
- DOM widgets inside canvas nodes: LoRA Manager's own prompt box + LoRA list now run in the node via a transparent WebKitGTK overlay (live-tested on Xvfb with the real backend). See `docs/WEBKITGTK.md`.

- Extension action-bar buttons/bottom panels as canvas-menu commands; outside-click replay closes extension menus; WebKit context menu hidden; mirror widgets get `widget.node` (Easy-Use lists work); DOM rows grow with the node and scroll internally.
- Link-render button beside the Log/Preview toggle (settings `ComfyGNOME.LinkButton`, `LinkButtonShow`).
- Resize grips: 16 px triangle on nodes and groups, `ComfyGNOME.ResizeSide` left/right, socket hover ring.
- Saved node sizes clamped to ComfyUI's minimum; the small GNOME size is kept in `ComfyGNOME.size`.
- LoRA strength scrubbing, Copy/Paste (Clipspace) on preview nodes, hollow dot on optional sockets.
- Browser drops: `http(s)` URLs, link text and raw picture data are accepted on the canvas and the window; the
  picture is fetched on a worker (ureq now built with `tls`) and goes to the backend's `input/` — no temp files.
- Extension sidebar tabs (`registerSidebarTab`) and nodes' DOM widgets (LoRA Manager's row editor) open from the
  canvas menus ("Extension commands", "Open node UI…") in a window that hosts the implant page.
- Extension commands and keybindings (`app.registerExtension`) exposed as shortcuts and a canvas-menu submenu.

### Added

- **Extension hook runtime (`src/implant/`)**: a hidden WebKitGTK page loads every custom node pack's `web/`
  JavaScript against a shim of the ComfyUI frontend API (`app`, `api`, `ComfyWidgets`, `LGraphNode`, extension
  registration), registers the backend's node definitions there, and mirrors the active tab's graph into it. Canvas
  edits are replayed into that page and whatever the extensions' own code changes comes back as a `changed`
  payload that is applied to the workflow — so extension behaviour is the extension's code, not a Rust copy of it.
- Custom-node **Settings sections come from their own code**: the Language selector and the EreNodes `.csv`
  autocomplete are real drop-downs with the node's own list, and any pack that registers a combo gets the same
  treatment without knowing Comfy-GNOME exists.
- Choices an extension computes **at runtime** are mirrored too: when it defines `options.values` as a getter
  (Impact Pack's wildcard databank), the canvas combo lists those entries. ImpactWildcard en/decoder show the real
  wildcards and insert them through the node's own callback.
- **LoRA Manager opens in the app** (`src/lora_view.rs`): a WebKitGTK window on `{base}/loras` replaces the
  Thorium/Chromium/Firefox launch and its per-display browser profile directory. Works under Flatpak.
- **Ctrl+wheel grows or shrinks the text in prompt widgets** (0.6×–2.5×, one notch = 0.1, wheel down = smaller).
  It applies to multiline/stretchy text rows only — the wrap, the row heights and the in-place editor all follow the
  same scale, and it resets when the app restarts.
- Files dropped anywhere on the window are accepted — sidebar, header, tab strip, splash — not only over an open
  canvas; they land where the active tab puts them, and images still upload into ComfyUI's `input/`.

### Changed

- Text widgets are live `<textarea>`s carrying `widget.inputEl`, so extensions that attach to the element
  (`TextAreaAutoComplete` in pysssss' custom-scripts, LoRA Manager's prompt box) work unchanged.
- The `api` shim implements `clientId` / `initialClientId`, which LoRA Manager's node registration requires.
- The video preview **keeps its mute state and volume** from one clip to the next: the setting is applied to each new
  media stream, and the transport bar records what the user changes.
- The autocomplete list in the in-place prompt editor is an **overlay child instead of a popover**, so the first
  click anywhere else now dismisses the suggestions and commits the text (a popover grabbed the pointer and only
  swallowed that click).

### Fixed

- Impact Pack's `Switch (Any)` grows an optional input when one of its inputs is connected.
- Escape closes the video preview even after a click on the video (the key handler now also sits on the window in
  the capture phase — the transport controls took the focus and ate the key).
- Settings no longer show **Modern Node Design (Nodes 2.0)**: it switches the web frontend's Vue renderer, which has
  no effect here, so offering it only invited confusion.
- Editing a LoRA Manager prompt in the canvas refreshes the LoRA rows and the TriggerWord Toggle node: the prompt
  callback posts to `/api/lm/loras/get_trigger_words`, the backend emits `trigger_word_update`, and the tag widget's
  value comes back into the workflow.

## [0.2.0-beta.8] — 2026-11-17

### Changed

- The **Clear queue** button now has a soft red backing.

### Fixed

- Clicking pause (or any transport control) on the video preview no longer closes it.
- Dropping a file from the file manager onto an open canvas now works (one drop target for files and in-app images).
- The pencil icon on the launch screen no longer shows a broken placeholder under the `gnome` icon theme.

## [0.2.0-beta.7] — 2026-11-16

### Added

- **Video and audio viewing**: outputs under `images`, `gifs` (VHS), `video` and `audio` are now picked up from
  live runs and history. mp4/webm/mkv/mov/avi and mp3/wav/flac/ogg/m4a open in the lightbox with a GStreamer
  player (controls, looping). The media browser lists them as icon tiles and plays them on double-click.
- **Animated gif / webp / apng** play in the lightbox instead of showing the first frame.
- **Image tools in the lightbox**: *Compare* (before/after slider against the next picture), *Crop*
  (drag a rectangle) and *Paint* (freehand, colour + size, undo). Crop and Paint can *Save As…* or *Use in
  Workflow* (uploads the result and adds a Load Image node).

## [0.2.0-beta.6] — 2026-11-16

### Added

- **Find in workflow (Ctrl+F)**: a browser-style find bar. It searches everything the workflow holds, not just
  node text boxes: node titles and types, ids, widget labels and values (prompts, notes, combos, numbers), socket
  names, properties and group titles, including inside subgraphs. All matching nodes are outlined, Enter /
  Shift+Enter (or Ctrl+G) step through them and the view jumps to each, entering subgraphs when needed.
- **Parameter tooltips**: hovering a widget shows the node definition's tooltip for that input.

## [0.2.0-beta.5] — 2026-11-16

### Added

- **Error reporting**: a failed run outlines the offending node in red with a `!` badge and lists the errors in a
  banner on the canvas (click a line to jump to the node, ✕ to dismiss). Rejected prompts (validation errors) are
  mapped to their nodes too.
- **Node tooltips**: hovering a node shows its description (or its error message).
- **Reroute nodes**: drawn as a compact pill; `Reroute` is available in the add-node menu even though the backend
  does not list it.
- **Minimap** (bottom-right): click or drag to pan; toggle with `Comfy.Minimap.Visible`.

## [0.2.0-beta.4] — 2026-11-16

Runs natively on Wayland (COSMIC); the Flatpak launches again.

### Fixed

- Clicks landed in the wrong place and the window did not fit the screen on Wayland desktops with fractional
  scaling (Pop!_OS COSMIC): the launchers forced `GDK_BACKEND=x11`, i.e. XWayland. GTK now picks Wayland by
  itself; `GDK_BACKEND=x11 ./launch-system.sh` still forces X11.
- The Flatpak did not start ("Failed to open display"): the manifest forced X11 while only the Wayland socket
  was available. The `--env=GDK_BACKEND=x11` finish-arg is gone.

## [0.2.0-beta.3] — 2026-11-09

Context menus, quick-search dismissal, group snapping, workflow folders and a narrower sidebar.

### Added

- **Right-click menus** for nodes, subgraphs, groups, the group-selection box and the canvas, matching
  `reference/Node Right-Click.png`, `Subgraph Right-Click.png`, `Group Right-Click.png`, `Group Select.png` and
  `Grid Right-Click.png`: node reload/convert/properties/queue/mode, subgraph edit/unpack/open, group rename /
  duplicate / colour / convert-to-subgraph / fit-to-nodes, arrange selection, palette picker, and the canvas
  cleanup / reboot / add-node / add-group / paste / pin / follow-execution actions (`src/workspace/menus.rs`).
- **Comfy-GNOME** settings has its own APPLICATION SETTINGS entry under *Comfy* and above *Lite Graph* (mono-white
  app icon), instead of living under *Appearance*.
- **Left sidebar colour** (Settings → Comfy-GNOME → Sidebar, `ComfyGNOME.SidebarTheme`): *Original* keeps
  libadwaita's grey, *Theme* paints the bar with the palette's menu colour (follows palette switches).

### Changed

- The left sidebar squeezes in smoother steps: the size classes carry 12 px of hysteresis (no flapping on a
  threshold), the drag writes a single fraction instead of three min/max widths, the bar can be narrowed to a
  160 px rail, and the search field hides below 200 px where it cannot be typed in.

### Fixed

- The quick node browser (double-click on empty canvas) now closes when clicking anywhere off its card — the
  canvas, the dimmed area, or another control.
- Groups snap to the grid when moved and resized (Shift or `Comfy.SnapToGrid.GridSize`), with the group's nodes
  riding along.
- Workflow subfolders from the backend's `userdata` listing appear in the sidebar as folder rows (collapsible),
  and a workflow stays visible while every ancestor folder is open (the list filter used to drop nested names).

## [0.2.0-beta.2] — 2026-11-09

Palettes, lightbox captions and the Settings dialog, on top of the properties panel.

### Added

- **Color palettes** — Settings → Appearance → *Color Palette* lists the frontend's Dark, Light, Solarized, Arc, Nord and
  Github palettes (`data/palettes/*.json`), the web UI's custom palettes (`Comfy.CustomColorPalettes`) and any
  ComfyUI-format JSON in `~/.config/comfy-gnome/themes/` (Import button copies one there). Applies to the canvas
  (background, grid, nodes, sockets, widgets) and recolours the GTK chrome. Stored in `Comfy.ColorPalette`.
  `src/theme.rs` is the single place to add another palette source.
- **Image preview file name** (Settings → Appearance → Comfy-GNOME): "name - 1/3" is drawn outside the picture, below
  (default) or above (`ComfyGNOME.PreviewCaption`).

### Changed

- Image preview is a scrim over the window: clicking the dark area (anywhere off the picture and its buttons) closes it.
- *Text Editor* setting (was "Text boxes") now reads "Legacy (pop-up editor)" / "Edit in place".

### Fixed

- Settings opened before the backend connected never got its APPLICATION SETTINGS (first "Loading…" forever, then an
  empty list): closing hides the window and reopening re-presented that stale copy. The dialog now waits for the
  backend and fills itself in, hidden or not.
- Settings could sit on "Loading…" forever: a panic in the definition scan (or a stalled backend) is now contained —
  a bad file is skipped, fetching has a 30 s budget, and a failed scan falls back to the stored keys with an error in the footer.
- Settings showed placeholder string names for core settings: the definition scan skipped the frontend chunk
  that now holds the core list. Real names, categories and dropdowns are back (Comfy, Lite Graph, Appearance, Mask Editor).

## [0.2.0-beta.1] — 2026-11-09

Properties panel, tracked against the ComfyUI 1.60.0 frontend (`src/components/rightSidePanel/`).

### Added

- **Properties panel** — "Toggle Properties Panel" in the toolbar opens a right sidebar with Parameters, Info and
  Settings tabs for the selected node.
  - Parameters: INPUTS and ADVANCED INPUTS sections with reload and collapse, promoted subgraph inputs, and a
    per-row ⋮ menu (Rename, Show input, Reset to default).
  - Info: node name, description, and Inputs / Outputs tables; shown only for a single plain node.
  - Settings: node state (Normal / Bypass / Mute), the ten litegraph node colours, and Pinned.
  - Subgraph instances get the editor view instead: SHOWN ON NODE with Hide all, HIDDEN / NESTED PARAMETERS with
    Show all, and an eye per row that promotes or demotes the input.
- Close button on the panel header; the panel header carries the window controls while the panel is open.

### Fixed

- The panel stopped following the canvas selection after a field in it had been focused.
- The subgraph editor closed itself on every refresh, and lost its scroll position and the ADVANCED INPUTS fold
  when a value was edited.
- The panel did not react to changes of title, node state, colour, Pinned, node properties, or values inside a
  subgraph.
- "Show all" and "Hide all" were swapped, and the hidden list omitted widgets without an input socket.
- The colour swatches never showed which colour was current.
- Node colour, background colour and edited properties were not written back on save; a focus-out with an
  unchanged value marked the workflow modified.
- Showing an input on an older-style subgraph (promotions kept in `properties.proxyWidgets`) put it at the top of
  the node instead of the end.

## [0.1.0] — 2026-11-09

Untagged baseline before the properties panel work (no release artifact): launch or attach to a ComfyUI backend, edit workflows in a cairo canvas, and browse the
backend's media, queue, templates and settings.

- Launch or attach: starts `main.py` with the checkout's `venv` and stops it on quit; browser-opening launchers are
  blocked; the Flatpak starts ComfyUI on the host through `flatpak-spawn --host`.
- Graph editing: typed wires, move / wire / de-anchor, rubber band and multi-select, add / duplicate / delete /
  collapse / bypass / mute, groups, snap to grid, link render modes, node resize.
- Widgets in place: numbers, combos, text, toggles, seeds with `control_after_generate`, subgraph-promoted
  widgets, rgthree Power Lora / Fast Groups Bypasser and tag-cloud widgets, each with its input socket.
- Subgraphs: drill in with a breadcrumb, rename the definition, clear a subgraph's workflow, `Esc` back.
- Prompt editor with `(word:1.05)` weighting.
- Node Library (≈4 700 nodes from the backend), MTB media browser with thumbnails and drag-to-canvas, queue with
  day groups and thumbnails, image lightbox, live TAESD / Latent2RGB previews and a progress strip.
- Templates, backend-driven Settings, faithful saving (an untouched workflow round-trips byte-identically).

[0.2.0-beta.3]: https://github.com/intosica/Comfy-GNOME/releases/tag/v0.2.0-beta.3
[0.2.0-beta.2]: https://github.com/intosica/Comfy-GNOME/releases/tag/v0.2.0-beta.2
[0.2.0-beta.1]: https://github.com/intosica/Comfy-GNOME/releases/tag/v0.2.0-beta.1
