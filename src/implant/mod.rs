// SPDX-License-Identifier: GPL-3.0-only
//! The implant: one hidden, never-drawn WebKit page that runs every custom-node frontend
//! extension unchanged (`shim.js` provides `window.comfyAPI`, the seam the backend's
//! `/scripts/*.js` re-export from). Comfy-GNOME keeps all drawing; the implant only answers
//! "what did the extensions register / do" so native widgets can show it. Main thread only.

use adw::prelude::*;
use gtk::glib;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use webkit::prelude::*;

#[derive(Debug, Default)]
pub struct Ready {
    /// Serialized `addSetting` / `registerExtension({settings})` definitions, options resolved.
    pub settings: Vec<Value>,
    /// Setting id → options for the core refs passed to [`start`].
    pub core_options: Map<String, Value>,
    /// Extension commands `{id,label,ext}` and their keybindings `{id,key,ctrl,alt,shift}`.
    pub commands: Vec<Value>,
    pub keybindings: Vec<Value>,
    /// Extension sidebar tabs `{id,title}`, shown through [`open_panel`].
    pub sidebars: Vec<Value>,
}

struct Implant {
    view: webkit::WebView,
    ready: Rc<RefCell<Option<Rc<Ready>>>>,
}

/// The graph last sent for the mirrored tab: `(key, graph_json)`.
type Last = Option<(String, Value)>;
type OnChange = Box<dyn Fn(&str, &Value)>;

thread_local! {
    /// Mirror node id → its DOM widgets `(name, min height)`, see `isDom` in shim.js.
    static DOM: RefCell<std::collections::HashMap<String, Vec<(String, f64, usize)>>> = RefCell::default();
    static ON_DOM: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    /// What the page currently draws (window-relative px of the view): pointer goes there.
    static HITS: RefCell<Vec<[f64; 4]>> = const { RefCell::new(Vec::new()) };
    /// The overlay the view lives in while it draws DOM widgets over the canvas.
    static HOME: RefCell<Option<gtk::Overlay>> = const { RefCell::new(None) };
    static LAYOUT: RefCell<String> = const { RefCell::new(String::new()) };
    /// The page's container in [`HOME`]: draws it moved/scaled by [`XFORM`], see [`layout`].
    static SHIFT: RefCell<Option<Shift>> = const { RefCell::new(None) };
    /// Layouts sent but not yet seen on screen (the page stamps `seq` into its pixel 0,0), oldest first.
    static SENT: RefCell<std::collections::VecDeque<(u64, Frame)>> = RefCell::default();
    static SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The layout the page has on screen, and the canvas' current one.
    static PAINTED: RefCell<Option<Frame>> = const { RefCell::new(None) };
    static CUR: RefCell<Option<Frame>> = const { RefCell::new(None) };
    /// Page → screen: `(scale, dx, dy)`; None hides the page (it shows rows that moved apart).
    static XFORM: std::cell::Cell<Option<(f64, f64, f64)>> = const { std::cell::Cell::new(None) };
    static AREA: RefCell<Option<gtk::Widget>> = const { RefCell::new(None) };
    /// The canvas stopped moving: send it its layout ([`layout`]).
    static SETTLE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };

    static IMPLANT: RefCell<Option<Implant>> = const { RefCell::new(None) };
    static LAST: RefCell<Last> = const { RefCell::new(None) };
    /// A sync asked for before the page finished loading; sent once it's ready.
    static PENDING: RefCell<Last> = const { RefCell::new(None) };
    static ON_CHANGE: RefCell<Option<OnChange>> = const { RefCell::new(None) };
}

/// (Re)start against `base` (`http://host:port`). `core`: `(id, url, export)` options to evaluate.
pub fn start(base: &str, core: &[(String, String, String)]) {
    stop();
    let ucm = webkit::UserContentManager::new();
    ucm.register_script_message_handler("cg", None);
    let ready: Rc<RefCell<Option<Rc<Ready>>>> = Rc::default();
    let r = ready.clone();
    ucm.connect_script_message_received(Some("cg"), move |_, v| {
        let Ok(m) = serde_json::from_str::<Value>(&v.to_str()) else { return };
        match m["t"].as_str() {
            Some("log") => eprintln!("[implant] {}: {}", m["lvl"].as_str().unwrap_or(""), m["msg"].as_str().unwrap_or("")),
            Some("ready") => {
                let settings = m["settings"].as_array().cloned().unwrap_or_default();
                eprintln!("[implant] ready: {} extensions, {} settings", m["extensions"].as_array().map_or(0, Vec::len), settings.len());
                let core_options = m["coreOptions"].as_object().cloned().unwrap_or_default();
                let list = |k: &str| m[k].as_array().cloned().unwrap_or_default();
                r.replace(Some(Rc::new(Ready { settings, core_options, commands: list("commands"), keybindings: list("keybindings"), sidebars: list("sidebars") })));
                if let Some((key, g)) = PENDING.with(|p| p.take()) {
                    sync(&key, g);
                }
            }
            Some("dom") => {
                if std::env::var_os("CG_IMPLANT_DEBUG").is_some() {
                    eprintln!("[implant] dom: {}", m["nodes"]);
                }
                let map = m["nodes"].as_object().map(|o| {
                    o.iter().map(|(id, ws)| (id.clone(), ws.as_array().into_iter().flatten().filter_map(|w| Some((w[0].as_str()?.to_owned(), w[1].as_f64()?, w[2].as_u64().unwrap_or(u64::MAX) as usize))).collect())).collect()
                });
                DOM.with(|d| d.replace(map.unwrap_or_default()));
                ON_DOM.with(|f| if let Some(f) = f.borrow().as_ref() { f() });
            }
            Some("hits") if std::env::var_os("CG_IMPLANT_DEBUG").is_some() && { eprintln!("[implant] hits: {}", m["rects"]); false } => {}
            Some("hits") => drop(HITS.with(|h| h.replace(m["rects"].as_array().into_iter().flatten().filter_map(|r| Some([r[0].as_f64()?, r[1].as_f64()?, r[2].as_f64()?, r[3].as_f64()?])).collect()))),
            // Extension hooks reshaped nodes of the mirrored tab.
            Some("changed") => {
                let key = m["key"].as_str().unwrap_or_default().to_owned();
                if std::env::var_os("CG_IMPLANT_DEBUG").is_some() {
                    eprintln!("[implant] changed: {}", m);
                }
                ON_CHANGE.with(|f| {
                    if let Some(f) = f.borrow().as_ref() {
                        f(&key, &m);
                    }
                });
            }
            _ => {}
        }
    });
    let view: webkit::WebView = glib::Object::builder().property("user-content-manager", &ucm).build();
    // WebKit's own Back/Forward/Reload menu means nothing here; page `contextmenu` handlers still run.
    view.connect_context_menu(|_, _, _| true);
    if let Some(s) = WebViewExt::settings(&view) {
        // Only DOM widgets / panels are shown: no GPU surface, no media, no plugins.
        s.set_hardware_acceleration_policy(webkit::HardwareAccelerationPolicy::Never);
        s.set_enable_webgl(false);
        s.set_enable_media(false);
        s.set_enable_webaudio(false);
        s.set_enable_page_cache(false);
    }
    let core: Vec<Value> = core.iter().map(|(id, url, ex)| serde_json::json!({"id": id, "url": url, "export": ex})).collect();
    let boot = format!(
        "<!doctype html><meta charset=utf-8><script>{}</script><script type=module>cg.start({}).catch(e=>console.error('start',e))</script>",
        include_str!("shim.js"),
        Value::Array(core)
    );
    // The base URL makes the page same-origin with the backend: extension `import`s and
    // `fetch("/api/…")` resolve exactly as in the browser.
    view.load_html(&boot, Some(&format!("{}/", base.trim_end_matches('/'))));
    // Headless debugging: JS written to $CG_IMPLANT_EVAL runs in the page, result to stderr.
    if let Some(path) = std::env::var_os("CG_IMPLANT_EVAL") {
        let v = view.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            let Some(v) = v.upgrade() else { return glib::ControlFlow::Break };
            if let Ok(js) = std::fs::read_to_string(&path) {
                let _ = std::fs::remove_file(&path);
                let js = format!("(async()=>{{try{{return JSON.stringify(await (async()=>{{{js}}})())}}catch(e){{return 'ERR '+e}}}})()");
                v.call_async_javascript_function(&format!("return await {js}"), None, None, None, None::<&gtk::gio::Cancellable>, |r| match r {
                    Ok(v) => eprintln!("[implant] eval-file: {}", v.to_str()),
                    Err(e) => eprintln!("[implant] eval-file error: {e}"),
                });
            }
            glib::ControlFlow::Continue
        });
    }
    IMPLANT.with(|i| i.replace(Some(Implant { view, ready })));
    if let Some(o) = HOME.with(|h| h.borrow().clone()) {
        attach(&o);
    }
}

pub fn stop() {
    LAST.with(|l| l.take());
    LAYOUT.with(|l| l.take());
    SENT.with(|l| l.take());
    PAINTED.with(|l| l.take());
    XFORM.with(|x| x.set(None));
    if let Some(id) = SETTLE.with(|t| t.take()) {
        id.remove();
    }
    HITS.with(|l| l.take());
    DOM.with(|l| l.take());
    PENDING.with(|l| l.take());
    if let Some(i) = IMPLANT.with(|i| i.take()) {
        i.view.terminate_web_process();
        if i.view.parent().is_some_and(|p| p.is::<Shift>()) {
            i.view.unparent();
        }
    }
}

/// The extensions' registrations, once they've all loaded; None if it doesn't get there in time.
pub async fn ready() -> Option<Rc<Ready>> {
    // ponytail: 20 s budget, polled; a notify channel if anything ever needs it sooner.
    for _ in 0..200 {
        let r = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.ready.borrow().clone()))?;
        if r.is_some() {
            return r;
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    None
}

fn eval(js: String) {
    IMPLANT.with(|i| {
        if let Some(i) = i.borrow().as_ref() {
            i.view.evaluate_javascript(&js, None, None, None::<&gtk::gio::Cancellable>, |r| {
                if let Err(e) = r {
                    eprintln!("[implant] eval: {e}");
                }
            });
        }
    });
}

/// The settings page stored a new value: run the extension's `onChange` like the web UI does.
pub fn apply_setting(id: &str, v: &Value) {
    eval(format!("cg.applySetting({},{})", Value::from(id), v));
}

/// Run an extension command by id (menu entry or keybinding).
pub fn run_command(id: &str) {
    eval(format!("cg.runCommand({})", Value::from(id)));
}

/// Where hook-made changes go: `(key, {nodes, links})`, see `cg` `report` in shim.js.
pub fn on_change(f: impl Fn(&str, &Value) + 'static) {
    ON_CHANGE.with(|c| c.replace(Some(Box::new(f))));
}

/// The tab `key` now has `graph` ([`crate::workflow::graph_json`]): replay the difference
/// on the mirror (or load it, for another tab) so the extensions' hooks see the edit.
pub fn sync(key: &str, graph: Value) {
    if IMPLANT.with(|i| i.borrow().as_ref().is_none_or(|i| i.ready.borrow().is_none())) {
        PENDING.with(|p| p.replace(Some((key.to_owned(), graph))));
        return;
    }
    let pre = LAST.with(|l| match l.borrow().as_ref() {
        Some((k, g)) if k == key => Some(g.clone()),
        _ => None,
    });
    if pre.as_ref() == Some(&graph) {
        return;
    }
    eval(format!("cg.sync({},{},{})", Value::from(key), pre.unwrap_or(Value::Null), graph));
    LAST.with(|l| l.replace(Some((key.to_owned(), graph))));
}

/// The extensions are loaded and following a tab: their hooks own what they implement.
pub fn live() -> bool {
    IMPLANT.with(|i| i.borrow().as_ref().is_some_and(|i| i.ready.borrow().is_some())) && LAST.with(|l| l.borrow().is_some())
}

/// The canvas took a reported change: that's what the mirror has, nothing to replay.
pub fn applied(key: &str, graph: Value) {
    LAST.with(|l| l.replace(Some((key.to_owned(), graph))));
}

/// The next [`sync`] reloads the mirror (undo, a graph replaced wholesale).
pub fn forget() {
    LAST.with(|l| l.take());
}

/// A backend websocket text frame, for `api.addEventListener` hooks.
pub fn ws(raw: &str) {
    if IMPLANT.with(|i| i.borrow().as_ref().is_some_and(|i| i.ready.borrow().is_some())) {
        eval(format!("cg.ws({})", Value::from(raw)));
    }
}

/// Show an extension's sidebar tab: the implant page itself moves into a window for as long as
/// it is open (the page is the only place the extension's DOM exists).
pub fn open_panel(parent: Option<&gtk::Window>, id: &str, title: &str) {
    show_in_window(parent, format!("cg.showPanel({})", Value::from(id)), title, 420);
}

/// DOM widgets the extensions put on node `id` (of type `ty`) of the mirrored graph.
/// ponytail: keyed by id+type, not by tab; another tab with the same id+type borrows the rows
/// (the page only draws the mirrored tab's elements, the others show the placeholder).
/// `(name, height, index in node.widgets)` of the node's DOM widgets.
pub fn dom_widgets(id: &str, ty: &str) -> Vec<(String, f64, usize)> {
    DOM.with(|d| d.borrow().get(&format!("{id}|{ty}")).cloned().unwrap_or_default())
}

/// The DOM-widget set changed: the active tab must rebuild its rows.
pub fn on_dom(f: impl Fn() + 'static) {
    ON_DOM.with(|c| c.replace(Some(Box::new(f))));
}

/// Put the page over the canvas area: it is transparent and only takes the pointer where it
/// draws something ([`pointer`]), so the canvas underneath works as before.
pub fn attach(overlay: &gtk::Overlay) {
    HOME.with(|h| h.replace(Some(overlay.clone())));
    let Some(view) = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.view.clone())) else { return };
    view.set_background_color(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    view.set_can_target(true);
    let shift = SHIFT.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_none_or(|w| w.parent().as_ref() != Some(overlay.upcast_ref())) {
            let w: Shift = glib::Object::new();
            w.set_can_target(false);
            overlay.add_overlay(&w);
            overlay.set_measure_overlay(&w, false);
            overlay.set_clip_overlay(&w, true);
            *s = Some(w);
        }
        s.clone().unwrap()
    });
    if view.parent().is_none() {
        view.set_parent(&shift);
    }
    LAYOUT.with(|l| l.take());
    PAINTED.with(|l| l.take());
    XFORM.with(|x| x.set(None));
    if let Some(id) = SETTLE.with(|t| t.take()) {
        id.remove();
    }
}

fn on_canvas(view: &webkit::WebView) -> bool {
    view.parent().is_some_and(|p| p.is::<Shift>())
}

fn shift() -> Option<Shift> {
    SHIFT.with(|s| s.borrow().clone())
}

/// Pointer at `(x, y)` in the overlay: the page takes events only over its own elements.
/// `held`: a button is down — keep whoever has the press (a slider dragged off its row).
pub fn pointer(x: f64, y: f64, held: bool) {
    let Some(view) = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.view.clone())) else { return };
    if held {
        return;
    }
    let Some(sh) = shift() else { return };
    // Mid-motion the page is drawn moved: its hit rects are only right once it caught up.
    let still = XFORM.with(|t| t.get()).is_some_and(|(k, dx, dy)| k == 1.0 && dx.abs() < 0.5 && dy.abs() < 0.5);
    let on = still && on_canvas(&view) && HITS.with(|h| h.borrow().iter().any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]));
    if on != sh.can_target() && std::env::var_os("CG_IMPLANT_DEBUG").is_some() {
        eprintln!("[implant] pointer {x:.0},{y:.0} -> page {on}");
    }
    sh.set_can_target(on);
}

/// A press landed on the canvas, not on the page: let the page see an outside click
/// (closes the extension's open menus, blurs its text box) like it would in a browser.
pub fn outside_click() {
    let Some(view) = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.view.clone())) else { return };
    if on_canvas(&view) && !shift().is_some_and(|s| s.can_target()) {
        eval("cg.outside()".into());
    }
}

/// The page currently shows this DOM widget on its row: the canvas must not draw its
/// stand-in under it (LoRA Manager's list is translucent, both texts would show).
pub fn covers(node: &str, widget: &str) -> bool {
    XFORM.with(|x| x.get()).is_some() && PAINTED.with(|p| p.borrow().as_ref().is_some_and(|p| p.rows.iter().any(|r| r.0 == node && r.1 == widget)))
}

/// One layout: the canvas view (`origin` in overlay px, `zoom`) and each row's canvas rect.
#[derive(Clone)]
struct Frame {
    origin: (f64, f64),
    zoom: f64,
    rows: Vec<(String, String, [f64; 4])>,
    /// `cg.layout` argument for it.
    js: String,
}

/// How the page has to move to line up with the canvas. The page lags the canvas (another
/// process, async eval), so while the canvas pans or zooms the page is *not* re-laid out: the
/// layout it shows is moved/scaled onto the current view at snapshot time ([`Shift`]), in the
/// same GTK frame as the canvas, like a browser compositor scrolls. Which layout that is, the
/// page's frame itself says (pixel (0,0) stamped with its number, read back in `Shift`), so
/// a new layout ([`settle`], once the canvas stands still) takes over exactly when it shows.
/// Hidden only when rows moved relative to each other (node drag/resize): the canvas then
/// draws the values until the page caught up.
fn compute() -> Option<(f64, f64, f64)> {
    CUR.with(|c| {
        PAINTED.with(|p| {
            let (c, p) = (c.borrow(), p.borrow());
            let (Some(c), Some(p)) = (c.as_ref(), p.as_ref()) else { return None };
            if c.rows.is_empty() {
                return None; // zoomed below `hideOnZoom` / nothing to show
            }
            let moved = c.rows.iter().any(|(n, w, r)| p.rows.iter().any(|(pn, pw, pr)| pn == n && pw == w && r.iter().zip(pr).any(|(a, b)| (a - b).abs() > 0.01)));
            if moved {
                return None;
            }
            let k = c.zoom / p.zoom;
            Some((k, c.origin.0 - p.origin.0 * k, c.origin.1 - p.origin.1 * k))
        })
    })
}

/// Recompute [`XFORM`]; true when it changed between shown and hidden (the canvas' stand-ins
/// must follow, see [`covers`]).
fn recompute() -> bool {
    let x = compute();
    let was = XFORM.with(|t| t.replace(x));
    if was != x {
        if let Some(s) = shift() {
            s.queue_draw();
        }
    }
    was.is_some() != x.is_some()
}

/// Send the canvas' current layout to the page.
fn settle() {
    let Some(f) = CUR.with(|c| c.borrow().clone()) else { return };
    if LAYOUT.with(|l| *l.borrow() == f.js) {
        return;
    }
    LAYOUT.with(|l| l.replace(f.js.clone()));
    let seq = SEQ.with(|s| {
        // 16 bits fit the stamp; 0 = never stamped.
        s.set(s.get() % 0xffff + 1);
        s.get()
    });
    let js = format!("cg.layout({},{seq})", f.js);
    if std::env::var_os("CG_IMPLANT_DEBUG").is_some() {
        eprintln!("[implant] send {seq}");
    }
    SENT.with(|s| {
        let mut s = s.borrow_mut();
        s.push_back((seq, f));
        if s.len() > 16 {
            s.pop_front(); // the page stopped answering: don't grow forever
        }
    });
    eval(js);
}

/// The page frame about to be drawn is stamped `seq`: it shows that layout from now on.
fn shown(seq: u64) {
    let f = SENT.with(|s| {
        let mut s = s.borrow_mut();
        let i = s.iter().position(|(n, _)| *n == seq)?;
        let f = s[i].1.clone();
        s.drain(..=i);
        Some(f)
    });
    let Some(f) = f else { return };
    PAINTED.with(|p| p.replace(Some(f)));
    let x = compute();
    if XFORM.with(|t| t.replace(x)).is_some() != x.is_some() {
        redraw_area(); // stand-ins under the rows the page shows now go, the others come
    }
}

/// Redraw the canvas next frame. Never `queue_draw` it from inside a snapshot (ours or the
/// canvas' own): GTK clears the ancestors' `draw_needed` after it, the request is lost and
/// the canvas never redraws again.
fn redraw_area() {
    if let Some(a) = AREA.with(|a| a.borrow().clone()) {
        glib::idle_add_local_once(move || a.queue_draw());
    }
}

fn pending() -> bool {
    SENT.with(|s| !s.borrow().is_empty())
}

/// The canvas `area` is about to draw with view `(offset, zoom)` and these DOM widget rows
/// `(node id, widget name, x, y, w, h)` (x/y area px, w/h canvas units): line up what the page
/// shows with this frame, and give the page the new layout once the view stands still.
pub fn layout(area: &gtk::Widget, rows: Vec<(String, String, f64, f64, f64, f64)>, offset: (f64, f64), zoom: f64) {
    let Some(view) = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.view.clone())) else { return };
    let Some(sh) = shift().filter(|_| on_canvas(&view)) else { return };
    let off = area.compute_point(&sh, &gtk::graphene::Point::new(0.0, 0.0)).map_or((0.0, 0.0), |p| (p.x() as f64, p.y() as f64));
    let origin = (offset.0 + off.0, offset.1 + off.1);
    let rows: Vec<_> = rows.iter().map(|(n, w, x, y, rw, rh)| (n.clone(), w.clone(), [(x - offset.0) / zoom, (y - offset.1) / zoom, *rw, *rh])).collect();
    // Page coords are exactly `canvas * zoom + origin`: the transform in `recompute` relies on it.
    let items: Vec<Value> = rows.iter().map(|(n, w, r)| serde_json::json!([n, w, r[0] * zoom + origin.0, r[1] * zoom + origin.1, r[2], r[3], zoom])).collect();
    let js = Value::Array(items).to_string();
    let changed = LAYOUT.with(|l| *l.borrow() != js);
    // Still moving (this frame differs from the last one): the settle timer starts over.
    let moving = CUR.with(|c| c.borrow().as_ref().is_none_or(|c| c.js != js));
    CUR.with(|c| c.replace(Some(Frame { origin, zoom, rows, js })));
    AREA.with(|a| a.replace(Some(area.clone())));
    if moving || !changed {
        if let Some(id) = SETTLE.with(|t| t.take()) {
            id.remove();
        }
    }
    if changed && SETTLE.with(|t| t.borrow().is_none()) {
        if PAINTED.with(|p| p.borrow().is_none()) && !pending() {
            settle(); // first layout: nothing to move yet
        } else {
            let t = glib::timeout_add_local_once(Duration::from_millis(150), || {
                SETTLE.with(|t| t.take());
                settle();
            });
            SETTLE.with(|s| s.replace(Some(t)));
        }
    }
    if recompute() {
        redraw_area(); // next frame: stand-ins follow
    }
}

fn show_in_window(parent: Option<&gtk::Window>, js: String, title: &str, width: i32) {
    let Some(view) = IMPLANT.with(|i| i.borrow().as_ref().map(|i| i.view.clone())) else { return };
    if view.parent().is_some() && !on_canvas(&view) {
        return; // already in a window
    }
    if on_canvas(&view) {
        view.unparent();
    }
    view.set_can_target(true);
    LAYOUT.with(|l| l.take());
    eval(js);
    let win = adw::Window::builder().title(title).default_width(width).default_height(720).build();
    if let Some(p) = parent {
        win.set_transient_for(Some(p));
    }
    let tv = adw::ToolbarView::builder().content(&view).build();
    tv.add_top_bar(&adw::HeaderBar::new());
    win.set_content(Some(&tv));
    win.connect_close_request(move |w| {
        eval("cg.hidePanel()".into());
        w.set_content(None::<&gtk::Widget>);
        if let Some(o) = HOME.with(|h| h.borrow().clone()) {
            attach(&o);
        }
        glib::Propagation::Proceed
    });
    win.present();
}

/// Holds the page over the canvas and draws it with [`XFORM`] applied (see [`recompute`]).
/// Snapshot-time, so it moves in the very frame the canvas does; no re-layout of the page.
mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct Shift;

    #[glib::object_subclass]
    impl ObjectSubclass for Shift {
        const NAME: &'static str = "CgImplantShift";
        type Type = super::Shift;
        type ParentType = gtk::Widget;
        fn class_init(k: &mut Self::Class) {
            k.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for Shift {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for Shift {
        fn snapshot(&self, s: &gtk::Snapshot) {
            let w = self.obj();
            let Some(c) = w.first_child() else { return };
            let sn = gtk::Snapshot::new();
            w.snapshot_child(&c, &sn);
            let Some(node) = sn.to_node() else { return };
            if pending() {
                // A private software renderer: the window's own (GL) one must not render mid-frame.
                thread_local!(static PROBE: Option<gtk::gsk::Renderer> = {
                    let r: gtk::gsk::Renderer = gtk::gsk::CairoRenderer::new().upcast();
                    r.realize(None).ok().map(|_| r)
                });
                if let Some(r) = PROBE.with(Clone::clone) {
                    // One pixel rendered and read back per page frame, only while a layout is on its way.
                    let t = r.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, 1.0, 1.0)));
                    let mut px = [0u8; 4];
                    t.download(&mut px, 4);
                    // Memory format is native-endian ARGB32 premultiplied: B, G, R, A on little-endian.
                    let (b, g, r) = if cfg!(target_endian = "little") { (px[0], px[1], px[2]) } else { (px[3], px[2], px[1]) };
                    if b == 90 && px[if cfg!(target_endian = "little") { 3 } else { 0 }] == 255 {
                        shown(r as u64 | (g as u64) << 8);
                    }
                }
            }
            let Some((k, dx, dy)) = XFORM.with(|x| x.get()) else { return };
            s.push_clip(&gtk::graphene::Rect::new(0.0, 0.0, w.width() as f32, w.height() as f32));
            s.translate(&gtk::graphene::Point::new(dx as f32, dy as f32));
            s.scale(k as f32, k as f32);
            // Not the stamp pixel's row.
            s.push_clip(&gtk::graphene::Rect::new(0.0, 1.0, c.width() as f32, c.height() as f32 - 1.0));
            s.append_node(&node);
            s.pop();
            s.pop();
        }
    }
}

glib::wrapper! {
    pub struct Shift(ObjectSubclass<imp::Shift>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
