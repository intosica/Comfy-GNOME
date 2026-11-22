# WebKitGTK in Comfy-GNOME — central hub

Everything that uses WebKitGTK lives here. There are two WebViews, both created in Rust (`webkit6` crate).

| WebView | Code | Purpose |
|---|---|---|
| **Implant** (hidden page) | `src/implant/mod.rs`, `src/implant/shim.js` | Runs custom nodes' own frontend JS against a shim of the ComfyUI `app`/`api`/`LiteGraph` API and mirrors the result into the Rust canvas. |
| **LoRA Manager viewer** | `src/lora_view.rs` | Window showing the LoRA Manager SPA served by the backend (replaces launching a browser). |

(`webkitgtk-master.zip` in GNOME Resources is a Go/purego binding — irrelevant here.)

## Implant → canvas contract
- JS → Rust: `webkit.messageHandlers.cg.postMessage(json)`, `t` = `log | ready | changed | dom | hits`.
  - `ready`: extensions, settings, `commands`, `keybindings`, `sidebars`.
  - `changed`: node inputs/outputs/widget values/links an extension hook changed.
  - `dom`: `{"id|type": [[widgetName, minHeight, widgetIndex]]}` — nodes with DOM widgets.
  - `hits`: screen rects of page-drawn elements (pointer routing).
- Rust → JS: `eval` of `cg.*` (`sync`, `layout`, `runCommand`, `showPanel`, `hidePanel`, `applySetting`).

## DOM widgets inside canvas nodes (`addDOMWidget`)
1. `shim.js` `isDom(w)`: widget element is an `Element` other than the plain text box `inputEl`. Reported via `dom`.
2. Rust (`graph_view::dom_rows`) replaces the canvas stand-in (same name, or same widget index; LoRA Manager's `lm:`/`lm+` rows) with `WKind::Dom(h)`, a reserved, non-interactive row.
3. Each canvas draw calls `implant::layout` with the row's screen rect; `cg.layout` puts the real element in a fixed `#cg-layer` at that rect, `transform: scale(zoom)`. Skipped below zoom 0.5 (`hideOnZoom`), off-screen, or covered by a later node.
4. The WebView is a transparent `gtk::Overlay` child above the canvas, `can_target=false`. `implant::pointer` (capture-phase motion controller on the overlay) turns targeting on only over `hits` rects, so the canvas keeps every other event. A held button keeps its target (slider dragged off its row).
5. Edits inside a widget (`pointerup/change/input/keyup`) → `poke` → `changed` → written to the workflow, so LoRA Manager's `loras` array and prompt text both save.
6. Page hygiene: body children not in `#cg-layer`/`#cg-panel` are hidden (`visibility:hidden`) unless added right after a DOM widget was used (menus/tooltips, `data-cg-ok`). `html,body` transparent, `overflow:hidden`; the layer's `scroll` is pinned to 0 because focusing an input otherwise scrolls the layer and shifts every widget.
7. Mirror widgets carry a non-enumerable `widget.node` (`own(w)` in the shim) — Easy-Use's Vue style list needs it. DOM rows grow with the node (`relayout`, equal share of spare height, like litegraph); an element with `overflow: visible` gets `overflow:auto` so tall content scrolls inside its row instead of spilling over nodes below.
8. Canvas presses (capture `GestureClick` on `tabs`, not an ancestor of the page) call `implant::outside_click` → `cg.outside()`: blur + replay pointer/mouse/click on `<html>`, which closes extensions' open menus exactly like a browser. WebKit's own context menu is suppressed (`connect_context_menu`); page `contextmenu` handlers (LoRA Manager row menu) still run.
9. Keyboard: `Workspace::typing()` is true while a WebView has focus, so canvas shortcuts don't fire.
10. Text boxes from DOM widgets use the canvas' own look: `shim.js` styles `#cg-layer textarea` (Sans 12 px / 14 px pitch / 8 px inset, `#222` on `#ddd`) to match `canvas/widgets.rs`, so a page text box is not a different colour or font from every canvas row.
11. Performance: a `poke` (and therefore a full `on_hooks` re-render, ~35 ms) is skipped while a pointer is held down in the page — scrubbing a LoRA strength no longer re-renders the canvas on every mouse move; one sync follows the release. `CG_IMPLANT_DEBUG=1` prints `on_hooks took …` and shim `report … ms` for anything above 3 ms.
12. Popups that live in `<body>` and are only shown on demand (LoRA Manager's autocomplete dropdown and its preview card) are revealed when their `style`/`class` flips from hidden right after a widget was used (`watchShow`), with `cg-*` ids skipped so Crystools' monitor bar never leaks.
13. `ComfyGNOME.LoraRows` (`page` | `canvas`, Comfy-GNOME settings → Canvas) picks between the extension's own LoRA Manager editor in the node (default, like native ComfyUI) and our plain canvas row list (`graph_view::set_lm_page`).

Verified live (Xvfb + real backend, `loraholicsSimpleTo_v12`, plus Easy-Use `easy stylesSelector`, tab switch round-trip, LM row context menu + outside-click close, LM autocomplete dropdown + preview card, strength drag 0.80 → 0.98 with 13 syncs instead of 27, `ComfyGNOME.LoraRows=canvas` startup): LoRA Manager's prompt box and LoRA list in both loaders; typing `<lora:NAME:0.8>` creates a row; `<`/`>` strength arrows change the row and the prompt; `changed` carries both widget 1 (text) and widget 2 (loras).

## Extension surface outside nodes
- Commands/keybindings: empty-canvas menu "Extension commands" + shortcuts (`workspace.rs::install_extension_commands`).
- Sidebar tabs: `implant::open_panel` moves the WebView into an `adw::Window` and calls the tab's `render()`; closing reattaches it to the overlay.
- Action-bar buttons (`actionBarButtons`) appear as `btn:<ext>:<i>` commands; `bottomPanelTabs` with `render` join the sidebar-style window panels.
- Not surfaced as real widgets: toolbar buttons are menu entries; sidebars/panels open as windows, not docked.

## Debugging
`CG_IMPLANT_DEBUG=1` prints `changed`, `dom`, `hits` and pointer-routing lines. Shim load errors appear as `[implant] error:`.
`CG_IMPLANT_EVAL=<file>` runs that JS (an async function body; `const app = window.comfyAPI.app.app;`) in the page every 500 ms and logs `[implant] eval-file:`.
Test recipe: Xvfb `:99` + `venv/bin/python main.py --port 8188 --cpu`, app with `DBUS_SESSION_BUS_ADDRESS=disabled:` and a private `XDG_CONFIG_HOME` (delete its `session.json` for a repeatable viewport). Kill all three afterwards.

## Known gaps

- The page lags the canvas by a few frames (separate web process, async `evaluate_javascript`). `implant::Shift` (custom gtk widget in the HOME overlay, holds the WebView) draws the page transformed (clip, translate, scale; `XFORM`) so the last PAINTED layout lines up with the current canvas view in the same frame. Layouts are sent as `cg.layout(items, seq)` (150 ms settle timer re-armed while moving); the page paints a 1x1 `#cg-seq` stamp (rgb(seq&255, seq>>8, 90)) and `Shift::snapshot` reads it back through a private software `CairoRenderer` (`PROBE`) to learn which seq is on screen (the page's own "done" message is unreliable). If a row's canvas rect changes (node drag) no transform exists: page rows are hidden and `widgets::draw_dom_fallback` draws stand-ins; `implant::covers` suppresses stand-ins while the page shows the row. Canvas redraws requested from inside a snapshot go through `redraw_area()` (idle) because GTK drops them. Below zoom 0.5 the stand-in stays; below 0.4 the canvas skips all text (LOD).
- Only the active tab's DOM widgets exist; zoom < 0.5 hides them (by design).
- Pointer events over a DOM widget are decided from the last motion event; very fast clicks without motion may reach the canvas.
- Extension stray DOM (Manager menus, Krita buttons…) is hidden, not hosted.
