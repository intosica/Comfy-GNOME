# Changelog

All notable changes to Comfy-GNOME. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The project is in **beta**: the `0.x` versions may change saved behaviour between releases, `1.0.0` is the first
version that promises stability.

## [Unreleased]

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
