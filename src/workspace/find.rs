// SPDX-License-Identifier: GPL-3.0-only
//! Ctrl+F: browser-style find over the whole workflow. Unlike the web UI's node search it reads
//! everything the file holds — titles, types, widget labels and values (prompts, notes, combos,
//! numbers), socket names, properties, group titles — and descends into subgraphs.

use super::*;
use crate::{graph_view, registry::Registry};
use std::collections::HashSet;

struct Hit {
    path: Vec<String>,
    node: Option<String>,
    group: usize,
    /// "Title — field: …context…"
    line: String,
}

pub(super) struct Find {
    pub root: gtk::Box,
    entry: gtk::SearchEntry,
    count: gtk::Label,
    line: gtk::Label,
    hits: RefCell<Vec<Hit>>,
    cur: std::cell::Cell<usize>,
    canvas: RefCell<Option<canvas::Canvas>>,
}

impl Find {
    pub fn new() -> Self {
        let entry = gtk::SearchEntry::builder().placeholder_text("Find in workflow").width_chars(26).build();
        let count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).width_chars(7).build();
        let line = gtk::Label::builder()
            .css_classes(["dim-label", "caption"])
            .xalign(0.0)
            .max_width_chars(48)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .visible(false)
            .build();
        let row = gtk::Box::builder().spacing(4).build();
        row.append(&entry);
        row.append(&count);
        for (name, tip) in [("go-up-symbolic", "Previous (Shift+Enter)"), ("go-down-symbolic", "Next (Enter)"), ("window-close-symbolic", "Close (Esc)")] {
            row.append(&gtk::Button::builder().icon_name(name).tooltip_text(tip).css_classes(["flat", "circular"]).build());
        }
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .css_classes(["osd", "toolbar"])
            .visible(false)
            .build();
        root.append(&row);
        root.append(&line);
        Self { root, entry, count, line, hits: Default::default(), cur: Default::default(), canvas: Default::default() }
    }

    fn button(&self, i: usize) -> gtk::Button {
        let row = self.root.first_child().unwrap();
        let mut w = row.first_child().unwrap();
        for _ in 0..i + 2 {
            w = w.next_sibling().unwrap();
        }
        w.downcast().unwrap()
    }
}

impl Workspace {
    pub(super) fn setup_find(&self) {
        let f = &self.find;
        let me = self.clone();
        f.entry.connect_search_changed(move |_| me.run_find());
        let me = self.clone();
        f.entry.connect_activate(move |_| me.step_find(1));
        let me = self.clone();
        f.entry.connect_next_match(move |_| me.step_find(1));
        let me = self.clone();
        f.entry.connect_previous_match(move |_| me.step_find(-1));
        let me = self.clone();
        f.entry.connect_stop_search(move |_| me.close_find());
        let keys = gtk::EventControllerKey::new();
        let me = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gtk::gdk::Key::Return && mods.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                me.step_find(-1);
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        f.entry.add_controller(keys);
        for (i, d) in [(0, -1), (1, 1)] {
            let me = self.clone();
            f.button(i).connect_clicked(move |_| me.step_find(d));
        }
        let me = self.clone();
        f.button(2).connect_clicked(move |_| me.close_find());
    }

    pub(super) fn open_find(&self) {
        let f = &self.find;
        f.root.set_visible(true);
        f.entry.grab_focus();
        f.entry.select_region(0, -1);
        if !f.entry.text().is_empty() {
            self.run_find();
        }
    }

    fn close_find(&self) {
        let f = &self.find;
        f.root.set_visible(false);
        if let Some(c) = f.canvas.borrow_mut().take() {
            c.set_hits(HashSet::new(), None);
        }
        if let Some(c) = self.active_canvas() {
            c.area.grab_focus();
        }
    }

    fn run_find(&self) {
        let f = &self.find;
        let q = f.entry.text().to_lowercase();
        let hits = match self.active_doc().filter(|_| !q.is_empty()) {
            Some(doc) => {
                self.sync_doc(&doc);
                let d = doc.borrow();
                collect(&d.wf, &self.registry.borrow(), &q)
            }
            None => vec![],
        };
        f.cur.set(0);
        *f.hits.borrow_mut() = hits;
        self.show_hit();
    }

    fn step_find(&self, d: i32) {
        let f = &self.find;
        let n = f.hits.borrow().len();
        if n > 0 {
            f.cur.set((f.cur.get() as i32 + d).rem_euclid(n as i32) as usize);
        }
        self.show_hit();
    }

    /// Go to the current hit: switch subgraph level if needed, outline every match on this level,
    /// centre on the current one.
    fn show_hit(&self) {
        let f = &self.find;
        let hits = f.hits.borrow();
        let (Some(doc), cur) = (self.active_doc(), f.cur.get()) else { return };
        f.count.set_text(&match (hits.get(cur), f.entry.text().is_empty()) {
            (Some(_), _) => format!("{} / {}", cur + 1, hits.len()),
            (None, true) => String::new(),
            (None, false) => "no match".into(),
        });
        f.line.set_visible(!hits.is_empty());
        let Some(h) = hits.get(cur) else {
            if let Some(c) = f.canvas.borrow_mut().take() {
                c.set_hits(HashSet::new(), None);
            }
            return;
        };
        f.line.set_text(&h.line);
        if doc.borrow().path != h.path {
            doc.borrow_mut().path = h.path.clone();
            self.render_doc(&doc);
        }
        let canvas = doc.borrow().canvas.clone();
        if let Some(old) = f.canvas.borrow().as_ref().filter(|c| !c.same(&canvas)) {
            old.set_hits(HashSet::new(), None);
        }
        let ids = hits.iter().filter(|x| x.path == h.path).filter_map(|x| x.node.clone()).collect();
        canvas.set_hits(ids, h.node.clone());
        *f.canvas.borrow_mut() = Some(canvas.clone());
        match &h.node {
            Some(id) => self.center_on(&doc, id, true),
            None => {
                let d = doc.borrow();
                if let Some(g) = graph_ref(&d).groups.get(h.group) {
                    let (zoom, _) = canvas.view();
                    let (w, ht) = (canvas.area.width() as f64, canvas.area.height() as f64);
                    let (cx, cy) = (g.bounding[0] + g.bounding[2] / 2.0, g.bounding[1] + g.bounding[3] / 2.0);
                    canvas.set_view(zoom, (w / 2.0 - cx * zoom, ht / 2.0 - cy * zoom));
                }
            }
        }
    }
}

fn collect(wf: &workflow::Workflow, reg: &Registry, q: &str) -> Vec<Hit> {
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    walk(wf, reg, q, &wf.root, &mut Vec::new(), &mut seen, &mut out);
    out
}

fn walk(wf: &workflow::Workflow, reg: &Registry, q: &str, g: &workflow::Graph, path: &mut Vec<String>, seen: &mut HashSet<String>, out: &mut Vec<Hit>) {
    for (group, gr) in g.groups.iter().enumerate() {
        if gr.title.to_lowercase().contains(q) {
            out.push(Hit { path: path.clone(), node: None, group, line: format!("Group — {}", gr.title) });
        }
    }
    for n in &g.nodes {
        if let Some(line) = node_line(wf, g, reg, n, q) {
            out.push(Hit { path: path.clone(), node: Some(n.id.clone()), group: 0, line });
        }
        if let Some(sg) = wf.subgraphs.get(&n.ty) {
            if seen.insert(n.ty.clone()) {
                path.push(n.ty.clone());
                walk(wf, reg, q, &sg.graph, path, seen, out);
                path.pop();
            }
        }
    }
}

/// Every text a node shows or stores, as (field, text).
fn node_fields(wf: &workflow::Workflow, g: &workflow::Graph, reg: &Registry, n: &workflow::Node) -> Vec<(String, String)> {
    let mut f = vec![("title".to_owned(), graph_view::node_label(wf, reg, n)), ("type".into(), n.ty.clone()), ("id".into(), n.id.clone())];
    for w in graph_view::build_widgets(wf, g, n, reg, &mut Vec::new()) {
        let v = flat(&w.value);
        f.push((w.label.clone(), if v.is_empty() { w.label } else { format!("{}: {v}", w.label) }));
    }
    strings(&n.widgets_values, "value", &mut f);
    f.extend(n.inputs.iter().map(|i| ("input".to_owned(), i.name.clone())));
    f.extend(n.outputs.iter().map(|o| ("output".to_owned(), o.name.clone())));
    strings(&n.properties, "property", &mut f);
    f
}

fn node_line(wf: &workflow::Workflow, g: &workflow::Graph, reg: &Registry, n: &workflow::Node, q: &str) -> Option<String> {
    let f = node_fields(wf, g, reg, n);
    let (what, text) = f.iter().find(|(_, t)| t.to_lowercase().contains(q))?;
    Some(format!("{} — {what}: {}", f[0].1, context(text, q)))
}

fn flat(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

fn strings(v: &Value, what: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) => out.push((what.to_owned(), s.clone())),
        Value::Array(a) => a.iter().for_each(|x| strings(x, what, out)),
        Value::Object(m) => m.values().for_each(|x| strings(x, what, out)),
        _ => {}
    }
}

/// ~60 chars around the first match, on one line.
fn context(text: &str, q: &str) -> String {
    let lower = text.to_lowercase();
    let at = lower.find(q).map_or(0, |b| lower[..b].chars().count());
    let start = at.saturating_sub(20);
    let s: String = text.chars().skip(start).take(60).map(|c| if c == '\n' { ' ' } else { c }).collect();
    format!("{}{s}{}", if start > 0 { "…" } else { "" }, if text.chars().count() > start + 60 { "…" } else { "" })
}

#[cfg(test)]
mod tests {
    #[test]
    fn context_centres_on_match() {
        let t = format!("{}needle{}", "a".repeat(50), "b".repeat(50));
        let c = super::context(&t, "needle");
        assert!(c.starts_with('…') && c.ends_with('…') && c.contains("needle"));
        assert_eq!(super::context("a\nb", "b"), "a b");
    }
}
