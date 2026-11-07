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

pub struct Node {
    pub title: String,
    pub pos: (f64, f64),
    pub size: (f64, f64),
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

impl Node {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.pos.0 && x <= self.pos.0 + self.size.0 && y >= self.pos.1 - TITLE_H && y <= self.pos.1 + self.size.1
    }
    /// `pos` is the top-left of the body; the title bar sits above it (litegraph convention).
    fn slot_pos(&self, output: bool, i: usize) -> (f64, f64) {
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
    drag: Option<Drag>,
    pointer: (f64, f64),
}

#[derive(Clone)]
pub struct Canvas {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
}

impl Canvas {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
        let c = Self { area, state: Rc::default() };
        c.setup_draw();
        c.setup_input();
        c
    }

    pub fn add_node(&self, node: Node) -> usize {
        let mut s = self.state.borrow_mut();
        s.nodes.push(node);
        let i = s.nodes.len() - 1;
        s.order.push(i);
        drop(s);
        self.area.queue_draw();
        i
    }

    pub fn add_link(&self, l: Link) {
        self.state.borrow_mut().links.push(l);
        self.area.queue_draw();
    }

    pub fn reset_view(&self) {
        self.state.borrow_mut().view = View { zoom: 1.0, offset: (40.0, 60.0) };
        self.area.queue_draw();
    }

    /// Phase 1 validation content: two nodes and one wire.
    pub fn add_demo_nodes(&self) {
        let a = self.add_node(Node {
            title: "Load Checkpoint".into(),
            pos: (0.0, 0.0),
            size: (240.0, 80.0),
            inputs: vec![],
            outputs: vec!["MODEL".into(), "CLIP".into(), "VAE".into()],
        });
        let b = self.add_node(Node {
            title: "KSampler".into(),
            pos: (360.0, 40.0),
            size: (220.0, 110.0),
            inputs: vec!["model".into(), "positive".into(), "negative".into(), "latent_image".into()],
            outputs: vec!["LATENT".into()],
        });
        self.add_link((a, 0, b, 0));
        self.reset_view();
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

    // Links under nodes (litegraph order).
    rgb(cr, 0x99aa99);
    cr.set_line_width(3.0);
    for &(a, ao, b, bi) in &s.links {
        link_path(cr, s.nodes[a].slot_pos(true, ao), s.nodes[b].slot_pos(false, bi));
    }
    cr.stroke().ok();

    cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(13.0);
    for &i in &s.order {
        let n = &s.nodes[i];
        let (x, y) = n.pos;
        let (nw, nh) = n.size;
        rounded_rect(cr, x, y - TITLE_H, nw, nh + TITLE_H, 8.0);
        rgb(cr, 0x353535);
        cr.fill_preserve().ok();
        let dragging = matches!(s.drag, Some(Drag::Node { idx, .. }) if idx == i);
        rgb(cr, if dragging { 0xffffff } else { 0x111111 });
        cr.set_line_width(if dragging { 2.0 } else { 1.0 });
        cr.stroke().ok();

        // Title bar
        cr.save().ok();
        rounded_rect(cr, x, y - TITLE_H, nw, nh + TITLE_H, 8.0);
        cr.clip();
        rgb(cr, 0x333333);
        cr.rectangle(x, y - TITLE_H, nw, TITLE_H);
        cr.fill().ok();
        cr.restore().ok();
        rgb(cr, 0x999999);
        cr.move_to(x + 28.0, y - TITLE_H / 2.0 + 5.0);
        cr.show_text(&n.title).ok();
        cr.arc(x + 14.0, y - TITLE_H / 2.0, 5.0, 0.0, 2.0 * PI);
        cr.fill().ok();

        // Slots
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
        cr.set_font_size(13.0);
    }
    cr.restore().ok();

    // HUD
    rgb(cr, 0x777777);
    cr.set_font_size(11.0);
    cr.move_to(8.0, h - 8.0);
    cr.show_text(&format!("zoom {:.0}%", v.zoom * 100.0)).ok();
}
