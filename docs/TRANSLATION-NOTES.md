# Translation Notes (Helvum → GNOME-ComfyUI)

Helvum (`/home/montel/Documents/helvum-main`, GPL-3.0-only, © Tom A. Wagner et al.) was the reference.
Visual constants come from ComfyUI_frontend 1.60.0 litegraph (`/home/montel/Documents/ComfyUI_frontend-1.60.0`, GPL-3.0).

## Helvum's model (`src/ui/graph/`)
- `GraphView` (`graph_view.rs`): custom `gtk::Widget` implementing `Scrollable`. Nodes are child widgets
  (`Node`, `Port` subclasses); positions in `HashMap<Node, Point>` in canvas space.
- Children allocated with `gsk::Transform::translate(-hadj, -vadj).scale(zoom)`; inverse for screen→canvas.
- `snapshot()`: culls nodes outside the allocation, draws links through `snapshot.append_cairo`.
- Input: primary `GestureDrag` picks the widget under the pointer → node drag with grab offset in canvas units;
  middle-button drag pans through the adjustments; Ctrl+scroll zooms in 0.1 steps; `GestureZoom` for pinch;
  zoom anchored so the point under the cursor stays fixed. Zoom 0.3–4.0, canvas clamped to 5000.
- Ports use `DragSource`/`DropTarget` + `DropControllerMotion` to draw the in-progress link.
- Threading: GTK thread ↔ PipeWire thread via channels (`GtkMessage` / `PipewireMessage`).
- Flatpak: `build-aux/org.pipewire.Helvum.json` — GNOME runtime, rust-stable extension, `--socket=fallback-x11`.

## What we took
| Helvum | GNOME-ComfyUI |
|---|---|
| Link Bezier: `half = |ox-ix|/2`, `off = max(0, 25-|oy-iy|)` if `ox > ix`; ctrl `(ox+half, oy-off)`, `(ix-half, iy-off)` | `canvas::link_path` (identical) |
| Zoom-around-pointer math | `transform::View::zoom_at` (clamp 0.1–4.0 instead of 0.3–4.0) |
| Drag-picks-topmost-node, grab offset in canvas units | `Canvas::setup_input` primary drag |
| Middle drag = pan | same, plus primary drag on empty space pans (ComfyUI behaviour) |
| GTK↔worker thread message enums | planned for Phase 2 backend thread |
| Flatpak manifest shape, `fallback-x11` | `build-aux/io.github.gnomecomfyui.GnomeComfyUI.json` |

## What we deliberately did not take
- Widget-per-node / `Scrollable` + adjustments: replaced by an immediate-mode DrawingArea (see ARCHITECTURE).
- GObject subclassing boilerplate: plain structs.
- DnD-based port linking: will be manual hit-testing in the canvas (Phase 3).

## litegraph constants used
`NODE_TITLE_HEIGHT=30`, `NODE_SLOT_HEIGHT=20`, slot y `(i+0.7)*20` (`slotCalculations.ts`),
title `#999`, node `#333`, body `#353535`, box `#666`, link `#9A9` (`LiteGraphGlobal.ts`).
