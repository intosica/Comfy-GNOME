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
use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use transform::View;

const TITLE_H: f64 = 30.0;
const SLOT_H: f64 = 20.0;
const GRID: f64 = 20.0;

#[derive(Default)]
pub struct Node {
    /// Workflow node id (opaque to the canvas; reported back on activation).
    pub id: String,
    pub title: String,
    pub pos: (f64, f64),
    pub size: (f64, f64),
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
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

/// (from_node, from_output, to_node, to_input)
pub type Link = (usize, usize, usize, usize);

enum Drag {
    Node { idx: usize, grab: (f64, f64) },
    Pan { start_offset: (f64, f64) },
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
}

#[derive(Clone)]
pub struct Canvas {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
    on_activate: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
}

impl Canvas {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
        let c = Self { area, state: Rc::default(), on_activate: Rc::default() };
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
        self.fit_view();
    }

    pub fn view(&self) -> (f64, (f64, f64)) {
        let v = self.state.borrow().view;
        (v.zoom, v.offset)
    }

    /// Restore a saved viewport (`extra.ds` in workflow files).
    pub fn set_view(&self, zoom: f64, offset: (f64, f64)) {
        let mut s = self.state.borrow_mut();
        s.view = View { zoom: zoom.clamp(transform::ZOOM_MIN, transform::ZOOM_MAX), offset };
        drop(s);
        self.area.queue_draw();
    }

    /// Double-click on a node (by id).
    pub fn connect_activate(&self, f: impl Fn(&str) + 'static) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// Zoom/pan so every node is visible (falls back to reset when empty).
    pub fn fit_view(&self) {
        let (w, h) = (self.area.width() as f64, self.area.height() as f64);
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

        // Primary drag: move node under pointer, else pan. Middle drag: always pan.
        for button in [gdk::BUTTON_PRIMARY, gdk::BUTTON_MIDDLE] {
            let drag = gtk::GestureDrag::builder().button(button).build();
            let st = self.state.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = st.borrow_mut();
                let c = s.view.to_canvas((x, y));
                let hit = (button == gdk::BUTTON_PRIMARY)
                    .then(|| s.order.iter().rposition(|&i| s.nodes[i].contains(c)))
                    .flatten();
                s.drag = Some(match hit {
                    Some(oi) => {
                        let idx = s.order.remove(oi);
                        s.order.push(idx); // raise to top
                        let p = s.nodes[idx].pos;
                        Drag::Node { idx, grab: (c.0 - p.0, c.1 - p.1) }
                    }
                    None => Drag::Pan { start_offset: s.view.offset },
                });
            });
            let (st, area) = (self.state.clone(), self.area.clone());
            drag.connect_drag_update(move |g, dx, dy| {
                let Some((sx, sy)) = g.start_point() else { return };
                let mut s = st.borrow_mut();
                match s.drag {
                    Some(Drag::Node { idx, grab }) => {
                        let c = s.view.to_canvas((sx + dx, sy + dy));
                        s.nodes[idx].pos = (c.0 - grab.0, c.1 - grab.1);
                    }
                    Some(Drag::Pan { start_offset }) => {
                        s.view.offset = (start_offset.0 + dx, start_offset.1 + dy);
                    }
                    None => return,
                }
                drop(s);
                area.queue_draw();
            });
            let st = self.state.clone();
            drag.connect_drag_end(move |_, _, _| st.borrow_mut().drag = None);
            self.area.add_controller(drag);
        }
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

    // Links under nodes (litegraph order). Culled by their bounding box.
    rgb(cr, 0x99aa99);
    cr.set_line_width(3.0);
    for &(a, ao, b, bi) in &s.links {
        let (o, i) = (s.nodes[a].slot_pos(true, ao), s.nodes[b].slot_pos(false, bi));
        let half = (o.0 - i.0).abs() / 2.0;
        if visible(o.0.min(i.0) - half, o.1.min(i.1) - 30.0, (o.0 - i.0).abs() + 2.0 * half, (o.1 - i.1).abs() + 30.0) {
            link_path(cr, o, i);
        }
    }
    cr.stroke().ok();

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
        let dragging = matches!(s.drag, Some(Drag::Node { idx, .. }) if idx == i);
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
        if n.collapsed { continue; }

        // Slots
        cr.save().ok();
        cr.rectangle(x, y, nw, nh);
        cr.clip();
        cr.set_font_size(12.0);
        for (out, labels) in [(false, &n.inputs), (true, &n.outputs)] {
            for (si, label) in labels.iter().enumerate() {
                let (px, py) = n.slot_pos(out, si);
                rgb(cr, 0x99aa99);
                cr.arc(px, py, 5.0, 0.0, 2.0 * PI);
                cr.fill().ok();
                rgb(cr, 0xaaaaaa);
                let tw = cr.text_extents(label).map(|e| e.width()).unwrap_or(0.0);
                cr.move_to(if out { px - 12.0 - tw } else { px + 12.0 }, py + 4.0);
                cr.show_text(label).ok();
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
    cr.restore().ok();

    // HUD
    rgb(cr, 0x777777);
    cr.set_font_size(11.0);
    cr.move_to(8.0, h - 8.0);
    cr.show_text(&format!("zoom {:.0}%", v.zoom * 100.0)).ok();
}
