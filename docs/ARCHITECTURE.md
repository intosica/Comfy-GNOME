# Architecture

## Layout
```
src/main.rs            adw::Application, window, Stack(splash → workspace)
src/splash.rs          launch configuration page → LaunchConfig
src/backend.rs         LaunchConfig, probe(), spawn_script() (+ browser shims), Backend::stop()
src/workspace.rs       OverlaySplitView: sidebar | header + Paned(canvas / log)
src/canvas/mod.rs      Canvas: DrawingArea, State, input controllers, cairo drawing
src/canvas/transform.rs View: zoom/pan math (unit-tested)
build-aux/*.json       Flatpak manifest;  data/*.desktop
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
- **Launch Script**: `sh -c 'cd "$1" && shift && PATH=<shims>:$PATH exec setsid "$0" "$@"' script dir args…`
  run with `gio::Subprocess` (stdout+stderr merged, read line-by-line async on the main loop into the log).
  - Working dir = script's directory (user scripts assume it).
  - `setsid` → own process group; `Backend::stop()` sends SIGTERM to `-pgid`, so python children die too.
  - **Browser blocking**: user scripts open Thorium/Chromium/xdg-open after starting ComfyUI. With the
    switch on, a shim dir under `$XDG_RUNTIME_DIR` is prepended to `PATH`: browser names → `exec sleep infinity`
    (scripts that run the browser in the foreground and kill ComfyUI when it exits keep waiting), openers
    (`xdg-open`, `gio`, …) → `exit 0`. Only works for bare-name invocations, not absolute paths.
  - Flatpak: prefixed with `flatpak-spawn --host --watch-bus`; shim dir is `$XDG_RUNTIME_DIR/app/<APP_ID>`,
    which is the same path on the host.

## Threading
GTK main thread owns all UI and state. Blocking I/O goes through `gio::spawn_blocking` (Phase 1) and will move to a
dedicated backend thread + `async_channel` in Phase 2 (Helvum's GTK↔worker message pattern).
