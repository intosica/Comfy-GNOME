// SPDX-License-Identifier: GPL-3.0-only
//! In-node widgets: drawing (cairo) and the popover editors.
//!
//! The canvas owns a copy of each value; edits update that copy in place and are
//! reported through `Canvas::connect_widget`, which writes them to the model.

use adw::{gdk, gtk, prelude::*};
use gtk::cairo::Context;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;
use std::time::Duration;

pub const ROW_H: f64 = 20.0;
pub const GAP: f64 = 4.0;
pub const MULTI_MIN: f64 = 60.0;
/// Multiline text metrics, shared by `draw` and the scroll math in `mod.rs`.
pub const LINE_H: f64 = 14.0;
const TEXT_TOP: f64 = 15.0;
const TEXT_INSET: f64 = 8.0;

/// Backend used for prompt autocomplete. The editor silently stays plain when the
/// Lora Manager routes are absent, so this is safe for every text widget.
#[derive(Clone, Debug)]
pub struct TextCompletion {
    pub base: String,
}

#[derive(Clone, Debug)]
pub enum WKind {
    Int { min: f64, max: f64, step: f64 },
    Float { min: f64, max: f64, step: f64, prec: usize },
    Combo(Vec<String>),
    Bool,
    Text { multiline: bool },
    Button,
    Label,
    /// rgthree Power Lora row: value `{on, lora, strength}`; options = available loras.
    Lora(Vec<String>),
    /// EreNodes tag cloud (value = tags array); second field = available loras.
    Cloud(CloudMode),
    /// DaSiWa seed control (value = `seed_control_state` JSON): left = mode, right = 🎲.
    Seed,
    /// Value we cannot edit safely (unknown extension widgets, objects).
    ReadOnly,
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub label: String,
    /// Opaque to the canvas; handed back to the workspace on edit.
    pub key: String,
    pub kind: WKind,
    pub value: Value,
    /// Index of the node input this widget shadows (it gets a socket on its row).
    pub input: Option<usize>,
    /// Edit changes the node's structure: the workspace must re-render.
    pub structural: bool,
}

impl Widget {
    pub fn new(label: &str, key: impl Into<String>, kind: WKind, value: Value) -> Self {
        Self { label: label.to_owned(), key: key.into(), kind, value, input: None, structural: false }
    }
    pub fn multiline(&self) -> bool {
        matches!(self.kind, WKind::Text { multiline: true })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CloudMode {
    Cloud,
    Gallery(f64, f64),
    Toggle,
    /// Only active tags are shown; clicking the background adds one. `random`: 🎲 button.
    Select { random: bool },
}

/// Which part of a row was hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zone {
    Main,
    Dec,
    Inc,
    Toggle,
    /// Lora strength readout.
    Aux,
}

/// Serialize one widget value the way a read-only pill shows it.
pub fn show(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let flat: String = s.chars().map(|c| if c == '\n' { '⏎' } else { c }).collect();
            if flat.chars().count() > 44 { flat.chars().take(43).collect::<String>() + "…" } else { flat }
        }
        Value::Bool(b) => if *b { "on".into() } else { "off".into() },
        Value::Null => String::new(),
        Value::Array(a) => {
            let inner: Vec<String> = a.iter().take(3).map(show).collect();
            let mut s = inner.join(", ");
            if a.len() > 3 { s += " …"; }
            s
        }
        Value::Object(_) => "{…}".into(),
        v => v.to_string(),
    }
}

pub fn as_num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// Clamp + round `x` for a numeric widget kind.
pub fn num_value(kind: &WKind, x: f64) -> Value {
    match kind {
        WKind::Int { min, max, .. } => Value::from(x.round().clamp(*min, *max) as i64),
        WKind::Float { min, max, prec, .. } => {
            let p = 10f64.powi(*prec as i32);
            Value::from(((x.clamp(*min, *max)) * p).round() / p)
        }
        _ => Value::Null,
    }
}

pub fn step_of(kind: &WKind) -> f64 {
    match kind {
        WKind::Int { step, .. } | WKind::Float { step, .. } => *step,
        _ => 0.0,
    }
}

fn fmt_num(kind: &WKind, v: &Value) -> String {
    match kind {
        WKind::Float { prec, .. } => format!("{:.*}", prec, as_num(v)),
        _ => v.as_i64().map(|i| i.to_string()).or_else(|| v.as_u64().map(|u| u.to_string())).unwrap_or_else(|| format!("{}", as_num(v) as i64)),
    }
}

/// Hit zone of `x` (absolute) inside row `r`.
pub fn zone(w: &Widget, r: [f64; 4], x: f64) -> Zone {
    let (l, rt) = (x - r[0], r[0] + r[2] - x);
    match &w.kind {
        WKind::Int { .. } | WKind::Float { .. } | WKind::Combo(_) => {
            if l < 18.0 { Zone::Dec } else if rt < 18.0 { Zone::Inc } else { Zone::Main }
        }
        WKind::Lora(_) => {
            if l < 24.0 { Zone::Toggle }
            else if rt < 18.0 { Zone::Inc }
            else if rt < 76.0 { if rt > 58.0 { Zone::Dec } else { Zone::Aux } }
            else { Zone::Main }
        }
        WKind::Seed => if rt < 28.0 { Zone::Inc } else { Zone::Main },
        _ => Zone::Main,
    }
}

// ---- EreNodes tag cloud ----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CHit {
    Menu,
    Add,
    Random,
    Tag(usize),
    Bg,
}

pub struct Pill {
    pub r: [f64; 4],
    pub hit: CHit,
}

fn label_of(mode: CloudMode, t: &Value) -> String {
    let n = crate::erenodes::display(t, mode == CloudMode::Toggle);
    let i = crate::erenodes::info(t);
    if i.is_empty() { n } else { format!("{n} {i}") }
}

/// Scratch context for measuring text with the widget font. Nothing is painted
/// with it, so a 1×1 surface is enough; cloning hands out a cheap reference.
fn scratch() -> Context {
    thread_local! {
        static SCRATCH: Context = {
            let s = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 1, 1).expect("scratch surface");
            let cr = Context::new(&s).expect("scratch context");
            cr.set_font_size(12.0);
            cr
        };
    }
    SCRATCH.with(|c| c.clone())
}

/// Wrapped lines of a multiline text widget `rw` wide, exactly as `draw` lays them out.
pub fn text_lines(text: &str, rw: f64) -> Vec<String> {
    wrap(&scratch(), text, (rw - 2.0 * TEXT_INSET).max(24.0))
}

/// Total height of a multiline text widget's wrapped text, `draw`'s geometry.
pub fn text_content_h(text: &str, rw: f64) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    (text_lines(text, rw).len() as f64 - 1.0) * LINE_H + TEXT_TOP + 6.0
}

/// Pill rects (relative to the row origin) and total height for a cloud of `width`.
pub fn cloud_layout(w: &Widget, width: f64) -> (Vec<Pill>, f64) {
    let WKind::Cloud(mode) = &w.kind else { return (vec![], ROW_H) };
    let cr = scratch();
    let mode = *mode;
    let tags = w.value.as_array().cloned().unwrap_or_default();
    let mut out = vec![];
    let mut x = 0.0;
    let mut y = 0.0;
    let mut btns = vec![CHit::Menu, CHit::Add];
    if matches!(mode, CloudMode::Select { random: true }) {
        btns.push(CHit::Random);
    }
    for b in btns {
        out.push(Pill { r: [x, 0.0, 20.0, 20.0], hit: b });
        x += 25.0;
    }
    let shown = |t: &Value| !matches!(mode, CloudMode::Select { .. }) || crate::erenodes::active(t);
    match mode {
        CloudMode::Toggle => {
            y = 25.0;
            for (i, _) in tags.iter().enumerate().filter(|(_, t)| shown(t)) {
                out.push(Pill { r: [0.0, y, width, 20.0], hit: CHit::Tag(i) });
                y += 25.0;
            }
            return (out, (y - 5.0).max(20.0));
        }
        CloudMode::Gallery(pw, ph) => {
            y = 25.0;
            x = 0.0;
            for (i, _) in tags.iter().enumerate() {
                if x + pw > width && x > 0.0 {
                    x = 0.0;
                    y += ph + 5.0;
                }
                out.push(Pill { r: [x, y, pw, ph], hit: CHit::Tag(i) });
                x += pw + 5.0;
            }
            return (out, if tags.is_empty() { 20.0 } else { y + ph });
        }
        _ => {}
    }
    for (i, t) in tags.iter().enumerate().filter(|(_, t)| shown(t)) {
        let pw = (super::text::width(&cr, &label_of(mode, t)) + 10.0).min(width);
        if x + pw > width && x > 0.0 {
            x = 0.0;
            y += 25.0;
        }
        out.push(Pill { r: [x, y, pw, 20.0], hit: CHit::Tag(i) });
        x += pw + 5.0;
    }
    (out, y + 20.0)
}

pub fn row_height(w: &Widget, width: f64) -> f64 {
    match w.kind {
        WKind::Cloud(..) => cloud_layout(w, width).1,
        _ => ROW_H,
    }
}

/// What lies under `p` (relative to the row origin).
pub fn cloud_hit(w: &Widget, width: f64, p: (f64, f64)) -> CHit {
    cloud_layout(w, width).0.iter().find(|q| p.0 >= q.r[0] && p.0 <= q.r[0] + q.r[2] && p.1 >= q.r[1] && p.1 <= q.r[1] + q.r[3]).map(|q| q.hit).unwrap_or(CHit::Bg)
}

/// Image keys of gallery tiles.
pub fn cloud_images(w: &Widget) -> Vec<String> {
    let WKind::Cloud(CloudMode::Gallery(pw, ph)) = &w.kind else { return vec![] };
    w.value.as_array().into_iter().flatten().filter_map(|t| crate::erenodes::image_key(t, *pw, *ph)).collect()
}

const KIND_COLORS: [(&str, u32); 4] = [("lora", 0x415041), ("embedding", 0x504149), ("group", 0x504C41), ("tag", 0x414650)];

pub fn draw_cloud(cr: &Context, w: &Widget, r: [f64; 4], imgs: &std::collections::HashMap<String, Option<gtk::cairo::ImageSurface>>) {
    let WKind::Cloud(mode) = &w.kind else { return };
    let mode = *mode;
    let tags = w.value.as_array().cloned().unwrap_or_default();
    for p in cloud_layout(w, r[2]).0 {
        let pr = [r[0] + p.r[0], r[1] + p.r[1], p.r[2], p.r[3]];
        let ty = pr[1] + 14.5;
        match p.hit {
            CHit::Tag(i) => {
                let t = &tags[i];
                let on = crate::erenodes::active(t);
                let fill = KIND_COLORS.iter().find(|(k, _)| *k == crate::erenodes::kind(t)).map_or(0x414650, |c| c.1);
                let label = label_of(mode, t);
                if let CloudMode::Gallery(..) = mode {
                    pill(cr, pr, 0x222222);
                    cr.save().ok();
                    cr.rectangle(pr[0], pr[1], pr[2], pr[3]);
                    cr.clip();
                    if let Some(Some(img)) = crate::erenodes::image_key(t, pr[2], pr[3]).and_then(|k| imgs.get(&k)) {
                        let (iw, ih) = (img.width() as f64, img.height() as f64);
                        let k = (pr[2] / iw).max(pr[3] / ih);
                        cr.save().ok();
                        cr.translate(pr[0] + (pr[2] - iw * k) / 2.0, pr[1] + (pr[3] - ih * k) / 2.0);
                        cr.scale(k, k);
                        if cr.set_source_surface(img, 0.0, 0.0).is_ok() {
                            cr.paint_with_alpha(if on { 1.0 } else { 0.25 }).ok();
                        }
                        cr.restore().ok();
                    }
                    let bar = [pr[0], pr[1] + pr[3] - 20.0, pr[2], 20.0];
                    rgb_a(cr, if on { fill } else { 0x222222 }, 0.9);
                    cr.rectangle(bar[0], bar[1], bar[2], bar[3]);
                    cr.fill().ok();
                    rgb(cr, if on { 0xeeeeee } else { 0x888888 });
                    cr.move_to(pr[0] + 5.0, bar[1] + 14.5);
                    super::text::show(cr, &fit(cr, &label, pr[2] - 10.0));
                    cr.restore().ok();
                } else {
                    pill(cr, pr, if on || matches!(mode, CloudMode::Select { .. }) { fill } else { 0x222222 });
                    let on_text = on || matches!(mode, CloudMode::Select { .. });
                    rgb(cr, if on_text { 0xeeeeee } else { 0x777777 });
                    let mut tx = pr[0] + 5.0;
                    if mode == CloudMode::Toggle {
                        rgb(cr, if on { 0x3b7a4a } else { 0x444444 });
                        cr.arc(pr[0] + 14.0, pr[1] + 10.0, 5.5, 0.0, 2.0 * PI);
                        cr.fill().ok();
                        rgb(cr, 0xeeeeee);
                        cr.arc(pr[0] + if on { 16.0 } else { 12.0 }, pr[1] + 10.0, 3.0, 0.0, 2.0 * PI);
                        cr.fill().ok();
                        rgb(cr, if on { 0xeeeeee } else { 0x777777 });
                        tx = pr[0] + 35.0;
                    }
                    cr.move_to(tx, ty);
                    super::text::show(cr, &fit(cr, &label, pr[2] - (tx - pr[0]) - 5.0));
                }
            }
            b => {
                pill(cr, pr, 0x3a3a3a);
                rgb(cr, 0xcccccc);
                let g = match b { CHit::Menu => "≡", CHit::Add => "+", _ => "🎲" };
                cr.move_to(pr[0] + (20.0 - super::text::width(cr, g)) / 2.0, ty);
                super::text::show(cr, g);
            }
        }
    }
}

fn rgb_a(cr: &Context, hex: u32, a: f64) {
    cr.set_source_rgba(((hex >> 16) & 0xff) as f64 / 255.0, ((hex >> 8) & 0xff) as f64 / 255.0, (hex & 0xff) as f64 / 255.0, a);
}

fn rgb(cr: &Context, hex: u32) {
    cr.set_source_rgb(((hex >> 16) & 0xff) as f64 / 255.0, ((hex >> 8) & 0xff) as f64 / 255.0, (hex & 0xff) as f64 / 255.0);
}

fn width(cr: &Context, s: &str) -> f64 {
    super::text::width(cr, s)
}

/// Shorten `s` with an ellipsis so it fits `max` wide.
pub fn fit(cr: &Context, s: &str, max: f64) -> String {
    if max <= 6.0 {
        return String::new();
    }
    let mut chars: Vec<char> = s.chars().take((max / 3.0) as usize + 2).collect();
    if chars.len() == s.chars().count() && width(cr, s) <= max {
        return s.to_owned();
    }
    while !chars.is_empty() {
        chars.pop();
        let t: String = chars.iter().collect::<String>() + "…";
        if width(cr, &t) <= max {
            return t;
        }
    }
    String::new()
}

fn pill(cr: &Context, r: [f64; 4], fill: u32) {
    pill_a(cr, r, fill, 1.0);
}

fn pill_a(cr: &Context, r: [f64; 4], fill: u32, a: f64) {
    let (x, y, w, h, rad) = (r[0], r[1], r[2], r[3], 6.0f64.min(r[3] / 2.0));
    cr.new_sub_path();
    cr.arc(x + w - rad, y + rad, rad, -PI / 2.0, 0.0);
    cr.arc(x + w - rad, y + h - rad, rad, 0.0, PI / 2.0);
    cr.arc(x + rad, y + h - rad, rad, PI / 2.0, PI);
    cr.arc(x + rad, y + rad, rad, PI, 1.5 * PI);
    cr.close_path();
    rgb_a(cr, fill, a);
    cr.fill().ok();
}

fn arrow(cr: &Context, cx: f64, cy: f64, right: bool) {
    let d = if right { 1.0 } else { -1.0 };
    cr.move_to(cx - 2.5 * d, cy - 4.0);
    cr.line_to(cx + 2.5 * d, cy);
    cr.line_to(cx - 2.5 * d, cy + 4.0);
    cr.close_path();
    cr.fill().ok();
}

/// Word-wrap `text` into lines no wider than `max`.
fn wrap(cr: &Context, text: &str, max: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let try_ = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
            if width(cr, &try_) <= max || cur.is_empty() {
                cur = try_;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_owned();
            }
        }
        lines.push(cur);
    }
    lines
}

/// Paint one widget into absolute rect `r`. `linked`: its input socket is wired;
/// `scroll`: the text offset of a multiline widget, in canvas pixels.
pub fn draw(cr: &Context, w: &Widget, r: [f64; 4], linked: bool, scroll: f64) {
    let [x, y, rw, rh] = r;
    let base = if matches!(w.kind, WKind::Button) { 0x3a3a3a } else { 0x222222 };
    let (dim, txt) = if linked { (0x555555, 0x777777) } else { (0x888888, 0xdddddd) };
    pill(cr, r, base);
    let ty = y + rh.min(ROW_H) / 2.0 + 4.0;
    match &w.kind {
        WKind::Int { .. } | WKind::Float { .. } | WKind::Combo(_) => {
            rgb(cr, dim);
            arrow(cr, x + 9.0, y + ROW_H / 2.0, false);
            arrow(cr, x + rw - 9.0, y + ROW_H / 2.0, true);
            let val = match &w.kind {
                WKind::Combo(_) => w.value.as_str().map(str::to_owned).unwrap_or_else(|| show(&w.value)),
                k => fmt_num(k, &w.value),
            };
            let vt = fit(cr, &val, rw - 36.0);
            let vw = width(cr, &vt);
            let label = fit(cr, &w.label, (rw - 36.0 - vw - 8.0).max(0.0));
            rgb(cr, dim);
            cr.move_to(x + 20.0, ty);
            super::text::show(cr, &label);
            rgb(cr, txt);
            cr.move_to(x + rw - 20.0 - vw, ty);
            super::text::show(cr, &vt);
        }
        WKind::Bool => {
            rgb(cr, dim);
            cr.move_to(x + 10.0, ty);
            super::text::show(cr, &fit(cr, &w.label, rw - 52.0));
            let on = w.value.as_bool().unwrap_or(false);
            pill(cr, [x + rw - 34.0, y + 4.0, 24.0, 12.0], if on { 0x3b7a4a } else { 0x444444 });
            rgb(cr, 0xeeeeee);
            cr.arc(x + rw - if on { 16.0 } else { 28.0 }, y + 10.0, 4.5, 0.0, 2.0 * PI);
            cr.fill().ok();
        }
        WKind::Text { multiline: false } => {
            let val = w.value.as_str().map(str::to_owned).unwrap_or_else(|| show(&w.value));
            let vt = fit(cr, &val, rw - 20.0 - width(cr, &w.label).min(rw / 3.0));
            let vw = width(cr, &vt);
            rgb(cr, dim);
            cr.move_to(x + 10.0, ty);
            super::text::show(cr, &fit(cr, &w.label, (rw - 28.0 - vw).max(0.0)));
            rgb(cr, txt);
            cr.move_to(x + rw - 10.0 - vw, ty);
            super::text::show(cr, &vt);
        }
        WKind::Text { multiline: true } => {
            let text = w.value.as_str().unwrap_or("");
            let content = text_content_h(text, rw);
            let max = (content - rh).max(0.0);
            let off = scroll.clamp(0.0, max);
            cr.save().ok();
            cr.rectangle(x + 2.0, y + 2.0, rw - 4.0, rh - 4.0);
            cr.clip();
            if text.is_empty() {
                rgb(cr, 0x666666);
                cr.move_to(x + TEXT_INSET, y + TEXT_TOP);
                super::text::show(cr, &w.label);
            } else {
                rgb(cr, txt);
                for (i, line) in text_lines(text, rw).iter().enumerate() {
                    let ly = y + TEXT_TOP + i as f64 * LINE_H - off;
                    if ly > y + rh + LINE_H {
                        break;
                    }
                    if ly < y + TEXT_TOP - LINE_H {
                        continue;
                    }
                    cr.move_to(x + TEXT_INSET, ly);
                    super::text::show(cr, line);
                }
            }
            if max > 0.0 {
                let track = rh - 12.0;
                let thumb = (track * rh / content).max(10.0);
                pill_a(cr, [x + rw - 7.5, y + 6.0 + (track - thumb) * (off / max), 4.0, thumb], 0xffffff, 0.6);
            }
            cr.restore().ok();
        }
        WKind::Button | WKind::Label => {
            rgb(cr, if matches!(w.kind, WKind::Label) { 0x777777 } else { 0xcccccc });
            let t = fit(cr, &w.label, rw - 12.0);
            cr.move_to(x + (rw - width(cr, &t)) / 2.0, ty);
            super::text::show(cr, &t);
        }
        WKind::Lora(_) => {
            let on = w.value["on"].as_bool().unwrap_or(true);
            let alpha = if on { 1.0 } else { 0.45 };
            cr.set_source_rgba(0.35, 0.8, 0.45, alpha);
            cr.arc(x + 12.0, y + ROW_H / 2.0, 4.5, 0.0, 2.0 * PI);
            if on { cr.fill().ok(); } else { cr.set_line_width(1.2); cr.stroke().ok(); }
            let name = w.value["lora"].as_str().unwrap_or("None");
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            cr.set_source_rgba(0.87, 0.87, 0.87, alpha);
            cr.move_to(x + 24.0, ty);
            super::text::show(cr, &fit(cr, name, rw - 24.0 - 80.0));
            rgb(cr, 0x888888);
            arrow(cr, x + rw - 69.0, y + ROW_H / 2.0, false);
            arrow(cr, x + rw - 9.0, y + ROW_H / 2.0, true);
            let s = format!("{:.2}", as_num(&w.value["strength"]));
            rgb(cr, 0xdddddd);
            cr.move_to(x + rw - 39.0 - width(cr, &s) / 2.0, ty);
            super::text::show(cr, &s);
        }
        WKind::Seed => {
            let st: Value = serde_json::from_str(w.value.as_str().unwrap_or("")).unwrap_or(Value::Null);
            let random = st["mode"] != "fixed";
            let last = st["last_seed"].as_str().unwrap_or("—");
            let last: String = if last.chars().count() > 10 {
                format!("…{}", last.chars().skip(last.chars().count() - 7).collect::<String>())
            } else {
                last.to_owned()
            };
            rgb(cr, dim);
            cr.move_to(x + 10.0, ty);
            super::text::show(cr, &if random { "🎲 Random" } else { "📌 Fixed" });
            rgb(cr, txt);
            cr.move_to(x + 96.0, ty);
            super::text::show(cr, &fit(cr, &last, (rw - 96.0 - 34.0).max(0.0)));
            cr.move_to(x + rw - 26.0, ty);
            super::text::show(cr, "🎲");
        }
        WKind::Cloud(..) => {}
        WKind::ReadOnly => {
            rgb(cr, 0x888888);
            let v = show(&w.value);
            let vt = fit(cr, &v, rw - 20.0 - width(cr, &w.label).min(rw / 2.0));
            cr.move_to(x + 10.0, ty);
            super::text::show(cr, &fit(cr, &w.label, rw / 2.0));
            rgb(cr, 0xaaaaaa);
            cr.move_to(x + rw - 10.0 - width(cr, &vt), ty);
            super::text::show(cr, &vt);
        }
    }
}

// ---- popover editors -------------------------------------------------------------

/// Vertical space a popover spends outside its content (shadow, padding, footer row).
const POPOVER_CHROME: f64 = 104.0;

/// Vertical room above and below widget rect `r` (area coords), in window pixels.
/// A popover that does not fit the window is never mapped at all, so editors size
/// themselves to whatever the window actually leaves them.
fn room(area: &gtk::DrawingArea, r: [i32; 4]) -> (f64, f64) {
    let Some(win) = area.root().and_then(|w| w.downcast::<gtk::Window>().ok()) else { return (600.0, 600.0) };
    let top = area.compute_bounds(&win).map_or(0.0, |b| b.y() as f64);
    let y = top + r[1] as f64;
    ((y - 8.0).max(0.0), (win.height() as f64 - y - r[3] as f64 - 8.0).max(0.0))
}

/// Height a popover child may spend on its own content: the room on the better side
/// of `r`, minus the popover chrome (shadow, padding, footer row) and a side margin.
fn budget(area: &gtk::DrawingArea, r: [i32; 4]) -> f64 {
    let (above, below) = room(area, r);
    (above.max(below) - 10.0 - POPOVER_CHROME).max(80.0)
}

fn popover(area: &gtk::DrawingArea, r: [i32; 4]) -> gtk::Popover {
    let p = gtk::Popover::new();
    p.set_parent(area);
    p.set_pointing_to(Some(&gdk::Rectangle::new(r[0], r[1], r[2], r[3])));
    let (above, below) = room(area, r);
    p.set_position(if above > below { gtk::PositionType::Top } else { gtk::PositionType::Bottom });
    let a = area.clone();
    p.connect_closed(move |p| {
        let (p, a) = (p.clone(), a.clone());
        gtk::glib::idle_add_local_once(move || {
            p.unparent();
            a.grab_focus();
        });
    });
    p
}

/// Number entry; commits when the popover closes with a changed value.
#[allow(clippy::too_many_arguments)]
pub fn edit_number(area: &gtk::DrawingArea, r: [i32; 4], v: f64, min: f64, max: f64, step: f64, digits: u32, commit: impl Fn(f64) + 'static) {
    let p = popover(area, r);
    let spin = gtk::SpinButton::with_range(min, max, step.max(1e-9));
    spin.set_digits(digits);
    spin.set_value(v);
    spin.set_width_chars(14);
    let pw = p.downgrade();
    spin.connect_activate(move |_| {
        if let Some(p) = pw.upgrade() {
            p.popdown();
        }
    });
    p.set_child(Some(&spin));
    let s = spin.clone();
    p.connect_closed(move |_| {
        s.update();
        if (s.value() - v).abs() > f64::EPSILON {
            commit(s.value());
        }
    });
    p.popup();
    spin.grab_focus();
}

/// Searchable option list. `extra` prepends a danger entry (e.g. "✕ Remove"), reported as `None`.
/// Asks for a preview image of a combo option; answers through the callback (maybe later,
/// `None` = no preview). Installed once by the workspace — the canvas knows nothing of models.
pub type PreviewFn = std::rc::Rc<dyn Fn(&str, Box<dyn FnOnce(Option<gtk::gdk::Texture>)>)>;

thread_local! {
    static PREVIEW: std::cell::RefCell<Option<PreviewFn>> = const { std::cell::RefCell::new(None) };
}

pub fn set_preview_provider(f: PreviewFn) {
    PREVIEW.with(|p| *p.borrow_mut() = Some(f));
}

pub fn edit_combo(area: &gtk::DrawingArea, r: [i32; 4], options: &[String], current: &str, extra: Option<&str>, commit: impl Fn(Option<String>) + 'static) {
    let p = popover(area, r);
    let commit = std::rc::Rc::new(commit);
    let search = gtk::SearchEntry::new();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let mut buttons: Vec<(gtk::Button, String)> = Vec::new();
    // Hover preview beside the list (every combo; hidden unless the provider finds an image).
    // Only lists of model files get the preview column; it is reserved up front because resizing an
    // open popover makes GTK dismiss it.
    const MODEL_EXT: &[&str] = &[".safetensors", ".ckpt", ".pt", ".pth", ".bin", ".gguf", ".sft", ".onnx"];
    let models = options.iter().any(|o| MODEL_EXT.iter().any(|x| o.to_lowercase().ends_with(x)));
    let provider = PREVIEW.with(|p| p.borrow().clone()).filter(|_| models);
    let pic = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).width_request(256).height_request(256).visible(provider.is_some()).build();
    pic.add_css_class("card");
    let hovered = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let mut add = |label: &str, value: Option<String>, danger: bool| {
        let lb = gtk::Label::builder().label(if label == current { format!("• {label}") } else { label.to_owned() }).xalign(0.0).build();
        if danger {
            lb.add_css_class("error");
        }
        let b = gtk::Button::builder().child(&lb).build();
        b.add_css_class("flat");
        let (pw, c, value2) = (p.downgrade(), commit.clone(), value.clone());
        b.connect_clicked(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
            c(value.clone());
        });
        if let (Some(f), Some(v)) = (provider.clone(), value2) {
            let motion = gtk::EventControllerMotion::new();
            let (pic, hov) = (pic.clone(), hovered.clone());
            motion.connect_enter(move |_, _, _| {
                *hov.borrow_mut() = v.clone();
                let (pic, hov, v) = (pic.clone(), hov.clone(), v.clone());
                f(&v.clone(), Box::new(move |t| {
                    if *hov.borrow() == v {
                        pic.set_paintable(t.as_ref());
                    }
                }));
            });
            b.add_controller(motion);
        }
        list.append(&b);
        buttons.push((b, label.to_lowercase()));
    };
    if let Some(e) = extra {
        add(e, None, true);
    }
    for o in options {
        add(o, Some(o.clone()), false);
    }
    let buttons = std::rc::Rc::new(buttons);
    let b2 = buttons.clone();
    search.connect_search_changed(move |s| {
        let q = s.text().to_lowercase();
        for (b, l) in b2.iter() {
            b.set_visible(l.contains(&q));
        }
    });
    let b3 = buttons.clone();
    search.connect_activate(move |_| {
        if let Some((b, _)) = b3.iter().find(|(b, _)| b.is_visible()) {
            b.emit_clicked();
        }
    });
    // Same fit rule as the text editor: a popover taller than the window side is never
    // mapped, so the list shrinks to whatever room the node leaves.
    let list_h = (budget(area, r) - 60.0).clamp(120.0, 280.0) as i32;
    let sc = gtk::ScrolledWindow::builder().child(&list).min_content_height(list_h).min_content_width(300).build();
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.append(&search);
    bx.append(&sc);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&bx);
    pic.set_valign(gtk::Align::Start);
    row.append(&pic);
    p.set_child(Some(&row));
    p.popup();
    search.grab_focus();
}

/// Token under the cursor for autocomplete: `(start, end, is_lora)` as char offsets.
fn completion_token(before: &str) -> Option<(usize, usize, bool)> {
    // Inside `<lora:` up to the strength separator.
    if let Some(pos) = before.rfind("<lora:") {
        let tail = &before[pos + 6..];
        if !tail.contains('>') && !tail.contains('<') {
            if tail.contains(':') {
                return None; // strength area: no name suggestions
            }
            let start = before[..pos + 6].chars().count();
            let len = tail.chars().count();
            if len >= 1 {
                return Some((start, start + len, true));
            }
        }
    }
    // Plain tag: the word after the last delimiter, without a trailing `:weight`.
    let start_byte = before
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, ' ' | '\n' | '\t' | ',' | '(' | ')' | '[' | ']' | '<' | '>'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let token = &before[start_byte..];
    let base = token.split(':').next().unwrap_or(token);
    if base.chars().count() < 2 || base.eq_ignore_ascii_case("lora") || !base.chars().any(char::is_alphanumeric) {
        return None;
    }
    let start = before[..start_byte].chars().count();
    Some((start, start + base.chars().count(), false))
}

/// Text editor; commits on close when changed.
pub fn edit_text(
    area: &gtk::DrawingArea,
    r: [i32; 4],
    text: &str,
    multiline: bool,
    completion: Option<TextCompletion>,
    commit: impl Fn(String) + 'static,
) {
    let p = popover(area, r);
    let old = text.to_owned();
    if multiline {
        let tv = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(8)
            .right_margin(8)
            .top_margin(6)
            .bottom_margin(6)
            .pixels_below_lines(2)
            .accepts_tab(false)
            .css_classes(["prompt-editor"])
            .build();
        p.add_css_class("prompt-popover");
        let buf = tv.buffer();
        buf.set_text(text);
        buf.place_cursor(&buf.end_iter());
        // Prompt keys: Ctrl+↑/↓ weight the word or selection (web `Comfy.EditAttention`),
        // Ctrl+Enter closes (= saves; clicking outside or Esc saves too).
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (b2, pw) = (buf.clone(), p.downgrade());
        keys.connect_key_pressed(move |_, key, _, mods| {
            use gdk::Key;
            if !mods.contains(gdk::ModifierType::CONTROL_MASK) {
                return gtk::glib::Propagation::Proceed;
            }
            match key {
                Key::Return | Key::KP_Enter => {
                    if let Some(p) = pw.upgrade() {
                        p.popdown();
                    }
                }
                Key::Up | Key::Down => {
                    let (s, e) = b2.selection_bounds().unwrap_or_else(|| {
                        let i = b2.iter_at_mark(&b2.get_insert());
                        (i, i)
                    });
                    let all = b2.text(&b2.start_iter(), &b2.end_iter(), false);
                    let delta = if key == Key::Up { 0.05 } else { -0.05 };
                    if let Some((t, (a, z))) = super::attention::edit(&all, (s.offset() as usize, e.offset() as usize), delta) {
                        // Replace just the group in one user action (`set_text` is not undoable).
                        let old_len = all.chars().count() + (z - a) - t.chars().count();
                        let group: String = t.chars().skip(a).take(z - a).collect();
                        b2.begin_user_action();
                        b2.delete(&mut b2.iter_at_offset(a as i32), &mut b2.iter_at_offset((a + old_len) as i32));
                        b2.insert(&mut b2.iter_at_offset(a as i32), &group);
                        b2.end_user_action();
                        b2.select_range(&b2.iter_at_offset(a as i32), &b2.iter_at_offset(z as i32));
                    }
                }
                _ => return gtk::glib::Propagation::Proceed,
            }
            gtk::glib::Propagation::Stop
        });
        tv.add_controller(keys);
        // As wide as the widget on screen (min 420), tall enough to read a real prompt,
        // and never taller than the side of the window it opens into.
        let fit = budget(area, r);
        let show_sug = completion.is_some() && fit >= 200.0;
        let sug_h = if show_sug { 140.0f64.min(fit - 120.0) } else { 0.0 };
        let editor_h = (fit - sug_h).clamp(110.0, 260.0);
        let w = r[2].clamp(420, 900);
        let sc = gtk::ScrolledWindow::builder()
            .child(&tv)
            .min_content_width(w)
            // Fixed height: a popover that changes size while open gets dismissed on X11.
            .min_content_height(editor_h as i32)
            .width_request(w)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        sc.add_css_class("card");
        // Autocomplete list lives inside the editor popover, so it cannot lose focus
        // or fight the canvas' own popovers.
        let sug_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        sug_list.add_css_class("boxed-list");
        // Space is reserved up front for the same reason; "hidden" just means an empty list.
        sug_list.set_placeholder(Some(&gtk::Label::builder().label("Suggestions appear as you type").css_classes(["dim-label", "caption"]).build()));
        let sug_sc = gtk::ScrolledWindow::builder()
            .child(&sug_list)
            .min_content_height(sug_h.max(70.0) as i32)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .visible(show_sug)
            .build();
        if let Some(comp) = completion {
            let prefs = Rc::new(RefCell::new(crate::api::LmPrefs::default()));
            let base_for_prefs = comp.base.clone();
            let prefs_for_load = prefs.clone();
            gtk::glib::spawn_future_local(async move {
                if let Ok(p) = gtk::gio::spawn_blocking(move || crate::api::lm_prefs(&base_for_prefs)).await {
                    *prefs_for_load.borrow_mut() = p;
                }
            });
            let items: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
            let tok: Rc<RefCell<Option<(usize, usize, bool)>>> = Rc::default();
            let sel = Rc::new(Cell::new(0usize));
            let gen = Rc::new(Cell::new(0u64));
            let visible = Rc::new(Cell::new(false));

            let render: Rc<dyn Fn()> = {
                let (list, sc, items, sel, visible) =
                    (sug_list.clone(), sug_sc.clone(), items.clone(), sel.clone(), visible.clone());
                Rc::new(move || {
                    while let Some(c) = list.first_child() {
                        list.remove(&c);
                    }
                    let items = items.borrow();
                    if items.is_empty() {
                        visible.set(false);
                        return;
                    }
                    for (name, detail) in items.iter() {
                        let row = gtk::ListBoxRow::new();
                        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        b.set_margin_top(5);
                        b.set_margin_bottom(5);
                        b.set_margin_start(8);
                        b.set_margin_end(8);
                        let l = gtk::Label::builder()
                            .label(name)
                            .xalign(0.0)
                            .hexpand(true)
                            .ellipsize(gtk::pango::EllipsizeMode::Middle)
                            .build();
                        b.append(&l);
                        if !detail.is_empty() {
                            b.append(&gtk::Label::builder().label(detail).css_classes(["dim-label", "caption"]).build());
                        }
                        row.set_child(Some(&b));
                        list.append(&row);
                    }
                    sel.set(0);
                    list.select_row(list.row_at_index(0).as_ref());
                    sc.vadjustment().set_value(0.0);
                    visible.set(true);
                })
            };

            let accept: Rc<dyn Fn()> = {
                let (buf, items, tok, sel, visible, prefs) =
                    (buf.clone(), items.clone(), tok.clone(), sel.clone(), visible.clone(), prefs.clone());
                let sug_list = sug_list.clone();
                Rc::new(move || {
                    if !visible.get() {
                        return;
                    }
                    let (Some((start, end, lora)), Some((name, _))) =
                        (*tok.borrow(), items.borrow().get(sel.get()).cloned())
                    else {
                        return;
                    };
                    let p = prefs.borrow().clone();
                    let mut name = name;
                    if !lora && p.space_underscore {
                        name = name.replace(' ', "_");
                    }
                    let cursor = buf.iter_at_mark(&buf.get_insert()).offset();
                    let (from, to, repl) = if lora {
                        let mut it = buf.iter_at_offset(cursor);
                        let mut to = cursor;
                        while !it.is_end() {
                            let ch = it.char();
                            if ch == '>' {
                                to = it.offset() + 1;
                                break;
                            }
                            if ch == '\n' {
                                break;
                            }
                            to = it.offset() + 1;
                            it.forward_char();
                        }
                        (start as i32, to, format!("{name}:1.00>"))
                    } else {
                        let mut repl = name;
                        if p.append_comma {
                            let it = buf.iter_at_offset(end as i32);
                            let next = if it.is_end() { None } else { Some(it.char()) };
                            if !matches!(next, Some(',') | Some('.') | Some(')')) {
                                repl.push_str(", ");
                            }
                        }
                        (start as i32, end as i32, repl)
                    };
                    buf.begin_user_action();
                    buf.delete(&mut buf.iter_at_offset(from), &mut buf.iter_at_offset(to));
                    buf.insert(&mut buf.iter_at_offset(from), &repl);
                    buf.end_user_action();
                    buf.place_cursor(&buf.iter_at_offset(from + repl.chars().count() as i32));
                    sug_list.remove_all();
                    visible.set(false);
                })
            };

            let query: Rc<dyn Fn()> = {
                let (buf, items, tok, gen, prefs, render) = (buf.clone(), items.clone(), tok.clone(), gen.clone(), prefs.clone(), render.clone());
                let base = comp.base.clone();
                Rc::new(move || {
                    let before = buf.text(&buf.start_iter(), &buf.iter_at_mark(&buf.get_insert()), false).to_string();
                    let Some((start, end, lora)) = completion_token(&before) else {
                        tok.replace(None);
                        gen.set(gen.get() + 1); // drop any reply still in flight
                        items.borrow_mut().clear();
                        render();
                        return;
                    };
                    if !lora && !prefs.borrow().tags {
                        gen.set(gen.get() + 1);
                        items.borrow_mut().clear();
                        render();
                        return;
                    }
                    tok.replace(Some((start, end, lora)));
                    let q: String = before.chars().skip(start).take(end - start).collect();
                    let g = gen.get() + 1;
                    gen.set(g);
                    let (items2, render2, gen2, base2) =
                        (items.clone(), render.clone(), gen.clone(), base.clone());
                    gtk::glib::timeout_add_local(Duration::from_millis(150), move || {
                        if gen2.get() == g {
                            let (q, base3, items3, render3, gen3) =
                                (q.clone(), base2.clone(), items2.clone(), render2.clone(), gen2.clone());
                            gtk::glib::spawn_future_local(async move {
                                if let Ok(list) = gtk::gio::spawn_blocking(move || crate::api::lm_suggest(&base3, &q, lora)).await {
                                    if gen3.get() == g {
                                        *items3.borrow_mut() = list;
                                        render3();
                                    }
                                }
                            });
                        }
                        gtk::glib::ControlFlow::Break
                    });
                })
            };
            let q = query.clone();
            buf.connect_changed(move |_| q());
            let acc = accept.clone();
            let sel_row = sel.clone();
            sug_list.connect_row_activated(move |_, row| {
                sel_row.set(row.index().max(0) as usize);
                acc();
            });
            let nav = gtk::EventControllerKey::new();
            nav.set_propagation_phase(gtk::PropagationPhase::Capture);
            let (vis2, sel2, items2, prefs2, accept2, list2) =
                (visible.clone(), sel.clone(), items.clone(), prefs.clone(), accept.clone(), sug_list.clone());
            nav.connect_key_pressed(move |_, key, _, mods| {
                use gdk::Key;
                if !vis2.get() || mods.contains(gdk::ModifierType::CONTROL_MASK) {
                    return gtk::glib::Propagation::Proceed;
                }
                match key {
                    Key::Up | Key::Down => {
                        let n = items2.borrow().len();
                        if n > 0 {
                            let i = if key == Key::Up { (sel2.get() + n - 1) % n } else { (sel2.get() + 1) % n };
                            sel2.set(i);
                            list2.select_row(list2.row_at_index(i as i32).as_ref());
                        }
                        gtk::glib::Propagation::Stop
                    }
                    Key::Tab if prefs2.borrow().accept_tab => {
                        accept2();
                        gtk::glib::Propagation::Stop
                    }
                    Key::Return | Key::KP_Enter if prefs2.borrow().accept_enter => {
                        accept2();
                        gtk::glib::Propagation::Stop
                    }
                    Key::Escape => {
                        items2.borrow_mut().clear();
                        list2.remove_all();
                        vis2.set(false);
                        gtk::glib::Propagation::Stop
                    }
                    _ => gtk::glib::Propagation::Proceed,
                }
            });
            tv.add_controller(nav);
        }
        let hint = gtk::Label::builder()
            .label("Ctrl+↑/↓ weight · Tab/Enter complete · Ctrl+Enter done")
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        let count = gtk::Label::builder().xalign(1.0).hexpand(true).css_classes(["dim-label", "caption", "numeric"]).build();
        let set_count = {
            let count = count.clone();
            move |b: &gtk::TextBuffer| count.set_label(&format!("{} chars", b.char_count()))
        };
        set_count(&buf);
        buf.connect_changed(set_count);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        foot.append(&hint);
        foot.append(&count);
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
        bx.append(&sc);
        bx.append(&sug_sc);
        bx.append(&foot);
        p.set_child(Some(&bx));
        let tv2 = tv.clone();
        p.connect_closed(move |_| {
            let b = tv2.buffer();
            let new = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            if new != old {
                commit(new);
            }
        });
        p.popup();
        tv.grab_focus();
    } else {
        let e = gtk::Entry::builder().text(text).width_chars(34).build();
        let pw = p.downgrade();
        e.connect_activate(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
        });
        p.set_child(Some(&e));
        let e2 = e.clone();
        p.connect_closed(move |_| {
            let new = e2.text().to_string();
            if new != old {
                commit(new);
            }
        });
        p.popup();
        e.grab_focus();
    }
}

/// Copy of the lora object `v` with `field` replaced.
pub fn lora_with(v: &Value, field: &str, nv: Value) -> Value {
    let mut o = if v.is_object() { v.clone() } else { json!({}) };
    o[field] = nv;
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_clamp_and_round() {
        let f = WKind::Float { min: 0.0, max: 1.0, step: 0.01, prec: 2 };
        assert_eq!(num_value(&f, 0.123456), json!(0.12));
        assert_eq!(num_value(&f, 5.0), json!(1.0));
        let i = WKind::Int { min: 0.0, max: 10.0, step: 1.0 };
        assert_eq!(num_value(&i, -3.0), json!(0));
        assert_eq!(fmt_num(&f, &json!(0.5)), "0.50");
    }

    #[test]
    fn lora_zones() {
        let w = Widget::new("l", "k", WKind::Lora(vec![]), json!({}));
        let r = [0.0, 0.0, 300.0, 20.0];
        assert_eq!(zone(&w, r, 5.0), Zone::Toggle);
        assert_eq!(zone(&w, r, 100.0), Zone::Main);
        assert_eq!(zone(&w, r, 295.0), Zone::Inc);
        assert_eq!(zone(&w, r, 235.0), Zone::Dec);
        assert_eq!(zone(&w, r, 260.0), Zone::Aux);
    }
}

/// Right-click action a widget row offers (the workspace maps it to a model edit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Flip a Power Lora row on / off.
    Toggle,
    MoveUp,
    MoveDown,
    /// Delete the row (`apply_widget` with a `Null` value).
    Remove,
}

/// Menu entries for widget row `i`, chosen by widget kind — the single place where a
/// custom widget kind opts into row actions. `false` in the tuple = disabled.
///
/// Power Lora rows follow rgthree's `getSlotMenuOptions`: toggle, move, remove.
pub fn menu(ws: &[Widget], i: usize) -> Vec<(String, Action, bool)> {
    let Some(w) = ws.get(i) else { return Vec::new() };
    match &w.kind {
        WKind::Lora(_) => {
            let on = w.value["on"].as_bool().unwrap_or(true);
            let same = |j: usize| ws.get(j).is_some_and(|o| matches!(o.kind, WKind::Lora(_)));
            vec![
                (format!("{} {}", if on { "⚫" } else { "🟢" }, if on { "Toggle Off" } else { "Toggle On" }), Action::Toggle, true),
                ("⬆️ Move Up".into(), Action::MoveUp, i > 0 && same(i - 1)),
                ("⬇️ Move Down".into(), Action::MoveDown, same(i + 1)),
                ("🗑️ Remove".into(), Action::Remove, true),
            ]
        }
        // Other kinds have no row actions yet; add them here together with their WKind.
        _ => Vec::new(),
    }
}
