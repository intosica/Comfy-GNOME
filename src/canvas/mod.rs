// SPDX-License-Identifier: GPL-3.0-only
// Link-curve routing adapted from Helvum `src/ui/graph/graph_view.rs`
// (c) 2021 Tom A. Wagner, GPL-3.0-only. Node metrics mirror ComfyUI_frontend
// litegraph defaults (LiteGraphGlobal.ts: NODE_TITLE_HEIGHT=30, NODE_SLOT_HEIGHT=20).
//
//! Immediate-mode node canvas on a single `gtk::DrawingArea`.
//!
//! Unlike Helvum (one GtkWidget per node/port), everything is painted with cairo
//! and hit-tested in canvas space. ComfyUI graphs reach hundreds of nodes; one
//! widget per node would cost layout passes and X11 input-region churn.

mod attention;
pub mod transform;
pub mod text;
pub mod widgets;

use adw::{gdk, gtk, prelude::*};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use serde_json::Value;
use std::rc::Rc;
use std::time::Duration;
use transform::View;
use widgets::{WKind, Widget, Zone};

const TITLE_H: f64 = 30.0;
const SLOT_H: f64 = 20.0;
const GRID: f64 = 20.0;

/// A node socket: label text plus the socket type (drives colour and link validity).
#[derive(Default, Clone)]
pub struct Slot {
    pub label: String,
    pub ty: String,
}

#[derive(Default, Clone)]
pub struct Node {
    /// Workflow node id (opaque to the canvas; reported back on activation).
    pub id: String,
    pub title: String,
    pub pos: (f64, f64),
    pub size: (f64, f64),
    pub inputs: Vec<Slot>,
    pub outputs: Vec<Slot>,
    /// Editable rows drawn below the slots.
    pub widgets: Vec<Widget>,
    /// Image preview keys (`/view` query strings) drawn in the leftover body space.
    pub images: Vec<String>,
    /// Drawn over the first image: a tinted mask (image key, alpha = mask) and numbered boxes.
    pub overlay: Option<Overlay>,
    // Layout cache (`relayout`): socket y offsets and widget rects, relative to `pos`.
    pub ins_y: Vec<f64>,
    pub outs_y: Vec<f64>,
    pub rows: Vec<[f64; 4]>,
    pub img_rect: Option<[f64; 4]>,
    pub color: Option<u32>,
    pub bgcolor: Option<u32>,
    /// litegraph mode: 2 = muted, 4 = bypassed.
    pub mode: i64,
    pub collapsed: bool,
    /// `flags.pinned`: drawn with a pin marker and never dragged.
    pub pinned: bool,
    /// Subgraph instance: drawn with a marker, double-click opens it.
    pub subgraph: bool,
}

/// Region boxes (normalised to the picture) and a painted mask over a node's preview.
#[derive(Default, Clone)]
pub struct Overlay {
    pub mask: Option<String>,
    /// Tint where the node's own picture is transparent (Load Image after "Open in Mask Editor").
    pub alpha_mask: bool,
    pub boxes: Vec<[f64; 4]>,
}

/// Box colours shared with the mask editor.
pub const BOX_COLORS: [u32; 8] = [0xff5252, 0x40c4ff, 0xffd740, 0x69f0ae, 0xe040fb, 0xff9100, 0x18ffff, 0xb2ff59];

#[derive(Clone)]
pub struct Group {
    pub title: String,
    pub rect: [f64; 4],
    pub color: Option<u32>,
}

impl Node {
    fn body_h(&self) -> f64 {
        if self.collapsed { 0.0 } else { self.size.1 }
    }
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.pos.0 && x <= self.pos.0 + self.size.0 && y >= self.pos.1 - TITLE_H && y <= self.pos.1 + self.body_h()
    }
    /// `pos` is the top-left of the body; the title bar sits above it (litegraph convention).
    fn slot_pos(&self, output: bool, i: usize) -> (f64, f64) {
        if self.collapsed {
            let x = if output { self.pos.0 + self.size.0 } else { self.pos.0 };
            return (x, self.pos.1 - TITLE_H / 2.0);
        }
        let x = if output { self.pos.0 + self.size.0 } else { self.pos.0 };
        let y = if output { &self.outs_y } else { &self.ins_y };
        (x, self.pos.1 + y.get(i).copied().unwrap_or((i as f64 + 0.7) * SLOT_H))
    }

    /// Lay out sockets and widget rows; grows `size.1` to fit, never shrinks.
    /// Inputs shadowed by a widget put their socket on the widget's row.
    pub fn relayout(&mut self) {
        let mut shadow = vec![false; self.inputs.len()];
        for i in self.widgets.iter().filter_map(|w| w.input) {
            if let Some(s) = shadow.get_mut(i) {
                *s = true;
            }
        }
        let mut k = 0.0;
        self.ins_y = shadow
            .iter()
            .map(|&sh| {
                let y = (k + 0.7) * SLOT_H;
                if !sh { k += 1.0; }
                y
            })
            .collect();
        self.outs_y = (0..self.outputs.len()).map(|j| (j as f64 + 0.7) * SLOT_H).collect();
        let y0 = k.max(self.outputs.len() as f64) * SLOT_H + if self.widgets.is_empty() { 0.0 } else { widgets::GAP };
        let flex = self.widgets.iter().filter(|w| w.multiline()).count();
        let rw = self.size.0 - 20.0;
        let hs: Vec<f64> = self.widgets.iter().map(|w| widgets::row_height(w, rw)).collect();
        let fixed: f64 = self.widgets.iter().zip(&hs).map(|(w, h)| if w.multiline() { widgets::MULTI_MIN } else { *h } + widgets::GAP).sum();
        let img = if !self.images.is_empty() { 80.0 } else { 0.0 };
        let min_h = y0 + fixed + img + if self.widgets.is_empty() && img == 0.0 { 8.0 } else { 2.0 };
        self.size.1 = self.size.1.max(min_h);
        let extra = if flex > 0 { (self.size.1 - min_h) / flex as f64 } else { 0.0 };
        self.rows.clear();
        let mut cur = y0;
        for (w, hh) in self.widgets.iter().zip(&hs) {
            let h = if w.multiline() { widgets::MULTI_MIN + extra } else { *hh };
            self.rows.push([10.0, cur, self.size.0 - 20.0, h]);
            if let Some(y) = w.input.and_then(|i| self.ins_y.get_mut(i)) {
                *y = cur + widgets::ROW_H / 2.0;
            }
            cur += h + widgets::GAP;
        }
        self.img_rect = (!self.images.is_empty()).then(|| [10.0, cur, self.size.0 - 20.0, (self.size.1 - cur - 6.0).max(20.0)]);
    }

    fn row(&self, i: usize) -> [f64; 4] {
        let r = self.rows[i];
        [self.pos.0 + r[0], self.pos.1 + r[1], r[2], r[3]]
    }

    /// Interactive widget row under canvas point `c`, with the zone hit.
    fn widget_at(&self, c: (f64, f64)) -> Option<(usize, Zone)> {
        if self.collapsed {
            return None;
        }
        self.widgets.iter().enumerate().find_map(|(i, w)| {
            if matches!(w.kind, WKind::Label | WKind::ReadOnly) || i >= self.rows.len() {
                return None;
            }
            let r = self.row(i);
            (c.0 >= r[0] && c.0 <= r[0] + r[2] && c.1 >= r[1] && c.1 <= r[1] + r[3]).then(|| (i, widgets::zone(w, r, c.0)))
        })
    }
}

/// What a click landed on, in canvas coordinates, for context menus.
#[derive(Clone, Debug)]
pub enum Hit {
    Node(String),
    /// A widget row inside a node (node id, row index).
    Widget(String, usize),
    Group(usize),
    Empty,
}

/// Wire between two nodes. `from_*` is the origin output, `to_*` the target input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: usize,
    pub from_slot: usize,
    pub to: usize,
    pub to_slot: usize,
}

enum Drag {
    /// Moving the current selection; positions are captured at drag start.
    Nodes { idx: usize, grab: (f64, f64), start: Vec<(usize, (f64, f64))> },
    Pan { start_offset: (f64, f64) },
    Link(LinkDrag),
    /// Rubber-band selection (ctrl + drag on empty canvas, as in litegraph).
    Band { start: (f64, f64), end: (f64, f64) },
    /// Dragging a group's title bar moves the group and the nodes it holds.
    Group { idx: usize, start: [f64; 4], nodes: Vec<(usize, (f64, f64))>, grab: (f64, f64) },
    /// Pressed on a widget row: a click activates it, a horizontal drag scrubs numbers.
    Widget { node: usize, w: usize, zone: Zone, base: Value, moved: bool },
    /// Bottom-right grip: `start` is the size at drag begin, `snap` rounds to the grid.
    Resize { idx: usize, start: (f64, f64), snap: bool },
}

/// A wire being dragged, anchored at one end. The free end is on the opposite
/// side: anchored at an output → looking for an input; anchored at an input
/// (shift on a connected output, or pulling a wire off an input) → looking for
/// a new output. In both cases the old wire is deleted up front, so dropping on
/// nothing leaves it removed.
struct LinkDrag {
    /// Node + slot the wire hangs from.
    node: usize,
    slot: usize,
    /// `true` when the anchor is an output of `node`.
    out: bool,
    ty: String,
    /// Canvas-space cursor position.
    to: (f64, f64),
    /// Anything added or removed during this drag (a wire pulled off a socket
    /// counts, so dropping it on nothing leaves the connection deleted).
    changed: bool,
}

#[derive(Default)]
struct State {
    view: View,
    nodes: Vec<Node>,
    /// Paint order, back to front. Indices into `nodes`; links stay index-stable.
    order: Vec<usize>,
    links: Vec<Link>,
    groups: Vec<Group>,
    drag: Option<Drag>,
    pointer: (f64, f64),
    /// Selected node ids (survives re-scenes: ids are stable).
    sel: HashSet<String>,
    /// Decoded previews by `/view` query; `None` while loading or when undecodable.
    images: HashMap<String, Option<gtk::cairo::ImageSurface>>,
    /// Multiline text widgets scrolled with the wheel: node id → widget key → canvas px.
    text_scroll: HashMap<String, HashMap<String, f64>>,
    /// Host node id currently executing (green outline).
    running: Option<String>,
    /// Snap node moves/resizes to `snap_grid` (`Comfy.SnapToGrid.GridSize`).
    snap_always: bool,
    snap_grid: f64,
    /// `Comfy.LinkRenderMode`: -1 hidden, 0 straight, 1 linear, 2 spline (litegraph values).
    link_mode: i32,
    /// Subgraph collapse waiting out the double-click interval (a double-click enters the
    /// subgraph instead of collapsing it).
    pending_collapse: Option<gtk::glib::SourceId>,
}

impl State {
    /// Wheel over a multiline text widget scrolls its text instead of zooming the
    /// canvas. Returns whether the wheel was consumed.
    fn scroll_text(&mut self, dy: f64) -> bool {
        let c = self.view.to_canvas(self.pointer);
        let Some(&i) = self.order.iter().rev().find(|&&i| self.nodes[i].contains(c)) else { return false };
        let n = &self.nodes[i];
        let Some((wi, _)) = n.widget_at(c) else { return false };
        let Some(w) = n.widgets.get(wi).filter(|w| w.multiline()) else { return false };
        let r = n.row(wi);
        let max = widgets::text_content_h(w.value.as_str().unwrap_or(""), r[2]) - r[3];
        if max <= 0.5 {
            return false;
        }
        let off = self.text_scroll.entry(n.id.clone()).or_default().entry(w.key.clone()).or_insert(0.0);
        let next = (*off + dy * 3.0 * widgets::LINE_H).clamp(0.0, max);
        // At either end the wheel goes back to zooming instead of being swallowed.
        if (next - *off).abs() < 0.5 {
            return false;
        }
        *off = next;
        true
    }
}

/// litegraph `snapPoint`: round to the nearest multiple; `0` keeps the value.
fn snap_to(v: f64, grid: f64) -> f64 {
    if grid > 0.0 { grid * (v / grid).round() } else { v }
}

impl State {
    fn toggle_collapse(&mut self, id: &str) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.collapsed = !n.collapsed;
            n.relayout();
        }
    }

    /// Topmost socket within `r` canvas units of `p`.
    fn hit_slot(&self, p: (f64, f64), r: f64) -> Option<(usize, bool, usize)> {
        for &i in self.order.iter().rev() {
            let n = &self.nodes[i];
            for (out, slots) in [(true, &n.outputs), (false, &n.inputs)] {
                for (si, _) in slots.iter().enumerate() {
                    let (sx, sy) = n.slot_pos(out, si);
                    // A collapsed node's input socket sits under its collapse button: the button wins.
                    if n.collapsed && !out && p.0 > sx + 3.0 {
                        continue;
                    }
                    if (sx - p.0).hypot(sy - p.1) <= r {
                        return Some((i, out, si));
                    }
                }
            }
        }
        None
    }

    /// Topmost node under canvas point `p`.
    fn hit_node(&self, p: (f64, f64)) -> Option<usize> {
        self.order.iter().rev().copied().find(|&i| self.nodes[i].contains(p))
    }

    /// Group whose title bar covers `p` (kept grabbable at low zoom).
    fn hit_group(&self, p: (f64, f64)) -> Option<usize> {
        let hh = 34.0f64.max(26.0 / self.view.zoom);
        self.groups.iter().rposition(|g| {
            let r = g.rect;
            p.0 >= r[0] && p.0 <= r[0] + r[2] && p.1 >= r[1] && p.1 <= r[1] + hh
        })
    }

    /// Group whose whole rect covers `p` (context menu target).
    fn group_at(&self, p: (f64, f64)) -> Option<usize> {
        self.groups.iter().rposition(|g| {
            let r = g.rect;
            p.0 >= r[0] && p.0 <= r[0] + r[2] && p.1 >= r[1] && p.1 <= r[1] + r[3]
        })
    }

    /// Topmost socket on the `out` side that the dragged wire may land on.
    fn hit_target(&self, p: (f64, f64), r: f64, ty: &str, out: bool) -> Option<(usize, usize)> {
        for &i in self.order.iter().rev() {
            let n = &self.nodes[i];
            let slots = if out { &n.outputs } else { &n.inputs };
            for (si, slot) in slots.iter().enumerate() {
                let (sx, sy) = n.slot_pos(out, si);
                if (sx - p.0).hypot(sy - p.1) > r {
                    continue;
                }
                // `ty` is the free end's type: the anchor decides which side is compared.
                let ok = if out { compatible(&slot.ty, ty) } else { compatible(ty, &slot.ty) };
                if ok || slot.ty.is_empty() || ty.is_empty() {
                    return Some((i, si));
                }
            }
        }
        None
    }

    /// Drop the dragged wire on the opposite side. Inputs take one link only
    /// (replacing what was there); outputs fan out freely.
    fn finish_link(&mut self, d: &mut LinkDrag, r: f64) -> bool {
        let Some((other, slot)) = self.hit_target(d.to, r, &d.ty, !d.out) else { return false };
        if d.out {
            self.links.retain(|l| !(l.to == other && l.to_slot == slot));
            self.links.push(Link { from: d.node, from_slot: d.slot, to: other, to_slot: slot });
        } else {
            self.links.push(Link { from: other, from_slot: slot, to: d.node, to_slot: d.slot });
        }
        d.changed = true;
        true
    }
}

/// Socket types match when identical, when either side is a wildcard, or when the
/// input lists alternatives (`"IMAGE,MASK"`).
pub fn compatible(out_ty: &str, in_ty: &str) -> bool {
    let ok = |t: &str| t == out_ty || t == "*" || out_ty.is_empty() || out_ty == "*";
    in_ty.is_empty() || in_ty.split(',').any(|t| ok(t.trim()))
}

/// ComfyUI dark-palette socket colours (`src/assets/palettes/dark.json` → colors.node_slot),
/// with litegraph's default link colour for everything else.
pub fn type_color(ty: &str) -> u32 {
    match ty {
        "CLIP" => 0xffd500,
        "CLIP_VISION" => 0xa8dadc,
        "CLIP_VISION_OUTPUT" => 0xad7452,
        "CONDITIONING" => 0xffa931,
        "CONTROL_NET" => 0x6ee7b7,
        "IMAGE" => 0x64b5f6,
        "LATENT" => 0xff9cf9,
        "MASK" => 0x81c784,
        "MODEL" => 0xb39ddb,
        "STYLE_MODEL" => 0xc2ffae,
        "VAE" => 0xff6e6e,
        "NOISE" => 0xb0b0b0,
        "GUIDER" => 0x66ffff,
        "SAMPLER" => 0xecb4b4,
        "SIGMAS" => 0xcdffcd,
        "TAESD" => 0xdcc274,
        _ => 0x99aa99,
    }
}

type WidgetCb = Box<dyn Fn(&str, &str, Value, bool)>;

#[derive(Clone)]
pub struct Canvas {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
    on_activate: Rc<RefCell<Option<Box<dyn Fn(&str, (f64, f64))>>>>,
    on_change: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    on_context: Rc<RefCell<Option<Box<dyn Fn(f64, f64, Hit)>>>>,
    /// (node id, widget key, new value, structural)
    on_widget: Rc<RefCell<Option<WidgetCb>>>,
    /// A wire was dropped on empty canvas: (source node, slot, from output?, type, drop pos).
    on_link_menu: Rc<RefCell<Option<Box<dyn Fn(&str, usize, bool, &str, (f64, f64))>>>>,
    /// A graph was set before the widget had a size: fit as soon as it does.
    pending_fit: Rc<Cell<bool>>,
    /// REST base for prompt autocomplete (set by the workspace once connected).
    api_base: Rc<RefCell<Option<String>>>,
}

impl Canvas {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
        let c = Self {
            area,
            state: Rc::default(),
            on_activate: Rc::default(),
            on_change: Rc::default(),
            on_context: Rc::default(),
            on_widget: Rc::default(),
            on_link_menu: Rc::default(),
            pending_fit: Rc::default(),
            api_base: Rc::default(),
        };
        let me = c.clone();
        let pending = c.pending_fit.clone();
        c.state.borrow_mut().link_mode = 2; // litegraph's default: spline
        c.area.connect_resize(move |_, w, h| {
            if pending.get() && w > 10 && h > 10 {
                me.fit_view();
            }
        });
        c.setup_draw();
        c.setup_input();
        c
    }

    /// Replace the whole scene and fit it into view.
    pub fn set_graph(&self, nodes: Vec<Node>, links: Vec<Link>, groups: Vec<Group>) {
        let mut s = self.state.borrow_mut();
        s.order = (0..nodes.len()).collect();
        let mut nodes = nodes;
        nodes.iter_mut().for_each(Node::relayout);
        s.nodes = nodes;
        s.links = links;
        s.groups = groups;
        s.drag = None;
        // Widgets are rebuilt per scene: keep scroll only for widgets that still exist.
        let State { nodes, text_scroll, .. } = &mut *s;
        text_scroll.retain(|id, m| {
            let Some(n) = nodes.iter().find(|n| &n.id == id) else { return false };
            m.retain(|k, _| n.widgets.iter().any(|w| &w.key == k));
            !m.is_empty()
        });
        drop(s);
        self.pending_fit.set(true);
        self.fit_view();
    }

    /// Copy of the current scene (nodes carry live positions).
    pub fn scene(&self) -> (Vec<Node>, Vec<Link>, Vec<Group>) {
        let s = self.state.borrow();
        (s.nodes.clone(), s.links.clone(), s.groups.clone())
    }

    pub fn view(&self) -> (f64, (f64, f64)) {
        let v = self.state.borrow().view;
        (v.zoom, v.offset)
    }

    /// Restore an explicit viewport (wins over the initial auto-fit).
    pub fn set_view(&self, zoom: f64, offset: (f64, f64)) {
        self.pending_fit.set(false);
        let mut s = self.state.borrow_mut();
        s.view = View { zoom: zoom.clamp(transform::ZOOM_MIN, transform::ZOOM_MAX), offset };
        drop(s);
        self.area.queue_draw();
    }

    /// Double-click on a node (by id).
    /// Double-click: node id (subgraph drill-in) or "" on empty space; second arg is the canvas position.
    pub fn connect_activate(&self, f: impl Fn(&str, (f64, f64)) + 'static) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// A node moved or a wire was connected/disconnected (edits to persist).
    /// `always` = `pysssss.SnapToGrid`; `grid` = `Comfy.SnapToGrid.GridSize` (0 = keep).
    pub fn set_snap(&self, always: bool, grid: f64) {
        let mut s = self.state.borrow_mut();
        s.snap_always = always;
        s.snap_grid = grid;
    }

    /// `Comfy.LinkRenderMode` (litegraph: -1 hidden, 0 straight, 1 linear, 2 spline).
    pub fn set_link_mode(&self, mode: i32) {
        self.state.borrow_mut().link_mode = mode;
        self.area.queue_draw();
    }

    /// Right-click menu entries for widget row `idx` of node `id`, derived from the widget
    /// kind (see `widgets::menu`), so every node's rows get their own actions for free.
    pub fn widget_menu(&self, id: &str, idx: usize) -> Vec<(String, widgets::Action, bool)> {
        let s = self.state.borrow();
        let Some(n) = s.nodes.iter().find(|n| n.id == id) else { return Vec::new() };
        widgets::menu(&n.widgets, idx)
    }

    /// Widget row `idx` of node `id` (label, model key, current value).
    pub fn widget_at_id(&self, id: &str, idx: usize) -> Option<(String, String, Value)> {
        let s = self.state.borrow();
        let n = s.nodes.iter().find(|n| n.id == id)?;
        let w = n.widgets.get(idx)?;
        Some((w.label.clone(), w.key.clone(), w.value.clone()))
    }

    pub fn connect_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// Right click on the canvas: what sits under the pointer.
    pub fn connect_context(&self, f: impl Fn(f64, f64, Hit) + 'static) {
        *self.on_context.borrow_mut() = Some(Box::new(f));
    }

    /// A widget value was edited (or a button pressed: value `Null`).
    pub fn connect_widget(&self, f: impl Fn(&str, &str, Value, bool) + 'static) {
        *self.on_widget.borrow_mut() = Some(Box::new(f));
    }

    /// A wire was dropped on empty canvas: offer nodes that can accept it there.
    pub fn connect_link_menu(&self, f: impl Fn(&str, usize, bool, &str, (f64, f64)) + 'static) {
        *self.on_link_menu.borrow_mut() = Some(Box::new(f));
    }

    /// Backend base URL used by prompt autocomplete. Set after connecting.
    pub fn set_api_base(&self, base: &str) {
        *self.api_base.borrow_mut() = Some(base.to_owned());
    }

    /// Image keys wanted by nodes that are neither loaded nor loading; marks them loading.
    pub fn missing_images(&self) -> Vec<String> {
        let mut s = self.state.borrow_mut();
        let want: Vec<String> = s.nodes.iter().flat_map(|n| n.images.first().cloned().into_iter().chain(n.overlay.iter().filter_map(|o| o.mask.clone())).chain(n.widgets.iter().flat_map(widgets::cloud_images))).collect();
        let mut out = Vec::new();
        for k in want {
            if !s.images.contains_key(&k) {
                s.images.insert(k.clone(), None);
                out.push(k);
            }
        }
        out
    }

    /// Decode `bytes` (downscaled to <= 512 px) as the preview for `key`.
    pub fn put_image(&self, key: &str, bytes: &[u8]) {
        let Ok(tex) = gdk::Texture::from_bytes(&gtk::glib::Bytes::from(bytes)) else { return };
        let (w, h) = (tex.width(), tex.height());
        if w < 1 || h < 1 {
            return;
        }
        let stride = w as usize * 4;
        let mut data = vec![0u8; stride * h as usize];
        tex.download(&mut data, stride);
        use gtk::cairo::{Filter, Format, ImageSurface};
        let Ok(full) = ImageSurface::create_for_data(data, Format::ARgb32, w, h, stride as i32) else { return };
        let k = (512.0 / w.max(h) as f64).min(1.0);
        let (tw, th) = (((w as f64 * k).round() as i32).max(1), ((h as f64 * k).round() as i32).max(1));
        let Ok(small) = ImageSurface::create(Format::ARgb32, tw, th) else { return };
        if let Ok(cr) = gtk::cairo::Context::new(&small) {
            cr.scale(k, k);
            if cr.set_source_surface(&full, 0.0, 0.0).is_ok() {
                cr.source().set_filter(Filter::Best);
                cr.paint().ok();
            }
        }
        self.state.borrow_mut().images.insert(key.to_owned(), Some(small));
        self.area.queue_draw();
    }

    /// Drop a cached preview so the next `missing_images` fetches it again (file overwritten).
    pub fn forget_image(&self, key: &str) {
        self.state.borrow_mut().images.remove(key);
    }

    /// Show `keys` on node `id`.
    pub fn set_node_images(&self, id: &str, keys: Vec<String>) {
        let mut s = self.state.borrow_mut();
        if let Some(n) = s.nodes.iter_mut().find(|n| n.id == id) {
            n.images = keys;
            n.relayout();
        }
        drop(s);
        self.area.queue_draw();
    }

    pub fn set_running(&self, id: Option<String>) {
        self.state.borrow_mut().running = id;
        self.area.queue_draw();
    }

    fn emit(&self, id: &str, key: &str, value: Value, store: bool) {
        let structural = {
            let mut s = self.state.borrow_mut();
            let Some(w) = s.nodes.iter_mut().find(|n| n.id == id).and_then(|n| n.widgets.iter_mut().find(|w| w.key == key)) else { return };
            if store {
                w.value = value.clone();
            }
            w.structural
        };
        self.area.queue_draw();
        if let Some(f) = self.on_widget.borrow().as_ref() {
            f(id, key, value, structural);
        }
    }

    /// Click on a widget row: step, toggle, or open the matching editor.
    fn activate_widget(&self, ni: usize, wi: usize, zone: Zone) {
        use widgets::{as_num, edit_combo, edit_number, edit_text, lora_with, num_value, step_of};
        let (id, w, rect, linked, ptr, rw) = {
            let s = self.state.borrow();
            let Some(n) = s.nodes.get(ni) else { return };
            let Some(w) = n.widgets.get(wi).cloned() else { return };
            let r = n.row(wi);
            let (a, b) = (s.view.to_screen((r[0], r[1])), s.view.to_screen((r[0] + r[2], r[1] + r[3])));
            let rect = [a.0 as i32, a.1 as i32, (b.0 - a.0) as i32, (b.1 - a.1) as i32];
            let linked = w.input.is_some_and(|i| s.links.iter().any(|l| l.to == ni && l.to_slot == i));
            let c = s.view.to_canvas(s.pointer);
            (n.id.clone(), w, rect, linked, (c.0 - r[0], c.1 - r[1]), r[2])
        };
        if linked {
            return;
        }
        let dir = match zone { Zone::Dec => -1.0, Zone::Inc => 1.0, _ => 0.0 };
        let key = w.key.clone();
        let btn_id = id.clone();
        let me = self.clone();
        let send = move |v: Value| me.emit(&id, &key, v, true);
        match (&w.kind, zone) {
            (WKind::Int { .. } | WKind::Float { .. }, Zone::Dec | Zone::Inc) => {
                send(num_value(&w.kind, as_num(&w.value) + dir * step_of(&w.kind)));
            }
            (WKind::Int { min, max, step }, _) => {
                edit_number(&self.area, rect, as_num(&w.value), *min, *max, *step, 0, move |x| send(Value::from(x.round() as i64)));
            }
            (WKind::Float { min, max, step, prec }, _) => {
                let k = w.kind.clone();
                edit_number(&self.area, rect, as_num(&w.value), *min, *max, *step, *prec as u32, move |x| send(num_value(&k, x)));
            }
            (WKind::Combo(opts), Zone::Dec | Zone::Inc) => {
                let cur = opts.iter().position(|o| Some(o.as_str()) == w.value.as_str()).unwrap_or(0) as i64;
                if let Some(o) = opts.get((cur + dir as i64).clamp(0, opts.len() as i64 - 1) as usize) {
                    send(Value::from(o.clone()));
                }
            }
            (WKind::Combo(opts), _) => {
                edit_combo(&self.area, rect, opts, w.value.as_str().unwrap_or(""), None, move |c| {
                    if let Some(c) = c {
                        send(Value::from(c));
                    }
                });
            }
            (WKind::Bool, _) => send(Value::Bool(!w.value.as_bool().unwrap_or(false))),
            (WKind::Text { multiline }, _) => {
                let completion = self.api_base.borrow().clone().map(|base| widgets::TextCompletion { base });
                if *multiline && widgets::inline_text() {
                    // Follow the row as the view pans/zooms; None once the node is gone or collapsed.
                    let (st, nid) = (Rc::downgrade(&self.state), btn_id.clone());
                    let place: widgets::PlaceFn = Rc::new(move || {
                        let st = st.upgrade()?;
                        let s = st.borrow();
                        let n = s.nodes.iter().find(|n| n.id == nid).filter(|n| !n.collapsed)?;
                        let r = n.row(wi);
                        let (a, b) = (s.view.to_screen((r[0], r[1])), s.view.to_screen((r[0] + r[2], r[1] + r[3])));
                        Some(([a.0, a.1, b.0 - a.0, b.1 - a.1], s.view.zoom))
                    });
                    let send2 = send.clone();
                    if widgets::edit_text_inline(&self.area, place, w.value.as_str().unwrap_or(""), completion.clone(), move |t| send2(Value::from(t))) {
                        return;
                    }
                }
                edit_text(&self.area, rect, w.value.as_str().unwrap_or(""), *multiline, completion, move |t| send(Value::from(t)));
            }
            (WKind::Button, _) => self.emit(&btn_id, &w.key, Value::Null, false),
            (WKind::Lora(_), Zone::Toggle) => send(lora_with(&w.value, "on", Value::Bool(!w.value["on"].as_bool().unwrap_or(true)))),
            (WKind::Lora(_), Zone::Dec | Zone::Inc) => {
                let v = ((as_num(&w.value["strength"]) + dir * 0.05) * 100.0).round() / 100.0;
                send(lora_with(&w.value, "strength", Value::from(v)));
            }
            (WKind::Lora(_), Zone::Aux) => {
                let old = w.value.clone();
                edit_number(&self.area, rect, as_num(&w.value["strength"]), -100.0, 100.0, 0.05, 2, move |x| {
                    send(lora_with(&old, "strength", Value::from((x * 100.0).round() / 100.0)))
                });
            }
            (WKind::Lora(opts), _) => {
                let old = w.value.clone();
                edit_combo(&self.area, rect, opts, w.value["lora"].as_str().unwrap_or(""), Some("✕ Remove this LoRA"), move |c| {
                    send(match c {
                        Some(name) => lora_with(&old, "lora", Value::from(name)),
                        None => Value::Null,
                    })
                });
            }
            (WKind::Cloud(mode), _) => self.activate_cloud(&w, *mode, widgets::cloud_hit(&w, rw, ptr), rect, ptr),
            (WKind::Seed, Zone::Inc) => {
                // 🎲: roll now, remember it — the queued copies then reuse it as Fixed would.
                let mut st = crate::dasiwa::SeedState::parse(w.value.as_str().unwrap_or(""));
                st.last_seed = Some(crate::dasiwa::rand_seed().to_string());
                st.recent.truncate(9);
                st.recent.insert(0, st.last_seed.clone().unwrap_or_default());
                send(Value::from(st.to_json()));
            }
            (WKind::Seed, _) => {
                let mut st = crate::dasiwa::SeedState::parse(w.value.as_str().unwrap_or(""));
                st.mode = if st.mode == "fixed" { "random".into() } else { "fixed".into() };
                send(Value::from(st.to_json()));
            }
            _ => {}
        }
    }

    /// Click inside an EreNodes tag widget.
    fn activate_cloud(&self, w: &Widget, mode: widgets::CloudMode, hit: widgets::CHit, rect: [i32; 4], ptr: (f64, f64)) {
        use crate::erenodes as ere;
        use widgets::CHit;
        let tags = w.value.as_array().cloned().unwrap_or_default();
        let (id, key) = (self.node_of_widget(w), w.key.clone());
        let me = self.clone();
        let send = move |v: Value| me.emit(&id, &key, v, true);
        let (id2, key2, me2) = (self.node_of_widget(w), w.key.clone(), self.clone());
        let op = move |o: Value| me2.emit(&id2, &key2, o, false);
        // Popup anchored at the click, not the whole (possibly huge) row.
        let at = [rect[0] + ptr.0 as i32, rect[1] + ptr.1 as i32 - 6, 1, 12];
        match hit {
            CHit::Tag(i) => {
                let mut t = tags;
                if let Some(x) = t.get_mut(i) {
                    x["active"] = Value::Bool(!ere::active(x));
                }
                send(Value::Array(t));
            }
            CHit::Menu => {
                let mut opts: Vec<String> = ["Toggle All Tags", "Remove All Tags", "Remove Inactive Tags", "Load Tag Group…"].map(String::from).into();
                opts.extend(ere::CONVERT.iter().map(|(d, _)| format!("Convert to {d}")));
                widgets::edit_combo(&self.area, at, &opts, "", None, move |c| {
                    let Some(c) = c else { return };
                    let any_off = tags.iter().any(|t| !ere::active(t));
                    match c.as_str() {
                        "Toggle All Tags" => send(Value::Array(tags.iter().cloned().map(|mut t| { t["active"] = Value::Bool(any_off); t }).collect())),
                        "Remove All Tags" => send(Value::Array(vec![])),
                        "Remove Inactive Tags" => send(Value::Array(tags.iter().filter(|t| ere::active(t)).cloned().collect())),
                        "Load Tag Group…" => op(serde_json::json!({"op": "load", "at": at})),
                        c => {
                            if let Some((_, ty)) = ere::CONVERT.iter().find(|(d, _)| format!("Convert to {d}") == c) {
                                op(serde_json::json!({"op": "convert", "type": ty}));
                            }
                        }
                    }
                });
            }
            CHit::Add => op(serde_json::json!({"op": "add", "at": at})),
            CHit::Random => op(serde_json::json!({"op": "random"})),
            CHit::Bg => {
                if !matches!(mode, widgets::CloudMode::Select { .. }) {
                    return;
                }
                let off: Vec<(usize, String)> = tags.iter().enumerate().filter(|(_, t)| !ere::active(t)).map(|(i, t)| (i, ere::name(t).to_owned())).collect();
                let names: Vec<String> = off.iter().map(|(_, n)| n.clone()).collect();
                if names.is_empty() {
                    return;
                }
                widgets::edit_combo(&self.area, at, &names, "", None, move |c| {
                    let Some(c) = c else { return };
                    let mut t = tags.clone();
                    if let Some((i, _)) = off.iter().find(|(_, n)| *n == c) {
                        t[*i]["active"] = Value::Bool(true);
                    }
                    send(Value::Array(t));
                });
            }
        }
    }

    fn node_of_widget(&self, w: &Widget) -> String {
        let s = self.state.borrow();
        s.nodes.iter().find(|n| n.widgets.iter().any(|x| std::ptr::eq(x, w) || (x.key == w.key && x.value == w.value))).map(|n| n.id.clone()).unwrap_or_default()
    }

    /// Selected node ids, sorted for stable output.
    pub fn selected(&self) -> Vec<String> {
        let mut v: Vec<String> = self.state.borrow().sel.iter().cloned().collect();
        v.sort();
        v
    }

    /// Bounds of the selection in area coordinates, plus how many nodes it holds. `None` when
    /// nothing is selected — the floating toolbox uses this to follow the nodes.
    pub fn selection_box(&self) -> Option<(f64, f64, f64, f64, usize)> {
        let s = self.state.borrow();
        if s.sel.is_empty() {
            return None;
        }
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for n in s.nodes.iter().filter(|n| s.sel.contains(&n.id)) {
            b = [b[0].min(n.pos.0), b[1].min(n.pos.1 - TITLE_H), b[2].max(n.pos.0 + n.size.0), b[3].max(n.pos.1 + n.body_h())];
        }
        if b[0] > b[2] {
            return None;
        }
        let (x, y) = s.view.to_screen((b[0], b[1]));
        Some((x, y, (b[2] - b[0]) * s.view.zoom, (b[3] - b[1]) * s.view.zoom, s.sel.len()))
    }

    /// Body colour of the first selected node, for the toolbox swatch.
    pub fn selection_color(&self) -> Option<u32> {
        let s = self.state.borrow();
        s.nodes.iter().find(|n| s.sel.contains(&n.id)).and_then(|n| n.bgcolor)
    }

    pub fn select(&self, ids: &[String]) {
        self.state.borrow_mut().sel = ids.iter().cloned().collect();
        self.area.queue_draw();
    }

    /// Zoom/pan so every node is visible (falls back to reset when empty).
    pub fn fit_view(&self) {
        let (w, h) = (self.area.width() as f64, self.area.height() as f64);
        if w < 10.0 || h < 10.0 {
            self.pending_fit.set(true); // retried from connect_resize
            return;
        }
        self.pending_fit.set(false);
        let mut s = self.state.borrow_mut();
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for n in &s.nodes {
            b = [b[0].min(n.pos.0), b[1].min(n.pos.1 - TITLE_H), b[2].max(n.pos.0 + n.size.0), b[3].max(n.pos.1 + n.body_h())];
        }
        if s.nodes.is_empty() || w < 10.0 || h < 10.0 {
            s.view = View { zoom: 1.0, offset: (40.0, 60.0) };
        } else {
            let m = 40.0;
            let zoom = ((w - 2.0 * m) / (b[2] - b[0])).min((h - 2.0 * m) / (b[3] - b[1])).clamp(transform::ZOOM_MIN, 1.0);
            let cx = (b[0] + b[2]) / 2.0;
            let cy = (b[1] + b[3]) / 2.0;
            s.view = View { zoom, offset: (w / 2.0 - cx * zoom, h / 2.0 - cy * zoom) };
        }
        drop(s);
        self.area.queue_draw();
    }

    #[allow(dead_code)]
    pub fn add_node(&self, node: Node) -> usize {
        let mut s = self.state.borrow_mut();
        let mut node = node;
        node.relayout();
        s.nodes.push(node);
        let i = s.nodes.len() - 1;
        s.order.push(i);
        drop(s);
        self.area.queue_draw();
        i
    }

    #[allow(dead_code)]
    pub fn add_link(&self, l: Link) {
        self.state.borrow_mut().links.push(l);
        self.area.queue_draw();
    }

    pub fn reset_view(&self) {
        self.pending_fit.set(false);
        self.state.borrow_mut().view = View { zoom: 1.0, offset: (40.0, 60.0) };
        self.area.queue_draw();
    }

    /// Programmatic connection by node ids (used after the wire-drop node picker).
    /// Replacing an existing wire into `to_slot` mirrors `finish_link`.
    pub fn connect_ids(&self, from: &str, from_slot: usize, to: &str, to_slot: usize) {
        let mut s = self.state.borrow_mut();
        let (Some(fi), Some(ti)) = (
            s.nodes.iter().position(|n| n.id == from),
            s.nodes.iter().position(|n| n.id == to),
        ) else {
            return;
        };
        s.links.retain(|l| !(l.to == ti && l.to_slot == to_slot));
        s.links.push(Link { from: fi, from_slot, to: ti, to_slot });
        drop(s);
        self.area.queue_draw();
    }


    fn setup_draw(&self) {
        let state = self.state.clone();
        self.area.set_draw_func(move |_, cr, w, h| {
            let s = state.borrow();
            draw(cr, &s, w as f64, h as f64);
        });
    }

    fn setup_input(&self) {
        // Track pointer for zoom anchoring (scroll events carry no position).
        let motion = gtk::EventControllerMotion::new();
        let st = self.state.clone();
        motion.connect_motion(move |_, x, y| st.borrow_mut().pointer = (x, y));
        self.area.add_controller(motion);

        // Scroll = zoom around pointer. Smooth (touchpad) deltas are fractional; powf handles both.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let (st, area) = (self.state.clone(), self.area.clone());
        scroll.connect_scroll(move |_, _, dy| {
            let mut s = st.borrow_mut();
            if s.scroll_text(dy) {
                drop(s);
                area.queue_draw();
                return gtk::glib::Propagation::Stop;
            }
            let p = s.pointer;
            s.view.zoom_at(p, 1.1f64.powf(-dy));
            drop(s);
            area.queue_draw();
            gtk::glib::Propagation::Stop
        });
        self.area.add_controller(scroll);

        // Double-click: activate (open subgraph).
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        let (st, cb) = (self.state.clone(), self.on_activate.clone());
        click.connect_pressed(move |_, n, x, y| {
            if n != 2 {
                return;
            }
            let (id, c) = {
                let s = st.borrow();
                let c = s.view.to_canvas((x, y));
                let hit = s.order.iter().rev().map(|&i| &s.nodes[i]).find(|n| n.contains(c));
                // On a widget row: no activation. On a node: its id. Empty space: "".
                (hit.map_or(Some(String::new()), |n| n.widget_at(c).is_none().then(|| n.id.clone())), c)
            };
            if let (Some(id), Some(f)) = (id, cb.borrow().as_ref()) {
                f(&id, c);
            }
        });
        self.area.add_controller(click);

        // Primary drag: socket → wire, node body → move, empty space → pan.
        // Middle drag: always pan (Helvum).
        {
            let drag = gtk::GestureDrag::builder().button(gdk::BUTTON_PRIMARY).build();
            let (st, area, change) = (self.state.clone(), self.area.clone(), self.on_change.clone());
            drag.connect_drag_begin(move |g, x, y| {
                // Take keyboard focus from the side panels: an entry there commits on
                // focus-out, and Delete/Ctrl+S must reach the canvas again.
                area.grab_focus();
                let shift = g
                    .current_event()
                    .map(|e| e.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
                    .unwrap_or(false);
                let ctrl = g
                    .current_event()
                    .map(|e| e.modifier_state().contains(gdk::ModifierType::CONTROL_MASK))
                    .unwrap_or(false);
                let mut s = st.borrow_mut();
                let c = s.view.to_canvas((x, y));
                let r = 9.0 / s.view.zoom; // constant ~9 screen px, whatever the zoom
                if let Some((idx, out, si)) = s.hit_slot(c, r) {
                    if out {
                        // Shift de-anchors the far end: the wire keeps hanging from the
                        // input it fed and follows the cursor looking for a new output.
                        if shift {
                            if let Some(li) = s.links.iter().rposition(|l| l.from == idx && l.from_slot == si) {
                                let l = s.links.remove(li);
                                let ty = s.nodes[l.to].inputs[l.to_slot].ty.clone();
                                s.drag = Some(Drag::Link(LinkDrag {
                                    node: l.to,
                                    slot: l.to_slot,
                                    out: false,
                                    ty,
                                    to: c,
                                    changed: true,
                                }));
                                drop(s);
                                area.queue_draw();
                                return;
                            }
                        }
                        let ty = s.nodes[idx].outputs[si].ty.clone();
                        s.drag = Some(Drag::Link(LinkDrag {
                            node: idx,
                            slot: si,
                            out: true,
                            ty,
                            to: c,
                            changed: false,
                        }));
                        drop(s);
                        area.queue_draw();
                        return;
                    }
                    // Dragging off a connected input takes that wire with it (anchored at
                    // its output, i.e. the other end).
                    if let Some(li) = s.links.iter().position(|l| l.to == idx && l.to_slot == si) {
                        let l = s.links.remove(li);
                        let ty = s.nodes[l.from].outputs[l.from_slot].ty.clone();
                        s.drag = Some(Drag::Link(LinkDrag {
                            node: l.from,
                            slot: l.from_slot,
                            out: true,
                            ty,
                            to: c,
                            changed: true,
                        }));
                        drop(s);
                        area.queue_draw();
                        return;
                    }
                    // An empty input starts a wire looking for an output; a widget row's dot
                    // is one of those, so pulling it turns the parameter into a link.
                    let ty = s.nodes[idx].inputs[si].ty.clone();
                    s.drag = Some(Drag::Link(LinkDrag { node: idx, slot: si, out: false, ty, to: c, changed: false }));
                    drop(s);
                    area.queue_draw();
                    return;
                }
                // Bottom-right corner: resize grip (litegraph draws it at 12 screen px).
                let hs = 12.0 / s.view.zoom;
                let corner = s.order.iter().rev().copied().find(|&i| {
                    let n = &s.nodes[i];
                    let (cx, cy) = (n.pos.0 + n.size.0, n.pos.1 + n.body_h());
                    c.0 >= cx - hs && c.0 <= cx + hs && c.1 >= cy - hs && c.1 <= cy + hs
                });
                if let Some(idx) = corner {
                    let start = s.nodes[idx].size;
                    let id = s.nodes[idx].id.clone();
                    s.sel.clear();
                    s.sel.insert(id);
                    s.drag = Some(Drag::Resize { idx, start, snap: shift || s.snap_always });
                    drop(s);
                    area.queue_draw();
                    return;
                }
                let hit = s.order.iter().rposition(|&i| s.nodes[i].contains(c));
                if let Some(oi) = hit {
                    // Litegraph's collapse button: the square at the node's top-left title
                    // corner (one click toggles). It sits above the body, so test it before
                    // the widget/body paths.
                    let n = &s.nodes[s.order[oi]];
                    let collapse_hit = c.0 >= n.pos.0
                        && c.0 <= n.pos.0 + TITLE_H
                        && c.1 >= n.pos.1 - TITLE_H
                        && c.1 <= n.pos.1;
                    if collapse_hit {
                        let idx = s.order.remove(oi);
                        s.order.push(idx);
                        let id = s.nodes[idx].id.clone();
                        if !s.sel.contains(&id) {
                            s.sel.clear();
                            s.sel.insert(id.clone());
                        }
                        s.drag = None;
                        let mut changed = false;
                        if !s.nodes[idx].subgraph {
                            s.toggle_collapse(&id);
                            changed = true;
                        } else if let Some(src) = s.pending_collapse.take() {
                            // Second press of a double-click: cancel; the activate gesture enters it.
                            src.remove();
                        } else {
                            let delay = gtk::Settings::default().map_or(400, |g| g.gtk_double_click_time().max(0) as u64);
                            let (st2, area2, change2) = (st.clone(), area.clone(), change.clone());
                            s.pending_collapse = Some(gtk::glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                                {
                                    let mut s = st2.borrow_mut();
                                    s.pending_collapse = None;
                                    s.toggle_collapse(&id);
                                }
                                area2.queue_draw();
                                if let Some(f) = change2.borrow().as_ref() {
                                    f();
                                }
                            }));
                        }
                        drop(s);
                        area.queue_draw();
                        if changed {
                            if let Some(f) = change.borrow().as_ref() {
                                f();
                            }
                        }
                        return;
                    }
                    let idx = s.order.remove(oi);
                    s.order.push(idx); // raise to top
                    let id = s.nodes[idx].id.clone();
                    if !shift {
                        if let Some((wi, zone)) = s.nodes[idx].widget_at(c) {
                            if !s.sel.contains(&id) {
                                s.sel.clear();
                                s.sel.insert(id.clone());
                            }
                            let base = s.nodes[idx].widgets[wi].value.clone();
                            s.drag = Some(Drag::Widget { node: idx, w: wi, zone, base, moved: false });
                            drop(s);
                            area.queue_draw();
                            return;
                        }
                    }
                    if shift {
                        // Shift toggles membership; a node that just left the selection
                        // is not dragged.
                        if !s.sel.insert(id.clone()) {
                            s.sel.remove(&id);
                            s.drag = None;
                            drop(s);
                            area.queue_draw();
                            return;
                        }
                    } else if !s.sel.contains(&id) {
                        s.sel.clear();
                        s.sel.insert(id);
                    }
                    let mut start: Vec<(usize, (f64, f64))> = s
                        .order
                        .iter()
                        .copied()
                        .filter(|&i| s.sel.contains(&s.nodes[i].id) && !s.nodes[i].pinned)
                        .map(|i| (i, s.nodes[i].pos))
                        .collect();
                    if start.is_empty() {
                        start.push((idx, s.nodes[idx].pos));
                    }
                    let p = s.nodes[idx].pos;
                    s.drag = Some(Drag::Nodes { idx, grab: (c.0 - p.0, c.1 - p.1), start });
                } else if let Some(gi) = s.hit_group(c) {
                    let r = s.groups[gi].rect;
                    let nodes: Vec<(usize, (f64, f64))> = s
                        .order
                        .iter()
                        .copied()
                        .filter(|&i| {
                            let n = &s.nodes[i];
                            let (cx, cy) = (n.pos.0 + n.size.0 / 2.0, n.pos.1 + n.body_h() / 2.0);
                            cx >= r[0] && cx <= r[0] + r[2] && cy >= r[1] && cy <= r[1] + r[3]
                        })
                        .filter(|&i| !s.nodes[i].pinned)
                        .map(|i| (i, s.nodes[i].pos))
                        .collect();
                    s.sel.clear();
                    s.drag = Some(Drag::Group { idx: gi, start: r, nodes, grab: c });
                } else if ctrl {
                    s.drag = Some(Drag::Band { start: c, end: c });
                } else {
                    s.sel.clear();
                    s.drag = Some(Drag::Pan { start_offset: s.view.offset });
                }
            });
            let (st, area) = (self.state.clone(), self.area.clone());
            drag.connect_drag_update(move |g, dx, dy| {
                let Some((sx, sy)) = g.start_point() else { return };
                let mut s = st.borrow_mut();
                let st = &mut *s;
                let view = st.view;
                match &mut st.drag {
                    Some(Drag::Nodes { idx, grab, start }) => {
                        let c = view.to_canvas((sx + dx, sy + dy));
                        let base = start.iter().find(|(i, _)| i == idx).map(|(_, p)| *p).unwrap_or(st.nodes[*idx].pos);
                        let d = (c.0 - grab.0 - base.0, c.1 - grab.1 - base.1);
                        for (i, p) in start.iter() {
                            st.nodes[*i].pos = (p.0 + d.0, p.1 + d.1);
                        }
                    }
                    Some(Drag::Group { idx, start, nodes, grab }) => {
                        let c = view.to_canvas((sx + dx, sy + dy));
                        let d = (c.0 - grab.0, c.1 - grab.1);
                        st.groups[*idx].rect = [start[0] + d.0, start[1] + d.1, start[2], start[3]];
                        for (i, p) in nodes.iter() {
                            st.nodes[*i].pos = (p.0 + d.0, p.1 + d.1);
                        }
                    }
                    Some(Drag::Resize { idx, snap, .. }) => {
                        let c = view.to_canvas((sx + dx, sy + dy));
                        let grid = if *snap { st.snap_grid } else { 0.0 };
                        let (w, h) = (c.0 - st.nodes[*idx].pos.0, c.1 - st.nodes[*idx].pos.1);
                        st.nodes[*idx].size = (snap_to(w, grid).max(120.0), snap_to(h, grid).max(44.0));
                        st.nodes[*idx].relayout();
                    }
                    Some(Drag::Band { end, .. }) => *end = view.to_canvas((sx + dx, sy + dy)),
                    Some(Drag::Pan { start_offset }) => st.view.offset = (start_offset.0 + dx, start_offset.1 + dy),
                    Some(Drag::Link(d)) => d.to = view.to_canvas((sx + dx, sy + dy)),
                    Some(Drag::Widget { node, w, zone, base, moved }) => {
                        let wd = &mut st.nodes[*node].widgets[*w];
                        if *zone != Zone::Main || !matches!(wd.kind, WKind::Int { .. } | WKind::Float { .. }) {
                            return;
                        }
                        if dx.abs() > 4.0 {
                            *moved = true;
                        }
                        if *moved {
                            let steps = (dx / 6.0).trunc();
                            wd.value = widgets::num_value(&wd.kind, widgets::as_num(base) + steps * widgets::step_of(&wd.kind));
                        }
                    }
                    None => return,
                }
                drop(s);
                area.queue_draw();
            });
            let (st, area, cb, me) = (self.state.clone(), self.area.clone(), self.on_change.clone(), self.clone());
            drag.connect_drag_end(move |g, _, _| {
                let shift = g
                    .current_event()
                    .map(|e| e.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
                    .unwrap_or(false);
                let mut s = st.borrow_mut();
                let r = 9.0 / s.view.zoom;
                let mut changed = false;
                let mut widget_act = None;
                let mut link_release = None;
                match s.drag.take() {
                    Some(Drag::Widget { node, w, zone, moved, .. }) => widget_act = Some((node, w, zone, moved)),
                    Some(Drag::Link(mut d)) => {
                        let connected = s.finish_link(&mut d, r);
                        changed = d.changed;
                        if !connected {
                            link_release = Some((
                                s.nodes[d.node].id.clone(),
                                d.slot,
                                d.out,
                                d.ty.clone(),
                                d.to,
                            ));
                        }
                    }
                    Some(Drag::Nodes { start, .. }) => {
                        if shift || s.snap_always {
                            let grid = s.snap_grid;
                            for (i, _) in start.iter() {
                                let n = &mut s.nodes[*i];
                                n.pos = (snap_to(n.pos.0, grid), snap_to(n.pos.1, grid));
                            }
                        }
                        changed = start.iter().any(|(i, p)| s.nodes[*i].pos != *p);
                    }
                    Some(Drag::Resize { idx, start, .. }) => changed = s.nodes[idx].size != start,
                    Some(Drag::Group { idx, start, nodes, .. }) => {
                        changed = s.groups[idx].rect != start || nodes.iter().any(|(i, p)| s.nodes[*i].pos != *p)
                    }
                    Some(Drag::Band { start, end }) => {
                        let band = [
                            start.0.min(end.0),
                            start.1.min(end.1),
                            (end.0 - start.0).abs(),
                            (end.1 - start.1).abs(),
                        ];
                        if band[2] > 4.0 || band[3] > 4.0 {
                            let ids: Vec<String> = s
                                .order
                                .iter()
                                .filter(|&&i| {
                                    let n = &s.nodes[i];
                                    let r = [n.pos.0, n.pos.1 - TITLE_H, n.size.0, n.body_h() + TITLE_H];
                                    r[0] <= band[0] + band[2]
                                        && r[0] + r[2] >= band[0]
                                        && r[1] <= band[1] + band[3]
                                        && r[1] + r[3] >= band[1]
                                })
                                .map(|&i| s.nodes[i].id.clone())
                                .collect();
                            s.sel.extend(ids);
                        }
                    }
                    _ => {}
                }
                let scrubbed = widget_act.filter(|a| a.3).and_then(|(n, w, ..)| {
                    let n = s.nodes.get(n)?;
                    Some((n.id.clone(), n.widgets.get(w)?.key.clone(), n.widgets.get(w)?.value.clone()))
                });
                drop(s);
                area.queue_draw();
                if changed {
                    if let Some(f) = cb.borrow().as_ref() {
                        f();
                    }
                }
                if let Some((id, slot, out, ty, pos)) = link_release {
                    if let Some(f) = me.on_link_menu.borrow().as_ref() {
                        f(&id, slot, out, &ty, pos);
                    }
                }
                match (widget_act, scrubbed) {
                    (_, Some((id, key, v))) => me.emit(&id, &key, v, true),
                    (Some((n, w, zone, false)), _) => me.activate_widget(n, w, zone),
                    _ => {}
                }
            });
            self.area.add_controller(drag);
        }
        {
            let drag = gtk::GestureDrag::builder().button(gdk::BUTTON_MIDDLE).build();
            let st = self.state.clone();
            drag.connect_drag_begin(move |_, _, _| {
                let mut s = st.borrow_mut();
                s.drag = Some(Drag::Pan { start_offset: s.view.offset });
            });
            let (st, area) = (self.state.clone(), self.area.clone());
            drag.connect_drag_update(move |_, dx, dy| {
                let mut s = st.borrow_mut();
                if let Some(Drag::Pan { start_offset }) = s.drag {
                    s.view.offset = (start_offset.0 + dx, start_offset.1 + dy);
                    drop(s);
                    area.queue_draw();
                }
            });
            let st = self.state.clone();
            drag.connect_drag_end(move |_, _, _| st.borrow_mut().drag = None);
            self.area.add_controller(drag);
        }
        // Right click: report the target; the workspace shows the menu.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let (st, cb) = (self.state.clone(), self.on_context.clone());
        click.connect_pressed(move |_, _, x, y| {
            let mut s = st.borrow_mut();
            let c = s.view.to_canvas((x, y));
            let hit = match s.hit_node(c) {
                // Right-clicking an unselected node selects it, as in every other editor.
                Some(i) => {
                    let id = s.nodes[i].id.clone();
                    if !s.sel.contains(&id) {
                        s.sel.clear();
                        s.sel.insert(id.clone());
                    }
                    // A widget row has its own menu (Power Lora rows, converted parameters…).
                    match s.nodes[i].widget_at(c) {
                        Some((w, _)) => Hit::Widget(id, w),
                        None => Hit::Node(id),
                    }
                }
                None => match s.group_at(c) {
                    Some(g) => Hit::Group(g),
                    None => Hit::Empty,
                },
            };
            drop(s);
            if let Some(f) = cb.borrow().as_ref() {
                f(x, y, hit);
            }
        });
        self.area.add_controller(click);
    }
}

/// Mask tint and numbered boxes over a preview drawn at `k` (image px → canvas units).
fn draw_overlay(cr: &gtk::cairo::Context, o: &Overlay, images: &HashMap<String, Option<gtk::cairo::ImageSurface>>, (iw, ih): (f64, f64), k: f64) {
    if let Some(Some(m)) = o.mask.as_ref().and_then(|key| images.get(key)) {
        cr.save().ok();
        cr.scale(iw / m.width() as f64, ih / m.height() as f64);
        cr.set_source_rgba(214.0 / 255.0, 64.0 / 255.0, 230.0 / 255.0, 0.55);
        cr.mask_surface(m, 0.0, 0.0).ok();
        cr.restore().ok();
    }
    cr.set_line_width(1.5 / k);
    cr.set_font_size(11.0 / k);
    for (i, b) in o.boxes.iter().enumerate() {
        let c = BOX_COLORS[i % BOX_COLORS.len()];
        rgb(cr, c);
        cr.rectangle(b[0] * iw, b[1] * ih, b[2] * iw, b[3] * ih);
        cr.stroke().ok();
        cr.rectangle(b[0] * iw, b[1] * ih, 14.0 / k, 14.0 / k);
        cr.fill().ok();
        rgb(cr, 0x000000);
        cr.move_to(b[0] * iw + 3.0 / k, b[1] * ih + 11.0 / k);
        cr.show_text(&(i + 1).to_string()).ok();
    }
}

fn rgb(cr: &gtk::cairo::Context, hex: u32) {
    cr.set_source_rgb(
        ((hex >> 16) & 0xff) as f64 / 255.0,
        ((hex >> 8) & 0xff) as f64 / 255.0,
        (hex & 0xff) as f64 / 255.0,
    );
}

fn rgba(cr: &gtk::cairo::Context, hex: u32, a: f64) {
    cr.set_source_rgba(
        ((hex >> 16) & 0xff) as f64 / 255.0,
        ((hex >> 8) & 0xff) as f64 / 255.0,
        (hex & 0xff) as f64 / 255.0,
        a,
    );
}

/// "#rrggbb" / "#rgb" (litegraph node/group colors). Alpha suffixes are ignored.
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    match h.len() {
        3 | 4 => {
            let v = u32::from_str_radix(&h[..3], 16).ok()?;
            Some(((v >> 8) & 0xf) * 0x110000 + ((v >> 4) & 0xf) * 0x1100 + (v & 0xf) * 0x11)
        }
        6 | 8 => u32::from_str_radix(&h[..6], 16).ok(),
        _ => None,
    }
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// A link path in the mode `Comfy.LinkRenderMode` selects, mirroring litegraph's
/// `pathRenderer`: 0 straight (orthogonal H), 1 linear (diagonal), 2 spline (bezier).
fn link_path(cr: &gtk::cairo::Context, mode: i32, (ox, oy): (f64, f64), (ix, iy): (f64, f64)) {
    match mode {
        0 => {
            // Juts out of both sockets, then runs between the two middle columns.
            let (ax, bx) = (ox + 10.0, ix - 10.0);
            let mid = (ax + bx) * 0.5;
            cr.move_to(ox, oy);
            cr.line_to(ax, oy);
            cr.line_to(mid, oy);
            cr.line_to(mid, iy);
            cr.line_to(bx, iy);
            cr.line_to(ix, iy);
        }
        1 => {
            cr.move_to(ox, oy);
            cr.line_to(ox + 15.0, oy);
            cr.line_to(ix - 15.0, iy);
            cr.line_to(ix, iy);
        }
        _ => {
            let d = (ix - ox).hypot(iy - oy);
            let cd = f64::max(30.0, d * 0.25);
            cr.move_to(ox, oy);
            cr.curve_to(ox + cd, oy, ix - cd, iy, ix, iy);
        }
    }
}

fn draw(cr: &gtk::cairo::Context, s: &State, w: f64, h: f64) {
    // ponytail: cairo errors (OOM / invalid surface) are ignored with `.ok()`; nothing useful to recover.
    rgb(cr, 0x202020);
    cr.paint().ok();

    let v = s.view;
    cr.save().ok();
    cr.translate(v.offset.0, v.offset.1);
    cr.scale(v.zoom, v.zoom);

    // Grid (only visible range; skipped when lines would be < 6px apart).
    if GRID * v.zoom >= 6.0 {
        let (x0, y0) = v.to_canvas((0.0, 0.0));
        let (x1, y1) = v.to_canvas((w, h));
        // Two strokes: faint minor lines, slightly brighter every 5th (litegraph-style).
        cr.set_line_width(1.0 / v.zoom);
        for (major, color) in [(false, 0x262626), (true, 0x2e2e2e)] {
            rgb(cr, color);
            let step = |k: f64| ((k / GRID).round() as i64).rem_euclid(5) == 0;
            let mut x = (x0 / GRID).floor() * GRID;
            while x <= x1 { if step(x) == major { cr.move_to(x, y0); cr.line_to(x, y1); } x += GRID; }
            let mut y = (y0 / GRID).floor() * GRID;
            while y <= y1 { if step(y) == major { cr.move_to(x0, y); cr.line_to(x1, y); } y += GRID; }
            cr.stroke().ok();
        }
    }

    let (vx0, vy0) = v.to_canvas((0.0, 0.0));
    let (vx1, vy1) = v.to_canvas((w, h));
    let visible = |x: f64, y: f64, w: f64, h: f64| x + w >= vx0 && x <= vx1 && y + h >= vy0 && y <= vy1;
    cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);

    // Groups (behind everything).
    for g in &s.groups {
        let [x, y, gw, gh] = g.rect;
        if !visible(x, y, gw, gh) { continue; }
        let c = g.color.unwrap_or(0x3f789e);
        rgba(cr, c, 0.25);
        cr.rectangle(x, y, gw, gh);
        cr.fill().ok();
        rgba(cr, c, 0.6);
        cr.rectangle(x, y, gw, 34.0);
        cr.fill().ok();
        rgb(cr, 0xdddddd);
        cr.set_font_size(22.0);
        cr.move_to(x + 8.0, y + 25.0);
        text::show(cr, &g.title);
    }

    // `Comfy.LinkRenderMode` -1 hides them entirely (litegraph's Hidden).
    if s.link_mode >= 0 {
        // Links under nodes, batched by socket type so each colour is a single stroke.
        let mut by_color: Vec<(u32, Vec<((f64, f64), (f64, f64))>)> = Vec::new();
        for l in &s.links {
            let (Some(a), Some(b)) = (s.nodes.get(l.from), s.nodes.get(l.to)) else { continue };
            let (o, i) = (a.slot_pos(true, l.from_slot), b.slot_pos(false, l.to_slot));
            let half = (o.0 - i.0).abs() / 2.0;
            if !visible(o.0.min(i.0) - half, o.1.min(i.1) - 30.0, (o.0 - i.0).abs() + 2.0 * half, (o.1 - i.1).abs() + 30.0) {
                continue;
            }
            let c = type_color(a.outputs.get(l.from_slot).map(|s| s.ty.as_str()).unwrap_or_default());
            match by_color.iter_mut().find(|(k, _)| *k == c) {
                Some((_, v)) => v.push((o, i)),
                None => by_color.push((c, vec![(o, i)])),
            }
        }
        // Dark casing under the coloured wires keeps them readable where they cross.
        rgba(cr, 0x000000, 0.45);
        cr.set_line_width(5.0);
        for (_, wires) in &by_color {
            for &(o, i) in wires {
                link_path(cr, s.link_mode, o, i);
            }
        }
        cr.stroke().ok();
        cr.set_line_width(3.0);
        for (color, wires) in &by_color {
            rgb(cr, *color);
            for &(o, i) in wires {
                link_path(cr, s.link_mode, o, i);
            }
            cr.stroke().ok();
        }

        // Bypassed nodes (mode 4) forward an input to a same-type output, so the graph
        // still reads as connected without tracing the wire into the node. Under the nodes,
        // like the links — drawn later they'd show through the node bodies.
        cr.save().ok();
        rgba(cr, 0xffffff, 0.45);
        cr.set_line_width(1.5);
        cr.set_dash(&[6.0, 4.0], 0.0);
        for &i in &s.order {
            let n = &s.nodes[i];
            if n.mode != 4 || n.outputs.is_empty() {
                continue;
            }
            for (si, slot) in n.inputs.iter().enumerate() {
                if !s.links.iter().any(|l| l.to == i && l.to_slot == si) {
                    continue;
                }
                let j = n.outputs.iter().position(|o| compatible(&o.ty, &slot.ty)).unwrap_or(0);
                link_path(cr, s.link_mode, n.slot_pos(true, j), n.slot_pos(false, si));
                cr.stroke().ok();
            }
        }
        cr.set_dash(&[], 0.0);
        cr.restore().ok();

    }

    let detail = v.zoom >= 0.4; // skip text when too small to read
    let connected: HashSet<(usize, usize)> = s.links.iter().map(|l| (l.to, l.to_slot)).collect();
    for &i in &s.order {
        let n = &s.nodes[i];
        let (x, y) = n.pos;
        let (nw, nh) = (n.size.0, n.body_h());
        if !visible(x, y - TITLE_H, nw, nh + TITLE_H) { continue; }
        let alpha = if n.mode == 2 { 0.4 } else { 1.0 };
        // Cheap drop shadow: one offset translucent fill (no blur).
        rounded_rect(cr, x + 2.0, y - TITLE_H + 4.0, nw, nh + TITLE_H, 9.0);
        rgba(cr, 0x000000, 0.3 * alpha);
        cr.fill().ok();
        rounded_rect(cr, x, y - TITLE_H, nw, nh + TITLE_H, 8.0);
        rgba(cr, if n.mode == 4 { 0x9a3a9a } else { n.bgcolor.unwrap_or(0x353535) }, alpha);
        cr.fill_preserve().ok();
        let dragging = matches!(&s.drag, Some(Drag::Nodes { idx, .. }) if *idx == i) || s.sel.contains(&n.id);
        rgb(cr, if dragging { 0xffffff } else { 0x111111 });
        cr.set_line_width(if dragging { 2.0 } else { 1.0 });
        cr.stroke().ok();

        // Title bar
        cr.save().ok();
        rounded_rect(cr, x, y - TITLE_H, nw, nh + TITLE_H, 8.0);
        cr.clip();
        rgba(cr, if n.mode == 4 { 0x7a2a7a } else { n.color.unwrap_or(0x333333) }, alpha);
        cr.rectangle(x, y - TITLE_H, nw, TITLE_H);
        cr.fill().ok();
        if !n.collapsed {
            rgba(cr, 0x000000, 0.25);
            cr.rectangle(x, y - 1.0, nw, 1.0);
            cr.fill().ok();
        }
        if !detail {
            cr.restore().ok();
            continue;
        }
        cr.set_font_size(13.0);
        rgb(cr, 0xd4d4d4);
        cr.move_to(x + 28.0, y - TITLE_H / 2.0 + 5.0);
        text::show(cr, &widgets::fit(cr, &n.title, nw - 36.0));
        cr.restore().ok();
        rgb(cr, 0x999999);
        if n.subgraph {
            // two stacked squares = "contains a graph"
            cr.rectangle(x + 8.0, y - TITLE_H / 2.0 - 6.0, 8.0, 8.0);
            cr.rectangle(x + 12.0, y - TITLE_H / 2.0 - 2.0, 8.0, 8.0);
            cr.set_line_width(1.5);
            cr.stroke().ok();
        } else {
            cr.arc(x + 14.0, y - TITLE_H / 2.0, 5.0, 0.0, 2.0 * PI);
            cr.fill().ok();
        }
        if s.sel.contains(&n.id) {
            rgb(cr, 0xffffff);
            cr.set_line_width(2.0 / v.zoom);
            rounded_rect(cr, x - 3.0, y - TITLE_H - 3.0, nw + 6.0, nh + TITLE_H + 6.0, 10.0);
            cr.stroke().ok();
        }
        if s.running.as_deref() == Some(n.id.as_str()) {
            rgb(cr, 0x3fc35a);
            cr.set_line_width(3.0 / v.zoom);
            rounded_rect(cr, x - 2.0, y - TITLE_H - 2.0, nw + 4.0, nh + TITLE_H + 4.0, 9.0);
            cr.stroke().ok();
        }
        if n.collapsed { continue; }
        // Resize grip, bottom-right (litegraph paints one in the corner too).
        rgb(cr, if s.sel.contains(&n.id) { 0xffffff } else { 0x5a5a5a });
        cr.set_line_width(2.0 / v.zoom);
        cr.move_to(x + nw - 9.0, y + nh - 1.0);
        cr.line_to(x + nw - 1.0, y + nh - 9.0);
        cr.stroke().ok();
        // "Add input" sockets are drawn after the clip so the ring stays whole.
        let mut add: Option<(f64, f64)> = None;

        // Slots
        cr.save().ok();
        cr.rectangle(x, y, nw, nh);
        cr.clip();
        cr.set_font_size(12.0);
        for (out, slots) in [(false, &n.inputs), (true, &n.outputs)] {
            for (si, slot) in slots.iter().enumerate() {
                let (px, py) = n.slot_pos(out, si);
                // Widget sockets are always visible (frontend 1.60 keeps the widget and its
                // socket side by side): a small dot on the row, a full one once wired.
                let shadowed = !out && n.widgets.iter().any(|w| w.input == Some(si));
                let faint = shadowed && !connected.contains(&(i, si));
                rgb(cr, type_color(&slot.ty));
                let bare = slot.label.is_empty() && slot.ty.is_empty();
                if bare {
                    add = Some((px, py));
                } else if faint {
                    rgba(cr, type_color(&slot.ty), 0.6);
                    cr.arc(px, py, 3.2, 0.0, 2.0 * PI);
                    cr.fill().ok();
                } else {
                    cr.arc(px, py, 5.0, 0.0, 2.0 * PI);
                    cr.fill().ok();
                }
                // Ring the socket a dragged wire would land in (the free end's side).
                if let Some(Drag::Link(d)) = &s.drag {
                    if d.out != out {
                        let ok = if out { compatible(&slot.ty, &d.ty) } else { compatible(&d.ty, &slot.ty) };
                        if ok && (px - d.to.0).hypot(py - d.to.1) <= 12.0 / v.zoom {
                            rgb(cr, 0xffffff);
                            cr.set_line_width(2.0);
                            cr.arc(px, py, 8.5, 0.0, 2.0 * PI);
                            cr.stroke().ok();
                        }
                    }
                }
                if shadowed { continue; }
                rgb(cr, 0xaaaaaa);
                let tw = text::width(cr, &slot.label);
                cr.move_to(if out { px - 12.0 - tw } else { px + 12.0 }, py + 4.0);
                text::show(cr, &slot.label);
            }
        }
        // Widget rows
        for (wi, w) in n.widgets.iter().enumerate() {
            let Some(r) = n.rows.get(wi) else { continue };
            let linked = w.input.is_some_and(|k| connected.contains(&(i, k)));
            if matches!(w.kind, WKind::Cloud(..)) {
                widgets::draw_cloud(cr, w, [x + r[0], y + r[1], r[2], r[3]], &s.images);
            } else {
                let scroll = s.text_scroll.get(&n.id).and_then(|m| m.get(&w.key)).copied().unwrap_or(0.0);
                widgets::draw(cr, w, [x + r[0], y + r[1], r[2], r[3]], linked, scroll);
            }
        }
        if let (Some(r), Some(key)) = (n.img_rect, n.images.first()) {
            let (rx, ry, rw, rh) = (x + r[0], y + r[1], r[2], r[3]);
            match s.images.get(key) {
                Some(Some(surf)) => {
                    let (iw, ih) = (surf.width() as f64, surf.height() as f64);
                    let k = (rw / iw).min(rh / ih);
                    cr.save().ok();
                    cr.translate(rx + (rw - iw * k) / 2.0, ry + (rh - ih * k) / 2.0);
                    cr.scale(k, k);
                    if cr.set_source_surface(surf, 0.0, 0.0).is_ok() {
                        cr.source().set_filter(gtk::cairo::Filter::Good);
                        cr.paint().ok();
                    }
                    if let Some(o) = &n.overlay {
                        if o.alpha_mask {
                            // Tint everywhere, then cut it away where the picture is opaque.
                            cr.push_group();
                            cr.set_source_rgba(214.0 / 255.0, 64.0 / 255.0, 230.0 / 255.0, 0.55);
                            cr.paint().ok();
                            cr.set_operator(gtk::cairo::Operator::DestOut);
                            cr.set_source_rgba(0.0, 0.0, 0.0, 1.0); // erase fully, not by the tint's alpha
                            cr.mask_surface(surf, 0.0, 0.0).ok();
                            cr.pop_group_to_source().ok();
                            cr.rectangle(0.0, 0.0, iw, ih);
                            cr.fill().ok();
                        }
                        draw_overlay(cr, o, &s.images, (iw, ih), k);
                    }
                    cr.restore().ok();
                }
                _ => {
                    rgb(cr, 0x2a2a2a);
                    cr.rectangle(rx, ry, rw, rh);
                    cr.fill().ok();
                }
            }
        }
        cr.restore().ok();
        if let Some((px, py)) = add {
            // Dashed ring: drag a wire here to add a parameter to the subgraph.
            rgb(cr, 0x8ad0a0);
            cr.set_line_width(1.6 / v.zoom);
            cr.set_dash(&[3.5, 3.0], 0.0);
            cr.arc(px, py, 6.5, 0.0, 2.0 * PI);
            cr.stroke().ok();
            cr.set_dash(&[], 0.0);
        }
    }

    if let Some(Drag::Band { start, end }) = &s.drag {
        let (x0, y0) = (start.0.min(end.0), start.1.min(end.1));
        let (bw, bh) = ((start.0 - end.0).abs(), (start.1 - end.1).abs());
        rgba(cr, 0x7aa2ff, 0.12);
        cr.rectangle(x0, y0, bw, bh);
        cr.fill().ok();
        rgba(cr, 0x7aa2ff, 0.9);
        cr.set_line_width(1.0 / v.zoom);
        cr.rectangle(x0, y0, bw, bh);
        cr.stroke().ok();
    }

    // Wire being dragged, on top of everything (litegraph CONNECTING_LINK_COLOR).
    if let Some(Drag::Link(d)) = &s.drag {
        if let Some(n) = s.nodes.get(d.node) {
            rgb(cr, 0xaaffaa);
            cr.set_line_width(3.0);
            link_path(cr, s.link_mode, n.slot_pos(d.out, d.slot), d.to);
            cr.stroke().ok();
        }
    }
    cr.restore().ok();

    // HUD
    rgb(cr, 0x777777);
    cr.set_font_size(11.0);
    cr.move_to(8.0, h - 8.0);
    text::show(cr, &format!("zoom {:.0}%", v.zoom * 100.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_sockets_accept_anything() {
        assert!(compatible("*", "IMAGE"));
        assert!(compatible("IMAGE", "*"));
        assert!(compatible("MASK", "IMAGE,MASK"));
        assert!(!compatible("LATENT", "IMAGE"));
    }

    /// A widget row's dot is a socket: pulling it starts a wire, and dropping that wire on
    /// an output links the parameter (the web UI's "convert widget to input" drag).
    #[test]
    fn widget_dot_starts_a_wire_that_lands_on_an_output() {
        let mut a = Node {
            id: "a".to_owned(),
            pos: (0.0, 0.0),
            size: (200.0, 100.0),
            widgets: vec![Widget::new("text", "0", WKind::Text { multiline: true }, Value::String(String::new()))],
            ..Default::default()
        };
        a.inputs.push(Slot { label: "text".to_owned(), ty: "STRING".to_owned() });
        a.widgets[0].input = Some(0);
        a.relayout();
        let mut b = Node {
            id: "b".to_owned(),
            pos: (300.0, 0.0),
            size: (200.0, 100.0),
            outputs: vec![Slot { label: "STRING".to_owned(), ty: "STRING".to_owned() }],
            ..Default::default()
        };
        b.relayout();

        let mut s = State::default();
        s.nodes = vec![a, b];
        s.order = vec![0, 1];

        let p = s.nodes[0].slot_pos(false, 0);
        assert_eq!(s.hit_slot(p, 9.0), Some((0, false, 0)), "the dot must be grabbable");

        let mut d = LinkDrag { node: 0, slot: 0, out: false, ty: "STRING".to_owned(), to: s.nodes[1].slot_pos(true, 0), changed: false };
        s.finish_link(&mut d, 9.0);
        assert_eq!(s.links, vec![Link { from: 1, from_slot: 0, to: 0, to_slot: 0 }]);
        assert!(d.changed);
    }

    #[test]
    fn snap_rounds_to_the_grid_and_zero_keeps_the_value() {
        assert_eq!(snap_to(37.0, 20.0), 40.0);
        assert_eq!(snap_to(37.0, 0.0), 37.0);
    }
}
