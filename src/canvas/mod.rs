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

pub mod transform;

use adw::{gdk, gtk, prelude::*};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::f64::consts::PI;
use std::rc::Rc;
use transform::View;

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
    /// (name, value) rows drawn below the slots. Read-only in Phase 2.
    pub widgets: Vec<(String, String)>,
    pub color: Option<u32>,
    pub bgcolor: Option<u32>,
    /// litegraph mode: 2 = muted, 4 = bypassed.
    pub mode: i64,
    pub collapsed: bool,
    /// Subgraph instance: drawn with a marker, double-click opens it.
    pub subgraph: bool,
}

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
        (x, self.pos.1 + (i as f64 + 0.7) * SLOT_H)
    }
}

/// What a click landed on, in canvas coordinates, for context menus.
#[derive(Clone, Debug)]
pub enum Hit {
    Node(String),
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
}

impl State {
    /// Topmost socket within `r` canvas units of `p`.
    fn hit_slot(&self, p: (f64, f64), r: f64) -> Option<(usize, bool, usize)> {
        for &i in self.order.iter().rev() {
            let n = &self.nodes[i];
            for (out, slots) in [(true, &n.outputs), (false, &n.inputs)] {
                for (si, _) in slots.iter().enumerate() {
                    let (sx, sy) = n.slot_pos(out, si);
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
    fn finish_link(&mut self, d: &mut LinkDrag, r: f64) {
        let Some((other, slot)) = self.hit_target(d.to, r, &d.ty, !d.out) else { return };
        if d.out {
            self.links.retain(|l| !(l.to == other && l.to_slot == slot));
            self.links.push(Link { from: d.node, from_slot: d.slot, to: other, to_slot: slot });
        } else {
            self.links.push(Link { from: other, from_slot: slot, to: d.node, to_slot: d.slot });
        }
        d.changed = true;
    }
}

/// Socket types match when identical, when either side is a wildcard, or when the
/// input lists alternatives (`"IMAGE,MASK"`).
fn compatible(out_ty: &str, in_ty: &str) -> bool {
    let ok = |t: &str| t == out_ty || t == "*" || out_ty.is_empty();
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

#[derive(Clone)]
pub struct Canvas {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
    on_activate: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    on_change: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    on_context: Rc<RefCell<Option<Box<dyn Fn(f64, f64, Hit)>>>>,
    /// A graph was set before the widget had a size: fit as soon as it does.
    pending_fit: Rc<Cell<bool>>,
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
            pending_fit: Rc::default(),
        };
        let me = c.clone();
        let pending = c.pending_fit.clone();
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
        s.nodes = nodes;
        s.links = links;
        s.groups = groups;
        s.drag = None;
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

    /// Restore a saved viewport (`extra.ds` in workflow files).
    /// Restore an explicit viewport (wins over the initial auto-fit).
    pub fn set_view(&self, zoom: f64, offset: (f64, f64)) {
        self.pending_fit.set(false);
        let mut s = self.state.borrow_mut();
        s.view = View { zoom: zoom.clamp(transform::ZOOM_MIN, transform::ZOOM_MAX), offset };
        drop(s);
        self.area.queue_draw();
    }

    /// Double-click on a node (by id).
    pub fn connect_activate(&self, f: impl Fn(&str) + 'static) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// A node moved or a wire was connected/disconnected (edits to persist).
    pub fn connect_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// Right click on the canvas: what sits under the pointer.
    pub fn connect_context(&self, f: impl Fn(f64, f64, Hit) + 'static) {
        *self.on_context.borrow_mut() = Some(Box::new(f));
    }

    /// Selected node ids, sorted for stable output.
    pub fn selected(&self) -> Vec<String> {
        let mut v: Vec<String> = self.state.borrow().sel.iter().cloned().collect();
        v.sort();
        v
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
            let id = {
                let s = st.borrow();
                let c = s.view.to_canvas((x, y));
                s.order.iter().rev().map(|&i| &s.nodes[i]).find(|n| n.contains(c)).map(|n| n.id.clone())
            };
            if let (Some(id), Some(f)) = (id, cb.borrow().as_ref()) {
                f(&id);
            }
        });
        self.area.add_controller(click);

        // Primary drag: socket → wire, node body → move, empty space → pan.
        // Middle drag: always pan (Helvum).
        {
            let drag = gtk::GestureDrag::builder().button(gdk::BUTTON_PRIMARY).build();
            let (st, area) = (self.state.clone(), self.area.clone());
            drag.connect_drag_begin(move |g, x, y| {
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
                }
                let hit = s.order.iter().rposition(|&i| s.nodes[i].contains(c));
                if let Some(oi) = hit {
                    let idx = s.order.remove(oi);
                    s.order.push(idx); // raise to top
                    let id = s.nodes[idx].id.clone();
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
                        .filter(|&i| s.sel.contains(&s.nodes[i].id))
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
                    Some(Drag::Band { end, .. }) => *end = view.to_canvas((sx + dx, sy + dy)),
                    Some(Drag::Pan { start_offset }) => st.view.offset = (start_offset.0 + dx, start_offset.1 + dy),
                    Some(Drag::Link(d)) => d.to = view.to_canvas((sx + dx, sy + dy)),
                    None => return,
                }
                drop(s);
                area.queue_draw();
            });
            let (st, area, cb) = (self.state.clone(), self.area.clone(), self.on_change.clone());
            drag.connect_drag_end(move |_, _, _| {
                let mut s = st.borrow_mut();
                let r = 9.0 / s.view.zoom;
                let mut changed = false;
                match s.drag.take() {
                    Some(Drag::Link(mut d)) => {
                        s.finish_link(&mut d, r);
                        changed = d.changed;
                    }
                    Some(Drag::Nodes { start, .. }) => changed = start.iter().any(|(i, p)| s.nodes[*i].pos != *p),
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
                drop(s);
                area.queue_draw();
                if changed {
                    if let Some(f) = cb.borrow().as_ref() {
                        f();
                    }
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
                    Hit::Node(id)
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

/// Helvum's link curve: control points offset by half the x-distance, with a
/// vertical kick when the output sits right of the input so the wire loops visibly.
fn link_path(cr: &gtk::cairo::Context, (ox, oy): (f64, f64), (ix, iy): (f64, f64)) {
    let y_kick = if ox > ix { f64::max(0.0, 25.0 - (oy - iy).abs()) } else { 0.0 };
    let half = (ox - ix).abs() / 2.0;
    cr.move_to(ox, oy);
    cr.curve_to(ox + half, oy - y_kick, ix - half, iy - y_kick, ix, iy);
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
        rgb(cr, 0x2a2a2a);
        cr.set_line_width(1.0 / v.zoom);
        let mut x = (x0 / GRID).floor() * GRID;
        while x <= x1 { cr.move_to(x, y0); cr.line_to(x, y1); x += GRID; }
        let mut y = (y0 / GRID).floor() * GRID;
        while y <= y1 { cr.move_to(x0, y); cr.line_to(x1, y); y += GRID; }
        cr.stroke().ok();
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
        cr.show_text(&g.title).ok();
    }

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
    cr.set_line_width(3.0);
    for (color, wires) in &by_color {
        rgb(cr, *color);
        for &(o, i) in wires {
            link_path(cr, o, i);
        }
        cr.stroke().ok();
    }

    let detail = v.zoom >= 0.4; // skip text when too small to read
    for &i in &s.order {
        let n = &s.nodes[i];
        let (x, y) = n.pos;
        let (nw, nh) = (n.size.0, n.body_h());
        if !visible(x, y - TITLE_H, nw, nh + TITLE_H) { continue; }
        let alpha = if n.mode == 2 { 0.4 } else { 1.0 };
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
        if !detail {
            cr.restore().ok();
            continue;
        }
        cr.set_font_size(13.0);
        rgb(cr, 0xbbbbbb);
        cr.move_to(x + 28.0, y - TITLE_H / 2.0 + 5.0);
        cr.show_text(&n.title).ok();
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
        if n.collapsed { continue; }

        // Slots
        cr.save().ok();
        cr.rectangle(x, y, nw, nh);
        cr.clip();
        cr.set_font_size(12.0);
        for (out, slots) in [(false, &n.inputs), (true, &n.outputs)] {
            for (si, slot) in slots.iter().enumerate() {
                let (px, py) = n.slot_pos(out, si);
                rgb(cr, type_color(&slot.ty));
                cr.arc(px, py, 5.0, 0.0, 2.0 * PI);
                cr.fill().ok();
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
                rgb(cr, 0xaaaaaa);
                let tw = cr.text_extents(&slot.label).map(|e| e.width()).unwrap_or(0.0);
                cr.move_to(if out { px - 12.0 - tw } else { px + 12.0 }, py + 4.0);
                cr.show_text(&slot.label).ok();
            }
        }
        // Widget rows (read-only values)
        let mut wy = y + n.inputs.len().max(n.outputs.len()) as f64 * SLOT_H + 4.0;
        for (name, val) in &n.widgets {
            if wy + 20.0 > y + nh { break; }
            rounded_rect(cr, x + 10.0, wy, nw - 20.0, 20.0, 6.0);
            rgb(cr, 0x222222);
            cr.fill().ok();
            rgb(cr, 0x888888);
            cr.move_to(x + 18.0, wy + 14.0);
            cr.show_text(name).ok();
            rgb(cr, 0xdddddd);
            let tw = cr.text_extents(val).map(|e| e.width()).unwrap_or(0.0);
            cr.move_to((x + nw - 18.0 - tw).max(x + nw / 2.0), wy + 14.0);
            cr.show_text(val).ok();
            wy += 24.0;
        }
        cr.restore().ok();
    }

    // Bypassed nodes (mode 4) forward an input to a same-type output, so the graph
    // still reads as connected without tracing the wire into the node.
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
            link_path(cr, n.slot_pos(true, j), n.slot_pos(false, si));
            cr.stroke().ok();
        }
    }
    cr.set_dash(&[], 0.0);
    cr.restore().ok();

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
            link_path(cr, n.slot_pos(d.out, d.slot), d.to);
            cr.stroke().ok();
        }
    }
    cr.restore().ok();

    // HUD
    rgb(cr, 0x777777);
    cr.set_font_size(11.0);
    cr.move_to(8.0, h - 8.0);
    cr.show_text(&format!("zoom {:.0}%", v.zoom * 100.0)).ok();
}
