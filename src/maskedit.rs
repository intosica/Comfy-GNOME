// SPDX-License-Identifier: GPL-3.0-only
//! Mask painter (and region-box editor) in its own window.
//!
//! The UI only paints the mask; the actual inpainting is done by the workflow's own nodes.
//! One editor serves both users: Load Image's "Open in Mask Editor" (full-resolution mask that
//! `/upload/mask` copies into the picture's alpha) and Image BBox Layout (a capped-size mask PNG
//! plus normalised boxes that snap to the node's 16 px output grid).
//!
//! The mask is a cairo A8 surface: strokes are cairo round-capped lines (Clear for the eraser),
//! drawn on screen as a tinted `mask_surface`. Nothing here needs a worker thread — a stroke
//! segment touches a few thousand pixels.

use adw::{gdk, glib, gtk, prelude::*};
use gtk::cairo::{self, Context, Format, ImageSurface, Operator};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// Region box, normalised to the image (0..1), as Image BBox Layout stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct BBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub caption: String,
    /// `None` = automatic.
    pub strength: Option<f64>,
}

pub fn parse_boxes(s: &str) -> Vec<BBox> {
    let v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let f = |k: &str| b[k].as_f64().or_else(|| b[k].as_str().and_then(|s| s.parse().ok()));
            Some(BBox {
                x: f("x")?,
                y: f("y")?,
                w: f("w")?,
                h: f("h")?,
                caption: b["caption"].as_str().unwrap_or_default().to_owned(),
                strength: f("strength"),
            })
        })
        .collect()
}

pub fn boxes_json(b: &[BBox]) -> String {
    let r8 = |v: f64| (v * 1e8).round() / 1e8;
    let a: Vec<Value> = b.iter().map(|b| json!({"x": r8(b.x), "y": r8(b.y), "w": r8(b.w), "h": r8(b.h), "caption": b.caption, "strength": b.strength})).collect();
    Value::Array(a).to_string()
}

/// Image BBox Layout output size (backend `resize_pair(always_snap)`): sides snap to 16.
pub fn output_size(w: f64, h: f64, max_mp: f64) -> (f64, f64) {
    let snap16 = |n: f64| (((n.floor() + 7.0) / 16.0).floor() * 16.0).max(16.0);
    let target = if max_mp > 0.0 { max_mp } else { 1.0 } * 1e6;
    if w * h > target {
        let k = (target / (w * h)).sqrt();
        return (snap16((w * k).floor()), snap16((h * k).floor()));
    }
    (snap16(w), snap16(h))
}

const GRID: f64 = 16.0;
const HANDLE: f64 = 6.0;
const UNDO_MAX: usize = 20;
const MASK_RGB: (f64, f64, f64) = (214.0 / 255.0, 64.0 / 255.0, 230.0 / 255.0);
use crate::canvas::BOX_COLORS as COLORS;

fn snap_n(v: f64, out: f64) -> f64 {
    ((v * out / GRID).round() * GRID / out).clamp(0.0, 1.0)
}

/// Where the mask starts from.
pub enum MaskSrc {
    None,
    /// Load Image: the picture's own alpha (`mask = 1 - alpha`).
    ImageAlpha,
    /// Image BBox Layout: a PNG whose alpha is the mask.
    Png(Vec<u8>),
}

pub struct Setup {
    pub title: String,
    pub image: Vec<u8>,
    pub mask: MaskSrc,
    /// Longest mask side; `None` = the image's own size (required by `/upload/mask`).
    pub mask_max: Option<i32>,
    /// Some = box tool on: initial boxes + the node's `max_megapixels`.
    pub boxes: Option<(Vec<BBox>, f64)>,
}

/// Painted mask, tightly packed (`w * h` bytes, 255 = paint here).
pub struct Mask {
    pub w: i32,
    pub h: i32,
    pub a: Vec<u8>,
}

impl Mask {
    pub fn is_empty(&self) -> bool {
        self.a.iter().all(|&v| v <= 3)
    }
    /// PNG whose alpha is the mask (`invert`: alpha = 255 - mask, what `/upload/mask` wants).
    pub fn png(&self, invert: bool) -> Vec<u8> {
        let mut px = Vec::with_capacity(self.a.len() * 4);
        for &a in &self.a {
            let (r, g, b) = MASK_RGB;
            px.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, if invert { 255 - a } else { a }]);
        }
        let tex = gdk::MemoryTexture::new(self.w, self.h, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from_owned(px), self.w as usize * 4);
        tex.save_to_png_bytes().to_vec()
    }
}

pub struct Outcome {
    /// `None` = mask untouched.
    pub mask: Option<Mask>,
    pub boxes: Option<Vec<BBox>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Box,
    Brush,
    Erase,
    Pan,
}

enum Drag {
    None,
    Pan { off: (f64, f64) },
    Paint { last: (f64, f64), erase: bool },
    NewBox { ax: f64, ay: f64, i: usize },
    Move { i: usize, o: (f64, f64), p: (f64, f64) },
    Resize { i: usize, h: &'static str },
}

struct Snap {
    mask: Vec<u8>,
    boxes: Vec<BBox>,
}

struct St {
    img: ImageSurface,
    nat: (f64, f64),
    mask: ImageSurface,
    /// Mask px per image px.
    k: f64,
    boxes: Option<Vec<BBox>>,
    sel: Option<usize>,
    out: (f64, f64),
    tool: Tool,
    size: f64,
    scale: f64,
    off: (f64, f64),
    ptr: Option<(f64, f64)>,
    drag: Drag,
    space: bool,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    mask_dirty: bool,
    fitted: bool,
}

impl St {
    fn to_img(&self, p: (f64, f64)) -> (f64, f64) {
        ((p.0 - self.off.0) / self.scale, (p.1 - self.off.1) / self.scale)
    }
    fn to_scr(&self, p: (f64, f64)) -> (f64, f64) {
        (p.0 * self.scale + self.off.0, p.1 * self.scale + self.off.1)
    }
    fn fit(&mut self, w: f64, h: f64) {
        self.scale = (0.94 * (w / self.nat.0).min(h / self.nat.1)).max(1e-3);
        self.off = ((w - self.nat.0 * self.scale) / 2.0, (h - self.nat.1 * self.scale) / 2.0);
    }
    fn mask_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.mask.with_data(|d| out = d.to_vec()).ok();
        out
    }
    fn snapshot(&self) -> Snap {
        Snap { mask: self.mask_bytes(), boxes: self.boxes.clone().unwrap_or_default() }
    }
    fn push_undo(&mut self) {
        let s = self.snapshot();
        self.undo.push(s);
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn restore(&mut self, s: Snap) {
        let (w, h, stride) = (self.mask.width(), self.mask.height(), self.mask.stride());
        if s.mask.len() == (stride * h) as usize {
            if let Ok(m) = ImageSurface::create_for_data(s.mask, Format::A8, w, h, stride) {
                self.mask = m;
            }
        }
        if self.boxes.is_some() {
            self.boxes = Some(s.boxes);
            self.sel = self.sel.filter(|&i| i < self.boxes.as_ref().map_or(0, Vec::len));
        }
        self.mask_dirty = true;
    }
    fn undo(&mut self, redo: bool) {
        let from = if redo { self.redo.pop() } else { self.undo.pop() };
        if let Some(s) = from {
            let cur = self.snapshot();
            if redo { self.undo.push(cur) } else { self.redo.push(cur) }
            self.restore(s);
        }
    }
    fn paint(&mut self, a: (f64, f64), b: (f64, f64), erase: bool) {
        let Ok(cr) = Context::new(&self.mask) else { return };
        cr.scale(self.k, self.k);
        cr.set_operator(if erase { Operator::Clear } else { Operator::Over });
        cr.set_source_rgba(0.0, 0.0, 0.0, 1.0);
        cr.set_line_width(self.size);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.move_to(a.0, a.1);
        cr.line_to(b.0, b.1);
        cr.stroke().ok();
        self.mask_dirty = true;
    }
    fn fill_mask(&mut self, op: Operator) {
        self.push_undo();
        if let Ok(cr) = Context::new(&self.mask) {
            cr.set_operator(op);
            cr.set_source_rgba(0.0, 0.0, 0.0, 1.0);
            cr.paint().ok();
        }
        self.mask_dirty = true;
    }
    /// Screen-space rect of box `b`.
    fn box_scr(&self, b: &BBox) -> (f64, f64, f64, f64) {
        let (x0, y0) = self.to_scr((b.x * self.nat.0, b.y * self.nat.1));
        let (x1, y1) = self.to_scr(((b.x + b.w) * self.nat.0, (b.y + b.h) * self.nat.1));
        (x0, y0, x1, y1)
    }
    fn handles(&self, b: &BBox) -> [(&'static str, f64, f64); 8] {
        let (x0, y0, x1, y1) = self.box_scr(b);
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        [("nw", x0, y0), ("n", mx, y0), ("ne", x1, y0), ("e", x1, my), ("se", x1, y1), ("s", mx, y1), ("sw", x0, y1), ("w", x0, my)]
    }
    fn hit_handle(&self, p: (f64, f64)) -> Option<&'static str> {
        let b = self.boxes.as_ref()?.get(self.sel?)?;
        self.handles(b).into_iter().find(|(_, hx, hy)| (p.0 - hx).abs() <= HANDLE + 3.0 && (p.1 - hy).abs() <= HANDLE + 3.0).map(|h| h.0)
    }
    /// Topmost box under screen point `p` (the selected one wins ties).
    fn hit_box(&self, p: (f64, f64)) -> Option<usize> {
        let bs = self.boxes.as_ref()?;
        let inside = |i: usize| {
            let (x0, y0, x1, y1) = self.box_scr(&bs[i]);
            p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1
        };
        if let Some(s) = self.sel.filter(|&s| s < bs.len() && inside(s)) {
            return Some(s);
        }
        (0..bs.len()).rev().find(|&i| inside(i))
    }
    fn info(&self) -> String {
        let mut parts = Vec::new();
        if let Some(bs) = &self.boxes {
            parts.push(format!("Output {} × {} px ({:.2} MP)", self.out.0, self.out.1, self.out.0 * self.out.1 / 1e6));
            parts.push(format!("{} box{}", bs.len(), if bs.len() == 1 { "" } else { "es" }));
        } else {
            parts.push(format!("{} × {} px", self.nat.0, self.nat.1));
        }
        parts.push(format!("brush {} px", self.size));
        parts.push(format!("{:.0} %", self.scale * 100.0));
        parts.join(" · ")
    }
}

/// Decode `bytes` into an opaque display surface plus the straight alpha channel.
fn decode(bytes: &[u8]) -> Option<(ImageSurface, Vec<u8>)> {
    let tex = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)).ok()?;
    let (w, h) = (tex.width(), tex.height());
    let mut dl = gdk::TextureDownloader::new(&tex);
    dl.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (px, stride) = dl.download_bytes();
    let mut surf = ImageSurface::create(Format::Rgb24, w, h).ok()?;
    let mut alpha = vec![0u8; (w * h) as usize];
    {
        let ss = surf.stride() as usize;
        let mut d = surf.data().ok()?;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let s = &px[y * stride + x * 4..y * stride + x * 4 + 4];
                // Rgb24 is native-endian 0x00RRGGBB.
                let v = u32::from_be_bytes([255, s[0], s[1], s[2]]).to_ne_bytes();
                d[y * ss + x * 4..y * ss + x * 4 + 4].copy_from_slice(&v);
                alpha[y * w as usize + x] = s[3];
            }
        }
    }
    Some((surf, alpha))
}

fn a8_from(w: i32, h: i32, f: impl Fn(usize, usize) -> u8) -> Option<ImageSurface> {
    let mut m = ImageSurface::create(Format::A8, w, h).ok()?;
    let stride = m.stride() as usize;
    {
        let mut d = m.data().ok()?;
        for y in 0..h as usize {
            for x in 0..w as usize {
                d[y * stride + x] = f(x, y);
            }
        }
    }
    Some(m)
}

fn rgb(cr: &Context, c: u32, a: f64) {
    cr.set_source_rgba(((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0, a);
}

/// Open the editor. `done` runs once when it closes with Done (or the window is closed);
/// Cancel drops everything.
pub fn open(parent: Option<&gtk::Window>, setup: Setup, done: impl Fn(Outcome) + 'static) -> Result<(), String> {
    let (img, alpha) = decode(&setup.image).ok_or("cannot decode the picture")?;
    let (nw, nh) = (img.width(), img.height());
    let k = setup.mask_max.map_or(1.0, |m| (m as f64 / nw.max(nh) as f64).min(1.0));
    let (mw, mh) = (((nw as f64 * k).round() as i32).max(1), ((nh as f64 * k).round() as i32).max(1));
    let mask = match setup.mask {
        MaskSrc::ImageAlpha if mw == nw && mh == nh => a8_from(mw, mh, |x, y| 255 - alpha[y * nw as usize + x]),
        MaskSrc::Png(bytes) => {
            let m = a8_from(mw, mh, |_, _| 0);
            // Scale the stored mask onto ours; only its alpha survives on an A8 target.
            if let (Some(m), Ok(tex)) = (&m, gdk::Texture::from_bytes(&glib::Bytes::from(&bytes))) {
                let (tw, th) = (tex.width(), tex.height());
                let mut data = vec![0u8; (tw * th * 4) as usize];
                tex.download(&mut data, tw as usize * 4);
                if let (Ok(src), Ok(cr)) = (ImageSurface::create_for_data(data, Format::ARgb32, tw, th, tw * 4), Context::new(m)) {
                    cr.scale(mw as f64 / tw as f64, mh as f64 / th as f64);
                    if cr.set_source_surface(&src, 0.0, 0.0).is_ok() {
                        cr.paint().ok();
                    }
                }
            }
            m
        }
        _ => a8_from(mw, mh, |_, _| 0),
    }
    .ok_or("cannot allocate the mask")?;
    let box_mode = setup.boxes.is_some();
    let (boxes, max_mp) = setup.boxes.map_or((None, 1.0), |(b, mp)| (Some(b), mp));
    let st = Rc::new(RefCell::new(St {
        img,
        nat: (nw as f64, nh as f64),
        mask,
        k,
        boxes,
        sel: None,
        out: output_size(nw as f64, nh as f64, max_mp),
        tool: if box_mode { Tool::Box } else { Tool::Brush },
        size: ((nw.max(nh) as f64) / 40.0).round().clamp(4.0, 120.0),
        scale: 1.0,
        off: (0.0, 0.0),
        ptr: None,
        drag: Drag::None,
        space: false,
        undo: Vec::new(),
        redo: Vec::new(),
        mask_dirty: false,
        fitted: false,
    }));

    let win = adw::Window::builder().title(&setup.title).default_width(1280).default_height(860).modal(true).build();
    if let Some(p) = parent {
        win.set_transient_for(Some(p));
        win.set_default_size((p.width() as f64 * 0.92) as i32, (p.height() as f64 * 0.92) as i32);
    }
    let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
    let info = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["dim-label", "caption", "numeric"]).build();

    // ---- header: tools, brush size, undo, mask ops, done ---------------------------------
    let head = adw::HeaderBar::new();
    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tools.add_css_class("linked");
    let mut tool_btns: Vec<(gtk::ToggleButton, Tool)> = Vec::new();
    // Text labels: the Adwaita icon set has no box / brush / hand glyphs.
    let mut add_tool = |label: &str, tip: &str, t: Tool| {
        let b = gtk::ToggleButton::builder().label(label).tooltip_text(tip).build();
        if let Some((first, _)) = tool_btns.first() {
            b.set_group(Some(first));
        }
        tools.append(&b);
        tool_btns.push((b, t));
    };
    if box_mode {
        add_tool("Box", "Draw region boxes (C)", Tool::Box);
    }
    add_tool("Brush", "Paint the mask (B)", Tool::Brush);
    add_tool("Eraser", "Erase the mask (E)", Tool::Erase);
    add_tool("Pan", "Move the view (V, Space, middle or right drag)", Tool::Pan);
    let tool_btns = Rc::new(tool_btns);
    let size = gtk::SpinButton::with_range(2.0, 400.0, 1.0);
    size.set_tooltip_text(Some("Brush size in image pixels ( [ and ] )"));
    size.set_value(st.borrow().size);
    let undo = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Undo (Ctrl+Z)").build();
    let redo = gtk::Button::builder().icon_name("edit-redo-symbolic").tooltip_text("Redo (Ctrl+Shift+Z)").build();
    let ops = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("Mask").build();
    let menu = gtk::gio::Menu::new();
    menu.append(Some("Invert mask"), Some("mask.invert"));
    menu.append(Some("Clear mask"), Some("mask.clear"));
    menu.append(Some("Fit view (0)"), Some("mask.fit"));
    ops.set_menu_model(Some(&menu));
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::builder().label("Done").css_classes(["suggested-action"]).build();
    head.pack_start(&tools);
    head.pack_start(&size);
    head.pack_start(&undo);
    head.pack_start(&redo);
    head.pack_end(&ok);
    head.pack_end(&ops);
    head.pack_start(&cancel);
    head.set_show_end_title_buttons(false);
    head.set_show_start_title_buttons(false);

    // ---- side panel (boxes) -------------------------------------------------------------
    let side = gtk::Box::new(gtk::Orientation::Vertical, 8);
    side.set_width_request(300);
    side.set_hexpand(false); // the flow box would otherwise steal the picture's width
    side.set_margin_start(12);
    side.set_margin_end(12);
    side.set_margin_top(12);
    side.set_margin_bottom(12);
    let chips = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(8).build();
    let caption = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).left_margin(6).right_margin(6).top_margin(6).bottom_margin(6).build();
    let cap_sc = gtk::ScrolledWindow::builder().child(&caption).min_content_height(120).hscrollbar_policy(gtk::PolicyType::Never).css_classes(["card"]).build();
    let strength = gtk::Entry::builder().placeholder_text("auto").tooltip_text("Denoising strength for just this box (0–1). Blank = auto.").build();
    let del = gtk::Button::builder().label("Delete box").css_classes(["destructive-action"]).build();
    let clear_boxes = gtk::Button::with_label("Clear boxes");
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 6);
    editor.append(&gtk::Label::builder().label("Caption").xalign(0.0).css_classes(["heading"]).build());
    editor.append(&cap_sc);
    let srow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    srow.append(&gtk::Label::builder().label("Strength").xalign(0.0).hexpand(true).build());
    srow.append(&strength);
    editor.append(&srow);
    editor.append(&del);
    let hint = gtk::Label::builder()
        .label("Drag on the picture to add a box; drag a box to move it, its handles to resize. Boxes snap to the 16 px output grid.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    side.append(&gtk::Label::builder().label("Boxes").xalign(0.0).css_classes(["title-4"]).build());
    side.append(&chips);
    side.append(&editor);
    side.append(&hint);
    let spacer = gtk::Box::builder().vexpand(true).build();
    side.append(&spacer);
    side.append(&clear_boxes);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&area);
    if box_mode {
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&side);
    }
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.set_margin_start(12);
    foot.set_margin_end(12);
    foot.set_margin_top(4);
    foot.set_margin_bottom(4);
    foot.append(&info);
    foot.append(&gtk::Label::builder().label(if box_mode { "C box · B brush · E eraser · Space pan · wheel zoom · [ ] size · Ctrl+Z undo" } else { "B brush · E eraser · Space pan · wheel zoom · [ ] size · Ctrl+Z undo" }).css_classes(["dim-label", "caption"]).build());
    let tv = adw::ToolbarView::builder().content(&body).build();
    tv.add_top_bar(&head);
    tv.add_bottom_bar(&foot);
    win.set_content(Some(&tv));

    // ---- side panel sync --------------------------------------------------------------
    // Programmatic edits of the caption/strength fields must not write back.
    let syncing = Rc::new(std::cell::Cell::new(false));
    type Hook = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
    let render_side: Hook = Rc::default();
    {
        let (st, chips, editor, caption, strength, syncing, area, rs) =
            (st.clone(), chips.clone(), editor.clone(), caption.clone(), strength.clone(), syncing.clone(), area.clone(), render_side.clone());
        let me = rs.clone();
        *rs.borrow_mut() = Some(Rc::new(move || {
            let s = st.borrow();
            let Some(bs) = &s.boxes else { return };
            chips.remove_all();
            for (i, b) in bs.iter().enumerate() {
                let label = if b.caption.is_empty() { format!("{}", i + 1) } else { format!("{} · {}", i + 1, b.caption.chars().take(14).collect::<String>()) };
                let btn = gtk::ToggleButton::builder().label(&label).active(s.sel == Some(i)).build();
                let c = COLORS[i % COLORS.len()];
                btn.add_css_class(&format!("bbox-chip-{}", i % COLORS.len()));
                btn.set_tooltip_text(Some(&format!("Box {} (#{c:06x})", i + 1)));
                let (st2, me2, area2) = (st.clone(), me.clone(), area.clone());
                btn.connect_clicked(move |_| {
                    st2.borrow_mut().sel = Some(i);
                    area2.queue_draw();
                    let f = me2.borrow().clone();
                    if let Some(f) = f {
                        glib::idle_add_local_once(move || f());
                    }
                });
                chips.insert(&btn, -1);
            }
            let cur = s.sel.and_then(|i| bs.get(i));
            editor.set_sensitive(cur.is_some());
            syncing.set(true);
            caption.buffer().set_text(cur.map_or("", |b| b.caption.as_str()));
            strength.set_text(&cur.and_then(|b| b.strength).map(|v| v.to_string()).unwrap_or_default());
            syncing.set(false);
        }));
    }
    let side_now = {
        let rs = render_side.clone();
        move || {
            let f = rs.borrow().clone();
            if let Some(f) = f {
                f();
            }
        }
    };
    side_now();
    {
        let (st, syncing, area) = (st.clone(), syncing.clone(), area.clone());
        caption.buffer().connect_changed(move |b| {
            if syncing.get() {
                return;
            }
            let mut s = st.borrow_mut();
            let Some(i) = s.sel else { return };
            let t = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            if let Some(bx) = s.boxes.as_mut().and_then(|v| v.get_mut(i)) {
                bx.caption = t;
            }
            drop(s);
            area.queue_draw(); // chip labels catch up on the next structural change
        });
    }
    {
        let (st, syncing) = (st.clone(), syncing.clone());
        strength.connect_changed(move |e| {
            if syncing.get() {
                return;
            }
            let mut s = st.borrow_mut();
            let Some(i) = s.sel else { return };
            let t = e.text();
            let v = t.trim();
            let val = if v.is_empty() { None } else { v.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)) };
            if let Some(bx) = s.boxes.as_mut().and_then(|v| v.get_mut(i)) {
                bx.strength = val;
            }
        });
    }
    let delete_sel: Rc<dyn Fn()> = {
        let (st, area, side_now) = (st.clone(), area.clone(), side_now.clone());
        Rc::new(move || {
            let mut s = st.borrow_mut();
            let Some(i) = s.sel else { return };
            s.push_undo();
            if let Some(v) = s.boxes.as_mut() {
                if i < v.len() {
                    v.remove(i);
                }
            }
            s.sel = None;
            drop(s);
            area.queue_draw();
            side_now();
        })
    };
    {
        let d = delete_sel.clone();
        del.connect_clicked(move |_| d());
        let (st, area, side_now) = (st.clone(), area.clone(), side_now.clone());
        clear_boxes.connect_clicked(move |_| {
            let mut s = st.borrow_mut();
            s.push_undo();
            if let Some(v) = s.boxes.as_mut() {
                v.clear();
            }
            s.sel = None;
            drop(s);
            area.queue_draw();
            side_now();
        });
    }

    // ---- draw -----------------------------------------------------------------------------
    {
        let (st, info) = (st.clone(), info.clone());
        area.set_draw_func(move |_, cr, w, h| {
            let mut s = st.borrow_mut();
            if !s.fitted && w > 10 && h > 10 {
                s.fit(w as f64, h as f64);
                s.fitted = true;
            }
            cr.set_source_rgb(0.12, 0.12, 0.13);
            cr.paint().ok();
            cr.save().ok();
            cr.translate(s.off.0, s.off.1);
            cr.scale(s.scale, s.scale);
            if cr.set_source_surface(&s.img, 0.0, 0.0).is_ok() {
                cr.source().set_filter(if s.scale > 2.0 { cairo::Filter::Nearest } else { cairo::Filter::Good });
                cr.paint().ok();
            }
            cr.scale(1.0 / s.k, 1.0 / s.k);
            cr.set_source_rgba(MASK_RGB.0, MASK_RGB.1, MASK_RGB.2, 0.55);
            cr.mask_surface(&s.mask, 0.0, 0.0).ok();
            cr.restore().ok();
            // Boxes in screen space so strokes and labels keep their size at any zoom.
            if let Some(bs) = &s.boxes {
                cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                cr.set_font_size(13.0);
                for (i, b) in bs.iter().enumerate() {
                    let c = COLORS[i % COLORS.len()];
                    let (x0, y0, x1, y1) = s.box_scr(b);
                    rgb(cr, c, 0.12);
                    cr.rectangle(x0, y0, x1 - x0, y1 - y0);
                    cr.fill_preserve().ok();
                    rgb(cr, c, 1.0);
                    cr.set_line_width(if s.sel == Some(i) { 3.0 } else { 2.0 });
                    cr.stroke().ok();
                    let label = if b.caption.is_empty() { format!("{}", i + 1) } else { format!("{} {}", i + 1, b.caption) };
                    let label: String = label.chars().take(48).collect();
                    let tw = cr.text_extents(&label).map_or(20.0, |e| e.x_advance());
                    rgb(cr, c, 0.9);
                    cr.rectangle(x0, y0 - 18.0, tw + 10.0, 18.0);
                    cr.fill().ok();
                    cr.set_source_rgb(0.0, 0.0, 0.0);
                    cr.move_to(x0 + 5.0, y0 - 5.0);
                    cr.show_text(&label).ok();
                    if s.sel == Some(i) {
                        for (_, hx, hy) in s.handles(b) {
                            rgb(cr, c, 1.0);
                            cr.rectangle(hx - HANDLE, hy - HANDLE, HANDLE * 2.0, HANDLE * 2.0);
                            cr.fill_preserve().ok();
                            cr.set_source_rgb(0.0, 0.0, 0.0);
                            cr.set_line_width(1.0);
                            cr.stroke().ok();
                        }
                    }
                }
            }
            // Brush ring.
            if let (Some(p), Tool::Brush | Tool::Erase, false) = (s.ptr, s.tool, s.space) {
                let r = s.size * s.scale / 2.0;
                cr.set_line_width(1.5);
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.8);
                cr.arc(p.0, p.1, r + 1.0, 0.0, std::f64::consts::TAU);
                cr.stroke().ok();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
                cr.arc(p.0, p.1, r, 0.0, std::f64::consts::TAU);
                cr.stroke().ok();
            }
            // Labels changed from inside a draw only show on the next frame: defer it.
            let (info, text) = (info.clone(), s.info());
            if info.label() != text {
                glib::idle_add_local_once(move || info.set_label(&text));
            }
        });
    }
    let set_cursor = {
        let (st, area) = (st.clone(), area.clone());
        Rc::new(move || {
            let s = st.borrow();
            let name = match (s.tool, s.space || matches!(s.drag, Drag::Pan { .. })) {
                (_, true) | (Tool::Pan, _) => "grab",
                (Tool::Box, _) => "crosshair",
                _ => "none",
            };
            area.set_cursor_from_name(Some(name));
        })
    };
    set_cursor();
    let set_tool: Rc<dyn Fn(Tool)> = {
        let (st, area, btns, set_cursor) = (st.clone(), area.clone(), tool_btns.clone(), set_cursor.clone());
        Rc::new(move |t| {
            if !st.borrow().boxes.is_some() && t == Tool::Box {
                return;
            }
            st.borrow_mut().tool = t;
            for (b, bt) in btns.iter() {
                if *bt == t && !b.is_active() {
                    b.set_active(true);
                }
            }
            set_cursor();
            area.queue_draw();
        })
    };
    for (b, t) in tool_btns.iter() {
        let (set_tool, t) = (set_tool.clone(), *t);
        b.connect_toggled(move |b| {
            if b.is_active() {
                set_tool(t);
            }
        });
    }
    let first = st.borrow().tool;
    set_tool(first);
    {
        let (st, area) = (st.clone(), area.clone());
        size.connect_value_changed(move |sp| {
            st.borrow_mut().size = sp.value();
            area.queue_draw();
        });
    }
    let do_undo: Rc<dyn Fn(bool)> = {
        let (st, area, side_now) = (st.clone(), area.clone(), side_now.clone());
        Rc::new(move |r| {
            st.borrow_mut().undo(r);
            area.queue_draw();
            side_now();
        })
    };
    {
        let u = do_undo.clone();
        undo.connect_clicked(move |_| u(false));
        let u = do_undo.clone();
        redo.connect_clicked(move |_| u(true));
    }
    let actions = gtk::gio::SimpleActionGroup::new();
    for (name, op) in [("invert", Some(Operator::Xor)), ("clear", Some(Operator::Clear)), ("fit", None)] {
        let a = gtk::gio::SimpleAction::new(name, None);
        let (st, area) = (st.clone(), area.clone());
        a.connect_activate(move |_, _| {
            let mut s = st.borrow_mut();
            match op {
                Some(op) => s.fill_mask(op),
                None => s.fit(area.width() as f64, area.height() as f64),
            }
            drop(s);
            area.queue_draw();
        });
        actions.add_action(&a);
    }
    win.insert_action_group("mask", Some(&actions));

    // ---- pointer ----------------------------------------------------------------------
    let motion = gtk::EventControllerMotion::new();
    {
        let (st, area) = (st.clone(), area.clone());
        motion.connect_motion(move |_, x, y| {
            st.borrow_mut().ptr = Some((x, y));
            area.queue_draw();
        });
    }
    {
        let (st, area) = (st.clone(), area.clone());
        motion.connect_leave(move |_| {
            st.borrow_mut().ptr = None;
            area.queue_draw();
        });
    }
    area.add_controller(motion);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let (st, area) = (st.clone(), area.clone());
        scroll.connect_scroll(move |c, _, dy| {
            let mut s = st.borrow_mut();
            let p = s.ptr.unwrap_or((area.width() as f64 / 2.0, area.height() as f64 / 2.0));
            let before = s.to_img(p);
            // Discrete wheels report ±1, touchpads small deltas: same curve for both.
            let dy = if c.unit() == gdk::ScrollUnit::Wheel { dy * 120.0 } else { dy * 4.0 };
            s.scale = (s.scale * (-dy * 0.0015f64).exp()).clamp(0.05, 40.0);
            s.off = (p.0 - before.0 * s.scale, p.1 - before.1 * s.scale);
            drop(s);
            area.queue_draw();
            glib::Propagation::Stop
        });
    }
    area.add_controller(scroll);
    let drag = gtk::GestureDrag::new();
    drag.set_button(0);
    {
        let (st, area, set_cursor, side_now) = (st.clone(), area.clone(), set_cursor.clone(), side_now.clone());
        drag.connect_drag_begin(move |g, x, y| {
            area.grab_focus();
            let btn = g.current_button();
            let mut s = st.borrow_mut();
            s.ptr = Some((x, y));
            let p = s.to_img((x, y));
            if btn == 2 || btn == 3 || s.space || s.tool == Tool::Pan {
                s.drag = Drag::Pan { off: s.off };
            } else if btn == 1 {
                match s.tool {
                    Tool::Brush | Tool::Erase => {
                        s.push_undo();
                        let erase = s.tool == Tool::Erase;
                        s.paint(p, p, erase);
                        s.drag = Drag::Paint { last: p, erase };
                    }
                    Tool::Box => {
                        if let Some(h) = s.hit_handle((x, y)) {
                            s.push_undo();
                            s.drag = Drag::Resize { i: s.sel.unwrap_or(0), h };
                        } else if let Some(i) = s.hit_box((x, y)) {
                            s.push_undo();
                            let b = &s.boxes.as_ref().unwrap()[i];
                            s.drag = Drag::Move { i, o: (b.x, b.y), p: (p.0 / s.nat.0, p.1 / s.nat.1) };
                            s.sel = Some(i);
                        } else {
                            s.push_undo();
                            let (nx, ny) = ((p.0 / s.nat.0).clamp(0.0, 1.0), (p.1 / s.nat.1).clamp(0.0, 1.0));
                            let (ax, ay) = (snap_n(nx, s.out.0), snap_n(ny, s.out.1));
                            let v = s.boxes.as_mut().unwrap();
                            v.push(BBox { x: ax, y: ay, w: 0.0, h: 0.0, caption: String::new(), strength: None });
                            let i = v.len() - 1;
                            s.sel = Some(i);
                            s.drag = Drag::NewBox { ax, ay, i };
                        }
                    }
                    Tool::Pan => {}
                }
            }
            drop(s);
            set_cursor();
            area.queue_draw();
            side_now();
        });
    }
    {
        let (st, area) = (st.clone(), area.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            let Some((sx, sy)) = g.start_point() else { return };
            let (x, y) = (sx + dx, sy + dy);
            let mut s = st.borrow_mut();
            s.ptr = Some((x, y));
            let p = s.to_img((x, y));
            let (nx, ny) = ((p.0 / s.nat.0).clamp(0.0, 1.0), (p.1 / s.nat.1).clamp(0.0, 1.0));
            let (ow, oh) = s.out;
            let (min_w, min_h) = (GRID / ow, GRID / oh);
            match s.drag {
                Drag::Pan { off } => s.off = (off.0 + dx, off.1 + dy),
                Drag::Paint { last, erase } => {
                    s.paint(last, p, erase);
                    s.drag = Drag::Paint { last: p, erase };
                }
                Drag::NewBox { ax, ay, i } => {
                    let (x0, x1) = (snap_n(ax.min(nx), ow), snap_n(ax.max(nx), ow));
                    let (y0, y1) = (snap_n(ay.min(ny), oh), snap_n(ay.max(ny), oh));
                    if let Some(b) = s.boxes.as_mut().and_then(|v| v.get_mut(i)) {
                        (b.x, b.y, b.w, b.h) = (x0, y0, x1 - x0, y1 - y0);
                    }
                }
                Drag::Move { i, o, p: p0 } => {
                    if let Some(b) = s.boxes.as_mut().and_then(|v| v.get_mut(i)) {
                        b.x = snap_n(o.0 + nx - p0.0, ow).clamp(0.0, 1.0 - b.w);
                        b.y = snap_n(o.1 + ny - p0.1, oh).clamp(0.0, 1.0 - b.h);
                    }
                }
                Drag::Resize { i, h } => {
                    if let Some(b) = s.boxes.as_mut().and_then(|v| v.get_mut(i)) {
                        let (mut x0, mut y0, mut x1, mut y1) = (b.x, b.y, b.x + b.w, b.y + b.h);
                        if h.contains('w') {
                            x0 = snap_n(nx, ow).min(x1 - min_w);
                        }
                        if h.contains('e') {
                            x1 = snap_n(nx, ow).max(x0 + min_w);
                        }
                        if h.contains('n') {
                            y0 = snap_n(ny, oh).min(y1 - min_h);
                        }
                        if h.contains('s') {
                            y1 = snap_n(ny, oh).max(y0 + min_h);
                        }
                        (b.x, b.y, b.w, b.h) = (x0.max(0.0), y0.max(0.0), x1.min(1.0) - x0.max(0.0), y1.min(1.0) - y0.max(0.0));
                    }
                }
                Drag::None => {}
            }
            drop(s);
            area.queue_draw();
        });
    }
    {
        let (st, area, set_cursor, side_now) = (st.clone(), area.clone(), set_cursor.clone(), side_now.clone());
        drag.connect_drag_end(move |_, _, _| {
            let mut s = st.borrow_mut();
            if let Drag::NewBox { i, .. } = s.drag {
                // A click (no area) drops the new box instead of keeping a zero-size one.
                let empty = s.boxes.as_ref().and_then(|v| v.get(i)).is_none_or(|b| b.w <= 0.0 || b.h <= 0.0);
                if empty {
                    if let Some(v) = s.boxes.as_mut() {
                        v.remove(i);
                    }
                    s.undo.pop();
                    s.sel = None;
                }
            }
            s.drag = Drag::None;
            drop(s);
            set_cursor();
            area.queue_draw();
            side_now();
        });
    }
    area.add_controller(drag);

    // ---- keys (canvas focus only, so typing a caption never switches tools) -------------
    let keys = gtk::EventControllerKey::new();
    {
        let (st, area, set_tool, set_cursor, do_undo, size2, del) =
            (st.clone(), area.clone(), set_tool.clone(), set_cursor.clone(), do_undo.clone(), size.clone(), delete_sel.clone());
        let win2 = win.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            use gdk::Key;
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                Key::z | Key::Z if ctrl => do_undo(shift),
                Key::y if ctrl => do_undo(true),
                Key::b => set_tool(Tool::Brush),
                Key::e => set_tool(Tool::Erase),
                Key::c => set_tool(Tool::Box),
                Key::v => set_tool(Tool::Pan),
                Key::bracketleft => size2.set_value((size2.value() * 0.85).floor()),
                Key::bracketright => size2.set_value((size2.value() * 1.15).ceil()),
                Key::_0 | Key::f => {
                    st.borrow_mut().fit(area.width() as f64, area.height() as f64);
                    area.queue_draw();
                }
                Key::Delete | Key::BackSpace => del(),
                Key::space => {
                    st.borrow_mut().space = true;
                    set_cursor();
                    area.queue_draw();
                }
                Key::Escape => win2.close(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    {
        let (st, set_cursor, area) = (st.clone(), set_cursor.clone(), area.clone());
        keys.connect_key_released(move |_, key, _, _| {
            if key == gdk::Key::space {
                st.borrow_mut().space = false;
                set_cursor();
                area.queue_draw();
            }
        });
    }
    area.add_controller(keys);

    // ---- close --------------------------------------------------------------------------
    let cancelled = Rc::new(std::cell::Cell::new(false));
    {
        let (w, c) = (win.clone(), cancelled.clone());
        cancel.connect_clicked(move |_| {
            c.set(true);
            w.close();
        });
        let w = win.clone();
        ok.connect_clicked(move |_| {
            w.close();
        });
    }
    let done = RefCell::new(Some(done));
    {
        let st = st.clone();
        win.connect_close_request(move |_| {
            if cancelled.get() {
                return glib::Propagation::Proceed;
            }
            let Some(done) = done.borrow_mut().take() else { return glib::Propagation::Proceed };
            let s = st.borrow();
            let mask = s.mask_dirty.then(|| {
                let (w, h, stride) = (s.mask.width(), s.mask.height(), s.mask.stride() as usize);
                let raw = s.mask_bytes();
                let mut a = Vec::with_capacity((w * h) as usize);
                for y in 0..h as usize {
                    a.extend_from_slice(&raw[y * stride..y * stride + w as usize]);
                }
                Mask { w, h, a }
            });
            let boxes = s.boxes.clone().map(|v| v.into_iter().filter(|b| b.w > 0.0 && b.h > 0.0).collect());
            drop(s);
            done(Outcome { mask, boxes });
            glib::Propagation::Proceed
        });
    }
    win.present();
    area.grab_focus();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_snaps_like_the_backend() {
        assert_eq!(output_size(1000.0, 1000.0, 1.0), (992.0, 992.0)); // under budget: just snap
        assert_eq!(output_size(2000.0, 1000.0, 1.0), (1408.0, 704.0));
        assert_eq!(output_size(5.0, 5.0, 1.0), (16.0, 16.0));
    }

    #[test]
    fn boxes_round_trip() {
        let s = r#"[{"x":0.235,"y":0.246,"w":0.215,"h":0.129,"caption":"ear","strength":null},{"x":"0.1","y":0,"w":0.5,"h":0.5,"strength":0.9}]"#;
        let b = parse_boxes(s);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].strength, None);
        assert_eq!(b[1].x, 0.1);
        assert_eq!(parse_boxes(&boxes_json(&b)), b);
        assert!(parse_boxes("garbage").is_empty());
    }

    #[test]
    fn grid_snap() {
        assert_eq!(snap_n(0.5, 1024.0), 0.5);
        assert!((snap_n(0.01, 1024.0) - 16.0 / 1024.0).abs() < 1e-12);
        assert_eq!(snap_n(1.2, 1024.0), 1.0);
    }
}
