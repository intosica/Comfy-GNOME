# Changelog

All notable changes to Comfy-GNOME. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The project is in **beta**: the `0.x` versions may change saved behaviour between releases, `1.0.0` is the first
version that promises stability.

## [Unreleased]

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

- Settings opened while the backend was still connecting stayed on "Loading…" forever; the dialog now lists the stored
  state at once and reloads with the real definitions as soon as the backend answers.
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

[0.2.0-beta.1]: https://github.com/intosica/Comfy-GNOME/releases/tag/v0.2.0-beta.1
