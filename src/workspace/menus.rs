// SPDX-License-Identifier: GPL-3.0-only
//! Canvas right-click menus (node, subgraph, group, empty grid), the group toolbox and the
//! actions behind them. Mirrors the ComfyUI 1.60 frontend menus (`reference/* Right-Click.png`)
//! including the extension items that work without the web runtime: rgthree "Queue … Output
//! Nodes", KJNodes Get/Set helpers + "Recreate node", pysssss "Go to node"/"Arrange"/"Follow
//! execution", easy-use "Cleanup Of VRAM Usage"/"Reboot ComfyUI", and core group options.
//! Submenus slide in GNOME-style inside one popover (no nested popovers).

use super::*;
use std::collections::{HashMap, HashSet};

pub(super) enum Entry {
    Item { label: String, danger: bool, enabled: bool, icon: Option<&'static str>, f: Box<dyn Fn()> },
    /// Lazily built submenu page.
    Sub { label: String, icon: Option<&'static str>, enabled: bool, items: Rc<dyn Fn() -> Vec<Entry>> },
    /// Dim section title (the "Extensions" line of the group menu).
    Header(String),
    Line,
    Swatches(Vec<(u32, Box<dyn Fn()>)>),
}

/// `label` may carry a shortcut after a tab: "Copy\tCtrl+C".
pub(super) fn item(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: false, enabled: true, icon: None, f: Box::new(f) }
}

/// Menu item with an icon (`""` = blank space for alignment), optionally greyed out.
pub(super) fn item_i(icon: &'static str, label: impl Into<String>, enabled: bool, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: false, enabled, icon: Some(icon), f: Box::new(f) }
}

pub(super) fn danger(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: true, enabled: true, icon: None, f: Box::new(f) }
}

fn off(label: impl Into<String>) -> Entry {
    Entry::Item { label: label.into(), danger: false, enabled: false, icon: None, f: Box::new(|| {}) }
}

fn check(on: bool, label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    item_i(if on { "cg-lucide-check-symbolic" } else { "" }, label, true, f)
}

fn sub(label: impl Into<String>, items: impl Fn() -> Vec<Entry> + 'static) -> Entry {
    Entry::Sub { label: label.into(), icon: None, enabled: true, items: Rc::new(items) }
}

/// Copied nodes/groups (Ctrl+C, "Copy"), pasted with fresh ids at the pointer.
#[derive(Clone, Default)]
struct Clip {
    nodes: Vec<workflow::Node>,
    links: Vec<workflow::Link>,
    groups: Vec<workflow::Group>,
}

thread_local! {
    static CLIP: RefCell<Option<Clip>> = const { RefCell::new(None) };
    /// "Copy (Clipspace)": an image widget value to paste into another image node.
    static CLIPSPACE: RefCell<Option<String>> = const { RefCell::new(None) };
    /// pysssss "Follow execution": keep the running node centred.
    static FOLLOW: Cell<bool> = const { Cell::new(false) };
}

const GROUP_PAD: f64 = 10.0; // Comfy.GroupSelectedNodes.Padding default
const GROUP_TITLE: f64 = 34.0;
const NODE_TITLE: f64 = 30.0;

fn wire(g: &mut workflow::Graph, from: &str, fs: usize, to: &str, ts: usize) {
    let id = g.links.keys().max().copied().unwrap_or(0) + 1;
    let ty = g.node(from).and_then(|n| n.outputs.get(fs)).map(|o| o.ty.clone()).unwrap_or_default();
    // An input takes one wire: drop the old one.
    if let Some(old) = g.node(to).and_then(|n| n.inputs.get(ts)).and_then(|i| i.link) {
        unwire(g, old);
    }
    g.links.insert(id, workflow::Link { id, origin_id: from.into(), origin_slot: fs, target_id: to.into(), target_slot: ts, ty });
    if let Some(o) = g.nodes.iter_mut().find(|n| n.id == from).and_then(|n| n.outputs.get_mut(fs)) {
        o.links.push(id);
    }
    if let Some(i) = g.nodes.iter_mut().find(|n| n.id == to).and_then(|n| n.inputs.get_mut(ts)) {
        i.link = Some(id);
    }
}

fn unwire(g: &mut workflow::Graph, id: i64) {
    let Some(l) = g.links.remove(&id) else { return };
    if let Some(o) = g.nodes.iter_mut().find(|n| n.id == l.origin_id).and_then(|n| n.outputs.get_mut(l.origin_slot)) {
        o.links.retain(|x| *x != id);
    }
    if let Some(i) = g.nodes.iter_mut().find(|n| n.id == l.target_id).and_then(|n| n.inputs.get_mut(l.target_slot)) {
        if i.link == Some(id) {
            i.link = None;
        }
    }
}

/// KJNodes virtual Set/Get node (frontend-only; `prompt.rs` resolves them).
fn kj_node(kind: &str, id: String, name: &str, ty: &str, pos: (f64, f64)) -> workflow::Node {
    let set = kind == "SetNode";
    workflow::Node {
        id,
        ty: kind.into(),
        title: Some(format!("{}_{name}", if set { "Set" } else { "Get" })),
        pos,
        size: (210.0, 58.0),
        mode: 0,
        inputs: if set { vec![workflow::Input { name: ty.into(), ty: ty.into(), link: None, widget: None, label: None }] } else { Vec::new() },
        outputs: vec![workflow::Output { name: ty.into(), ty: ty.into(), links: Vec::new() }],
        widgets_values: serde_json::json!([name]),
        properties: if set { serde_json::json!({ "previousName": name }) } else { serde_json::json!({}) },
        color: None,
        bgcolor: None,
        collapsed: false,
        pinned: false,
        shape: 0,
    }
}

/// KJ `validateName`: `name`, else `name_0`, `name_1`… not used by another SetNode.
fn unique_set_name(g: &workflow::Graph, name: &str) -> String {
    let taken = |n: &str| g.nodes.iter().any(|x| x.ty == "SetNode" && x.widgets_values.get(0).and_then(Value::as_str) == Some(n));
    if !taken(name) {
        return name.into();
    }
    (0..).map(|i| format!("{name}_{i}")).find(|n| !taken(n)).unwrap_or_default()
}

/// Bounding box (with title bars) of nodes, litegraph `LGraphGroup.resizeTo` padding applied.
fn group_rect<'a>(nodes: impl Iterator<Item = &'a workflow::Node>) -> Option<[f64; 4]> {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for n in nodes {
        b = [b[0].min(n.pos.0), b[1].min(n.pos.1 - NODE_TITLE), b[2].max(n.pos.0 + n.size.0), b[3].max(n.pos.1 + n.size.1)];
    }
    (b[0] < f64::MAX).then(|| [b[0] - GROUP_PAD, b[1] - GROUP_PAD - GROUP_TITLE, b[2] - b[0] + 2.0 * GROUP_PAD, b[3] - b[1] + 2.0 * GROUP_PAD + GROUP_TITLE])
}

/// litegraph `LGraph.arrange` (float left: columns by longest path from the sources) and
/// pysssss "Arrange (float right)" (columns counted back from the sinks, right-aligned).
fn arrange(g: &mut workflow::Graph, right: bool) {
    let n = g.nodes.len();
    let at: HashMap<String, usize> = g.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
    let edges: Vec<(usize, usize)> = g.links.values().filter_map(|l| Some((*at.get(&l.origin_id)?, *at.get(&l.target_id)?))).collect();
    let mut level = vec![0usize; n];
    // Bounded relaxation: a cycle cannot loop forever.
    for _ in 0..n {
        let mut changed = false;
        for &(a, b) in &edges {
            let (from, to) = if right { (b, a) } else { (a, b) };
            if level[to] < level[from] + 1 {
                level[to] = level[from] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let max = level.iter().copied().max().unwrap_or(0);
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); max + 1];
    for (i, l) in level.iter().enumerate() {
        cols[if right { max - l } else { *l }].push(i);
    }
    let (margin, mut x) = (50.0, 50.0);
    for col in cols.iter_mut().filter(|c| !c.is_empty()) {
        col.sort_by(|a, b| g.nodes[*a].pos.1.total_cmp(&g.nodes[*b].pos.1));
        let w = col.iter().map(|&i| g.nodes[i].size.0).fold(100.0, f64::max);
        let mut y = margin + NODE_TITLE;
        for &i in col.iter() {
            let nd = &mut g.nodes[i];
            nd.pos = (if right { x + w - nd.size.0 } else { x }, y);
            y += nd.size.1 + margin + NODE_TITLE;
        }
        x += w + margin;
    }
}

impl Workspace {
    /// Popover menu at widget coordinates. Plain buttons, not GMenuModel: menu-item action
    /// resolution through widget action groups proved unreliable here.
    pub(super) fn show_menu(&self, area: &impl IsA<gtk::Widget>, x: f64, y: f64, entries: Vec<Entry>) {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        pop.add_css_class("cg-menu");
        let stack = gtk::Stack::builder()
            .hhomogeneous(false)
            .vhomogeneous(false)
            .interpolate_size(true)
            .transition_duration(180)
            .build();
        stack.add_child(&menu_page(&pop, &stack, None, entries));
        pop.set_child(Some(&stack));
        pop.set_parent(area);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let p = pop.clone();
        pop.connect_closed(move |_| p.unparent());
        pop.popup();
    }

    /// Run `f` on a fresh clone of the workspace + doc (menu callbacks outlive the call).
    fn act(&self, doc: &DocRef, f: impl Fn(&Self, &DocRef) + 'static) -> impl Fn() + 'static {
        let (me, d) = (self.clone(), doc.clone());
        move || f(&me, &d)
    }

    // -----------------------------------------------------------------------------------
    // Node menu (`reference/Node Right-Click.png`, `Subgraph Right-Click.png`)
    // -----------------------------------------------------------------------------------

    pub(super) fn node_entries(&self, doc: &DocRef, id: &str, ids: &[String]) -> Vec<Entry> {
        let ids: Vec<String> = if ids.iter().any(|i| i == id) { ids.to_vec() } else { vec![id.to_owned()] };
        let (n, is_sub) = {
            let d = doc.borrow();
            match graph_ref(&d).node(id) {
                Some(n) => (n.clone(), d.wf.subgraphs.contains_key(&n.ty)),
                None => return Vec::new(),
            }
        };
        let (header, has_def, outputs_sel, has_preview) = {
            let reg = self.registry.borrow();
            let header = if is_sub { "Subgraph node".to_owned() } else { reg.get(&n.ty).map_or(n.ty.clone(), |d| d.display_name.clone()) };
            let d = doc.borrow();
            let g = graph_ref(&d);
            let outs: Vec<String> = ids.iter().filter(|i| g.node(i).is_some_and(|n| self.is_output(&d, n, &reg))).cloned().collect();
            (header, reg.get(&n.ty).is_some(), outs, reg.get("PreviewAny").is_some())
        };
        let group_outs = self.group_of(doc, id).map(|gi| self.group_outputs(doc, gi)).unwrap_or_default();
        let id = id.to_owned();
        let mut e = vec![Entry::Header(header)];
        if is_sub {
            let i = id.clone();
            e.push(item("Edit Subgraph Widgets", self.act(doc, move |w, d| w.edit_subgraph_widgets(d, &i))));
            let i = id.clone();
            e.push(item("Unpack Subgraph", self.act(doc, move |w, d| w.unpack(d, &i))));
            let i = id.clone();
            e.push(item("Open Subgraph", self.act(doc, move |w, d| w.open_node(d, &i))));
        } else if has_def {
            let i = id.clone();
            e.push(item_i("cg-view-refresh-symbolic", "Reload Node", true, self.act(doc, move |w, d| w.recreate(d, &i, true))));
        }
        let s = ids.clone();
        e.push(item("Convert to Subgraph", self.act(doc, move |w, d| w.convert(d, &s, None))));
        {
            let (me, d, i) = (self.clone(), doc.clone(), id.clone());
            e.push(sub("Properties", move || me.property_entries(&d, &i)));
        }
        let o = outputs_sel.clone();
        e.push(Entry::Item {
            label: "Queue Selected Output Nodes (rgthree)".into(),
            danger: false,
            enabled: !o.is_empty(),
            icon: None,
            f: Box::new(self.act(doc, move |w, _| w.queue_prompt_to(false, Some(&o)))),
        });
        let go = group_outs.clone();
        e.push(Entry::Item {
            label: "Queue Group Output Nodes (rgthree)".into(),
            danger: false,
            enabled: !go.is_empty(),
            icon: None,
            f: Box::new(self.act(doc, move |w, _| w.queue_prompt_to(false, Some(&go)))),
        });
        let i = id.clone();
        e.push(item("Properties Panel", self.act(doc, move |w, d| w.properties_panel(d, &i))));
        if self.image_slot(doc, &id).is_some() {
            let i = id.clone();
            e.push(item("Open in Mask Editor", self.act(doc, move |w, d| w.open_mask_editor(d, &i))));
        }
        if self.bbox_slots_pub(doc, &id) {
            let i = id.clone();
            e.push(item("Edit Boxes & Mask…", self.act(doc, move |w, d| { w.on_bbox(d, &i, "bbox:edit", Value::Null); })));
        }
        e.push(Entry::Line);
        {
            let (i, t) = (id.clone(), n.title.clone().unwrap_or_else(|| n.ty.clone()));
            e.push(item("Title", self.act(doc, move |w, d| {
                let (w2, d2, i2) = (w.clone(), d.clone(), i.clone());
                w.text_dialog("Title", &t, move |t| w2.set_title(&d2, &i2, t));
            })));
        }
        {
            let (me, d, s, mode) = (self.clone(), doc.clone(), ids.clone(), n.mode);
            e.push(sub("Mode", move || {
                [("Always", 0), ("On Event", 1), ("Never", 2), ("On Trigger", 3)]
                    .into_iter()
                    .map(|(l, m)| {
                        let s = s.clone();
                        check(mode == m, l, me.act(&d, move |w, d| w.set_mode(d, &s, m)))
                    })
                    .collect()
            }));
        }
        let s = ids.clone();
        e.push(item("Resize", self.act(doc, move |w, d| w.resize_nodes(d, &s))));
        let s = ids.clone();
        e.push(item(if n.collapsed { "Expand" } else { "Collapse" }, self.act(doc, move |w, d| w.collapse(d, &s))));
        let s = ids.clone();
        e.push(item(if n.pinned { "Unpin" } else { "Pin" }, self.act(doc, move |w, d| w.pin(d, Some(&s), !n.pinned))));
        e.push(item("Pin (all)", self.act(doc, |w, d| w.pin(d, None, true))));
        e.push(item("Unpin (all)", self.act(doc, |w, d| w.pin(d, None, false))));
        {
            let (me, d, s) = (self.clone(), doc.clone(), ids.clone());
            e.push(sub("Colors", move || {
                let s2 = s.clone();
                vec![
                    item("No color", me.act(&d, move |w, d| w.set_node_color(d, &s2, None))),
                    Entry::Swatches(
                        NODE_COLORS
                            .iter()
                            .map(|(_, rgb)| {
                                let (s, rgb) = (s.clone(), *rgb);
                                (rgb, Box::new(me.act(&d, move |w, d| w.set_node_color(d, &s, Some(rgb)))) as Box<dyn Fn()>)
                            })
                            .collect(),
                    ),
                ]
            }));
        }
        {
            let (me, d, s, shape) = (self.clone(), doc.clone(), ids.clone(), n.shape);
            e.push(sub("Shapes", move || {
                [("Default", 0), ("Box", 1), ("Round", 2), ("Card", 4)]
                    .into_iter()
                    .map(|(l, v)| {
                        let s = s.clone();
                        check(shape == v, l, me.act(&d, move |w, d| w.set_shape(d, &s, v)))
                    })
                    .collect()
            }));
        }
        e.push(Entry::Line);
        let s = ids.clone();
        e.push(item("Bypass", self.act(doc, move |w, d| w.bypass(d, &s))));
        if self.image_slot(doc, &id).is_some() {
            let i = id.clone();
            e.push(item("Copy (Clipspace)", self.act(doc, move |w, d| {
                CLIPSPACE.set(w.image_slot(d, &i).map(|(_, v)| v));
            })));
            if CLIPSPACE.with_borrow(Option::is_some) {
                let i = id.clone();
                e.push(item("Paste (Clipspace)", self.act(doc, move |w, d| w.paste_clipspace(d, &i))));
            }
        }
        e.push(Entry::Line);
        let s = ids.clone();
        e.push(item("Clone", self.act(doc, move |w, d| w.duplicate_nodes(d, &s))));
        e.push(Entry::Line);
        let s = ids.clone();
        e.push(danger("Remove", self.act(doc, move |w, d| w.delete_nodes(d, &s))));
        // KJNodes context menu.
        if !n.inputs.is_empty() {
            let i = id.clone();
            e.push(item("Add GetNode", self.act(doc, move |w, d| w.add_get_node(d, &i))));
        }
        if !n.outputs.is_empty() {
            let i = id.clone();
            e.push(item("Add SetNode", self.act(doc, move |w, d| w.add_set_node(d, &i))));
            if has_preview {
                let i = id.clone();
                e.push(item("Add PreviewAsTextNode", self.act(doc, move |w, d| w.add_preview_node(d, &i))));
            }
            let s = ids.clone();
            e.push(item("Convert all outputs to Set/Get", self.act(doc, move |w, d| w.outputs_to_set_get(d, &s))));
        }
        if has_def && !is_sub {
            let (me, d, i) = (self.clone(), doc.clone(), id.clone());
            e.push(sub("Recreate node", move || {
                let (i1, i2) = (i.clone(), i.clone());
                vec![
                    item("Keep widget values", me.act(&d, move |w, d| w.recreate(d, &i1, true))),
                    item("Reset widget values", me.act(&d, move |w, d| w.recreate(d, &i2, false))),
                ]
            }));
        }
        e
    }

    fn is_output(&self, d: &Doc, n: &workflow::Node, reg: &registry::Registry) -> bool {
        reg.get(&n.ty).is_some_and(|def| def.output_node)
            || d.wf.subgraphs.get(&n.ty).is_some_and(|s| s.graph.nodes.iter().any(|i| self.is_output(d, i, reg)))
    }

    /// First group whose area holds node `id`.
    fn group_of(&self, doc: &DocRef, id: &str) -> Option<usize> {
        let d = doc.borrow();
        let g = graph_ref(&d);
        let at = g.nodes.iter().position(|n| n.id == id)?;
        (0..g.groups.len()).find(|&gi| graph_view::group_members(g, gi).contains(&at))
    }

    fn group_ids(&self, doc: &DocRef, gi: usize) -> Vec<String> {
        let d = doc.borrow();
        let g = graph_ref(&d);
        if gi >= g.groups.len() {
            return Vec::new();
        }
        graph_view::group_members(g, gi).into_iter().map(|i| g.nodes[i].id.clone()).collect()
    }

    fn group_outputs(&self, doc: &DocRef, gi: usize) -> Vec<String> {
        let ids = self.group_ids(doc, gi);
        let d = doc.borrow();
        let reg = self.registry.borrow();
        let g = graph_ref(&d);
        ids.into_iter().filter(|i| g.node(i).is_some_and(|n| self.is_output(&d, n, &reg))).collect()
    }

    /// litegraph "Properties >": scalar `properties`, each editable in a dialog.
    fn property_entries(&self, doc: &DocRef, id: &str) -> Vec<Entry> {
        let mut props: Vec<(String, Value)> = {
            let d = doc.borrow();
            let Some(n) = graph_ref(&d).node(id) else { return Vec::new() };
            let mut p: Vec<(String, Value)> = n.properties.as_object().into_iter().flatten().filter(|(_, v)| !v.is_object() && !v.is_array()).map(|(k, v)| (k.clone(), v.clone())).collect();
            if !p.iter().any(|(k, _)| k == "Node name for S&R") && !d.wf.subgraphs.contains_key(&n.ty) {
                p.insert(0, ("Node name for S&R".into(), Value::String(n.ty.clone())));
            }
            p
        };
        if props.is_empty() {
            return vec![off("No properties")];
        }
        props.sort_by(|a, b| a.0.cmp(&b.0));
        props
            .into_iter()
            .map(|(k, v)| {
                let shown = match &v {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                };
                let i = id.to_owned();
                item(format!("{k}\t{}", shown.chars().take(24).collect::<String>()), self.act(doc, move |w, d| {
                    let (w2, d2, i2, k2, v2) = (w.clone(), d.clone(), i.clone(), k.clone(), v.clone());
                    w.text_dialog(&k, &shown, move |t| {
                        let nv = match &v2 {
                            Value::Number(_) => t.trim().parse::<f64>().ok().and_then(|f| serde_json::Number::from_f64(f).map(Value::Number)),
                            Value::Bool(_) => Some(Value::Bool(t.trim() == "true")),
                            _ => Some(Value::String(t)),
                        };
                        let Some(nv) = nv else { return };
                        {
                            let mut d = d2.borrow_mut();
                            let Some(n) = graph_mut(&mut d).nodes.iter_mut().find(|n| n.id == i2) else { return };
                            if !n.properties.is_object() {
                                n.properties = serde_json::json!({});
                            }
                            n.properties[&k2] = nv;
                        }
                        w2.edited(&d2);
                    });
                }))
            })
            .collect()
    }

    fn properties_panel(&self, doc: &DocRef, id: &str) {
        doc.borrow().canvas.select(&[id.to_owned()]);
        self.panel_split.set_show_sidebar(true);
        self.panel.refresh(self);
    }

    /// Frontend "Edit Subgraph Widgets": the properties panel's subgraph editor.
    fn edit_subgraph_widgets(&self, doc: &DocRef, id: &str) {
        self.properties_panel(doc, id);
        self.panel.open_editor();
    }

    fn set_mode(&self, doc: &DocRef, ids: &[String], mode: i64) {
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.mode = mode;
            }
        }
        self.edited(doc);
    }

    fn set_shape(&self, doc: &DocRef, ids: &[String], shape: i64) {
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.shape = shape;
            }
        }
        self.edited(doc);
    }

    /// litegraph "Resize": shrink to the content (the canvas layout grows it back to fit).
    fn resize_nodes(&self, doc: &DocRef, ids: &[String]) {
        self.sync_doc(doc);
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.size.1 = 1.0;
            }
        }
        self.edited(doc);
        self.sync_doc(doc);
    }

    /// `ids` None = every node of the graph ("Pin (all)" / "Unpin (all)").
    fn pin(&self, doc: &DocRef, ids: Option<&[String]>, on: bool) {
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.is_none_or(|i| i.contains(&n.id))) {
                n.pinned = on;
            }
        }
        self.edited(doc);
    }

    fn paste_clipspace(&self, doc: &DocRef, id: &str) {
        let (Some((idx, _)), Some(v)) = (self.image_slot(doc, id), CLIPSPACE.with_borrow(Clone::clone)) else { return };
        {
            let mut d = doc.borrow_mut();
            let Some(n) = graph_mut(&mut d).nodes.iter_mut().find(|n| n.id == id) else { return };
            if let Some(slot) = n.widgets_values.get_mut(idx) {
                *slot = Value::String(v);
            }
        }
        self.edited(doc);
    }

    fn convert(&self, doc: &DocRef, ids: &[String], group: Option<usize>) {
        self.sync_doc(doc);
        let res = {
            let mut d = doc.borrow_mut();
            let path = d.path.last().cloned();
            let r = d.wf.convert_to_subgraph(path.as_deref(), ids);
            // A converted group goes inside the new subgraph with its nodes.
            if let (Ok(inst), Some(gi)) = (&r, group) {
                let ty = graph_ref(&d).node(inst).map(|n| n.ty.clone()).unwrap_or_default();
                let grp = (gi < graph_ref(&d).groups.len()).then(|| graph_mut(&mut d).groups.remove(gi));
                if let (Some(grp), Some(sg)) = (grp, d.wf.subgraphs.get_mut(&ty)) {
                    sg.graph.groups.push(grp);
                }
            }
            r
        };
        match res {
            Ok(inst) => {
                self.refresh_doc(doc);
                doc.borrow().canvas.select(&[inst]);
                self.set_dirty(doc, true);
            }
            Err(e) => self.append_log(&format!("[subgraph] {e}")),
        }
    }

    fn unpack(&self, doc: &DocRef, id: &str) {
        self.sync_doc(doc);
        let res = {
            let mut d = doc.borrow_mut();
            let path = d.path.last().cloned();
            let reg = self.registry.borrow();
            d.wf.unpack_subgraph(path.as_deref(), id, |n, w, v| {
                let Some(def) = reg.get(&n.ty) else { return };
                if let Some(s) = prompt::widget_slots(n, def).into_iter().find(|s| s.name == w) {
                    if let Some(slot) = n.widgets_values.get_mut(s.idx) {
                        *slot = v;
                    }
                }
            })
        };
        match res {
            Ok(fresh) => {
                self.refresh_doc(doc);
                doc.borrow().canvas.select(&fresh);
                self.set_dirty(doc, true);
            }
            Err(e) => self.append_log(&format!("[subgraph] {e}")),
        }
    }

    /// KJNodes "Recreate node": a fresh node from `/object_info`, same id and place, wires
    /// kept where the socket still exists (matched by name).
    fn recreate(&self, doc: &DocRef, id: &str, keep: bool) {
        self.sync_doc(doc);
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(at) = g.nodes.iter().position(|n| n.id == id) else { return };
            let old = g.nodes[at].clone();
            let Some(def) = self.registry.borrow().get(&old.ty).cloned() else { return };
            let mut n = workflow::Node::from_def(&def, old.id.clone(), old.pos);
            n.size.0 = old.size.0;
            (n.title, n.mode, n.color, n.bgcolor, n.collapsed, n.pinned, n.shape) = (old.title.clone(), old.mode, old.color.clone(), old.bgcolor.clone(), old.collapsed, old.pinned, old.shape);
            n.properties = old.properties.clone();
            if keep {
                n.widgets_values = old.widgets_values.clone();
            }
            let mut dead = Vec::new();
            for o in &old.inputs {
                let Some(l) = o.link else { continue };
                match n.inputs.iter().position(|i| i.name == o.name) {
                    Some(k) => {
                        n.inputs[k].link = Some(l);
                        n.inputs[k].label = o.label.clone();
                        if let Some(l) = g.links.get_mut(&l) {
                            l.target_slot = k;
                        }
                    }
                    None => dead.push(l),
                }
            }
            for (j, o) in old.outputs.iter().enumerate() {
                let k = n.outputs.iter().position(|x| x.name == o.name).or((j < n.outputs.len()).then_some(j));
                match k {
                    Some(k) => {
                        n.outputs[k].links = o.links.clone();
                        for l in &o.links {
                            if let Some(l) = g.links.get_mut(l) {
                                l.origin_slot = k;
                            }
                        }
                    }
                    None => dead.extend(o.links.iter().copied()),
                }
            }
            g.nodes[at] = n;
            for l in dead {
                unwire(g, l);
            }
        }
        self.edited(doc);
    }

    /// KJ "Add GetNode": left of the node, feeding its first free input.
    fn add_get_node(&self, doc: &DocRef, id: &str) {
        self.sync_doc(doc);
        let fresh = {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(n) = g.node(id).cloned() else { return };
            let Some(k) = n.inputs.iter().position(|i| i.link.is_none() && i.widget.is_none()).or_else(|| n.inputs.iter().position(|i| i.link.is_none())) else { return };
            let ty = n.inputs[k].ty.clone();
            // Point it at an existing variable of that type when there is one.
            let name = g
                .nodes
                .iter()
                .find(|s| s.ty == "SetNode" && s.inputs.first().is_some_and(|i| i.ty == ty))
                .and_then(|s| s.widgets_values.get(0).and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            let nid = g.next_id();
            g.nodes.push(kj_node("GetNode", nid.clone(), &name, &ty, (n.pos.0 - 240.0, n.pos.1 + k as f64 * 90.0)));
            wire(g, &nid, 0, id, k);
            nid
        };
        self.after_add(doc, fresh);
    }

    /// KJ "Add SetNode": right of the node, fed by its first unconnected output.
    fn add_set_node(&self, doc: &DocRef, id: &str) {
        self.sync_doc(doc);
        let fresh = {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(n) = g.node(id).cloned() else { return };
            let Some(k) = n.outputs.iter().position(|o| o.links.is_empty()) else { return };
            let o = &n.outputs[k];
            let name = unique_set_name(g, &o.name);
            let nid = g.next_id();
            g.nodes.push(kj_node("SetNode", nid.clone(), &name, &o.ty, (n.pos.0 + n.size.0 + 30.0, n.pos.1 + k as f64 * 90.0)));
            wire(g, id, k, &nid, 0);
            nid
        };
        self.after_add(doc, fresh);
    }

    /// KJ "Add PreviewAsTextNode": a core Preview Any on the first unconnected output.
    fn add_preview_node(&self, doc: &DocRef, id: &str) {
        self.sync_doc(doc);
        let Some(def) = self.registry.borrow().get("PreviewAny").cloned() else { return };
        let fresh = {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(n) = g.node(id).cloned() else { return };
            let Some(k) = n.outputs.iter().position(|o| o.links.is_empty()) else { return };
            let nid = g.next_id();
            g.nodes.push(workflow::Node::from_def(&def, nid.clone(), (n.pos.0 + n.size.0 + 30.0, n.pos.1 + k as f64 * 132.0)));
            wire(g, id, k, &nid, 0);
            nid
        };
        self.after_add(doc, fresh);
    }

    /// KJ `convertOutputsToSetGet`: each wired output → a Set node + one Get per target.
    fn outputs_to_set_get(&self, doc: &DocRef, ids: &[String]) {
        self.sync_doc(doc);
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            for id in ids {
                let Some(n) = g.node(id).cloned() else { continue };
                for (k, o) in n.outputs.iter().enumerate() {
                    let targets: Vec<(i64, String, usize)> = o
                        .links
                        .iter()
                        .filter_map(|l| g.links.get(l))
                        .filter(|l| g.node(&l.target_id).is_some_and(|t| t.ty != "SetNode" && t.ty != "GetNode"))
                        .map(|l| (l.id, l.target_id.clone(), l.target_slot))
                        .collect();
                    if targets.is_empty() {
                        continue;
                    }
                    let name = unique_set_name(g, if o.name.is_empty() { &o.ty } else { &o.name });
                    let sid = g.next_id();
                    let mut set = kj_node("SetNode", sid.clone(), &name, &o.ty, (n.pos.0 + n.size.0 + 30.0, n.pos.1 + k as f64 * 60.0));
                    set.collapsed = true;
                    g.nodes.push(set);
                    for (l, ..) in &targets {
                        unwire(g, *l);
                    }
                    wire(g, id, k, &sid, 0);
                    for (_, t, ts) in targets {
                        let Some(tp) = g.node(&t).map(|t| t.pos) else { continue };
                        let gid = g.next_id();
                        let mut get = kj_node("GetNode", gid.clone(), &name, &o.ty, (tp.0 - 240.0, tp.1));
                        get.collapsed = true;
                        g.nodes.push(get);
                        wire(g, &gid, 0, &t, ts);
                    }
                }
            }
        }
        self.edited(doc);
    }

    fn after_add(&self, doc: &DocRef, fresh: String) {
        self.refresh_doc(doc);
        doc.borrow().canvas.select(&[fresh]);
        self.set_dirty(doc, true);
    }

    // -----------------------------------------------------------------------------------
    // Canvas + group menus (`Grid Right-Click.png`, `Group Right-Click.png`)
    // -----------------------------------------------------------------------------------

    pub(super) fn context_menu(&self, doc: &DocRef, x: f64, y: f64, hit: canvas::Hit) {
        // Pseudo-nodes have no model counterpart: fall back to the canvas menu.
        let hit = match &hit {
            canvas::Hit::Node(id) | canvas::Hit::Widget(id, _) if !graph_ref(&doc.borrow()).nodes.iter().any(|n| &n.id == id) => canvas::Hit::Empty,
            _ => hit,
        };
        let area = doc.borrow().canvas.area.clone();
        let ids = {
            let d = doc.borrow();
            let g = graph_ref(&d);
            let mut ids = d.canvas.selected();
            ids.retain(|i| g.node(i).is_some());
            ids
        };
        let entries = match hit {
            // A widget row can carry its own actions (Power Lora rows, …); the list comes from
            // `canvas::widgets::menu`, so a custom widget kind opts in there.
            canvas::Hit::Widget(id, w) => {
                let actions = doc.borrow().canvas.widget_menu(&id, w);
                if actions.is_empty() {
                    return self.context_menu(doc, x, y, canvas::Hit::Node(id));
                }
                actions
                    .into_iter()
                    .map(|(label, action, enabled)| {
                        let i = id.clone();
                        item_i("", label, enabled, self.act(doc, move |me, d| me.widget_row_action(d, &i, w, action)))
                    })
                    .collect()
            }
            canvas::Hit::Node(id) => self.node_entries(doc, &id, &ids),
            canvas::Hit::Group(gi) => self.canvas_entries(doc, x, y, &ids, Some(gi)),
            canvas::Hit::Empty => self.canvas_entries(doc, x, y, &ids, None),
        };
        self.show_menu(&area, x, y, entries);
    }

    fn canvas_entries(&self, doc: &DocRef, x: f64, y: f64, ids: &[String], group: Option<usize>) -> Vec<Entry> {
        let (zoom, offset) = doc.borrow().canvas.view();
        let pos = ((x - offset.0) / zoom, (y - offset.1) / zoom);
        let ids = ids.to_vec();
        let mut e = vec![
            item("Cleanup Of VRAM Usage", self.act(doc, |w, _| w.free_vram())),
            item("Reboot ComfyUI", self.act(doc, |w, _| w.reboot())),
            Entry::Line,
        ];
        {
            let (me, d) = (self.clone(), doc.clone());
            e.push(sub("Add Node", move || me.add_node_entries(&d, pos, String::new())));
        }
        e.push(item("Add Group", self.act(doc, move |w, d| w.add_group(d, &[], Some(pos)))));
        let has_clip = CLIP.with_borrow(Option::is_some);
        e.push(item_i("", "Paste\tCtrl+V", has_clip, self.act(doc, move |w, d| w.paste(d, Some(pos)))));
        let outs = {
            let d = doc.borrow();
            let reg = self.registry.borrow();
            let g = graph_ref(&d);
            ids.iter().filter(|i| g.node(i).is_some_and(|n| self.is_output(&d, n, &reg))).cloned().collect::<Vec<_>>()
        };
        e.push(Entry::Item {
            label: "Queue Selected Output Nodes (rgthree)".into(),
            danger: false,
            enabled: !outs.is_empty(),
            icon: None,
            f: Box::new(self.act(doc, move |w, _| w.queue_prompt_to(false, Some(&outs)))),
        });
        let gouts = group.map(|gi| self.group_outputs(doc, gi)).unwrap_or_default();
        e.push(Entry::Item {
            label: "Queue Group Output Nodes (rgthree)".into(),
            danger: false,
            enabled: !gouts.is_empty(),
            icon: None,
            f: Box::new(self.act(doc, move |w, _| w.queue_prompt_to(false, Some(&gouts)))),
        });
        e.push(Entry::Line);
        match group {
            Some(gi) => {
                let s = ids.clone();
                e.push(item_i("", "Add Selected Nodes To Group", !ids.is_empty(), self.act(doc, move |w, d| w.add_to_group(d, gi, &s))));
                e.extend(self.group_node_entries(doc, gi));
            }
            None if !ids.is_empty() => {
                let s = ids.clone();
                e.push(item("Add Group For Selected Nodes", self.act(doc, move |w, d| w.group_selected(d, &s))));
            }
            None => {}
        }
        e.push(Entry::Line);
        e.push(item("Pin (all)", self.act(doc, |w, d| w.pin(d, None, true))));
        e.push(item("Unpin (all)", self.act(doc, |w, d| w.pin(d, None, false))));
        e.push(Entry::Line);
        e.push(check(FOLLOW.get(), "Follow execution", || FOLLOW.set(!FOLLOW.get())));
        {
            let (me, d) = (self.clone(), doc.clone());
            e.push(sub("Go to node", move || me.goto_entries(&d)));
        }
        e.push(item("Arrange (float left)", self.act(doc, |w, d| w.arrange(d, false))));
        e.push(item("Arrange (float right)", self.act(doc, |w, d| w.arrange(d, true))));
        e.push(Entry::Line);
        e.push(item("Select All\tCtrl+A", self.act(doc, |w, d| w.select_all(d))));
        e.push(item("Fit View", self.act(doc, |_, d| d.borrow().canvas.fit_view())));
        e.push(item("Reset View", self.act(doc, |_, d| d.borrow().canvas.reset_view())));
        if let Some(gi) = group {
            e.push(Entry::Line);
            let (me, d) = (self.clone(), doc.clone());
            e.push(sub("Edit Group", move || {
                vec![
                    item("Title", me.act(&d, move |w, d| w.rename_group_dialog(d, gi))),
                    me.group_color_sub(&d, gi),
                    item("Fit to Group", me.act(&d, move |w, d| w.fit_group(d, gi))),
                    Entry::Line,
                    danger("Remove", me.act(&d, move |w, d| w.delete_group(d, gi))),
                ]
            }));
        }
        e
    }

    /// `Comfy.GroupOptions` items for a group with nodes: fit, select, mode switches.
    fn group_node_entries(&self, doc: &DocRef, gi: usize) -> Vec<Entry> {
        let ids = self.group_ids(doc, gi);
        if ids.is_empty() {
            return Vec::new();
        }
        let mut e = vec![Entry::Line, item_i("cg-zoom-fit-best-symbolic", "Fit Group To Nodes", true, self.act(doc, move |w, d| w.fit_group_to_nodes(d, gi)))];
        let s = ids.clone();
        e.push(item_i("", "Select Nodes", true, self.act(doc, move |_, d| d.borrow().canvas.select(&s))));
        e.extend(self.group_mode_entries(doc, &ids));
        e
    }

    /// "Set Group Nodes to Always/Never", "Bypass Group Nodes", minus the mode they all share.
    fn group_mode_entries(&self, doc: &DocRef, ids: &[String]) -> Vec<Entry> {
        let shared = {
            let d = doc.borrow();
            let g = graph_ref(&d);
            let modes: HashSet<i64> = ids.iter().filter_map(|i| g.node(i)).map(|n| n.mode).collect();
            (modes.len() == 1).then(|| modes.into_iter().next()).flatten()
        };
        [("cg-media-playback-start-symbolic", "Set Group Nodes to Always", 0), ("cg-media-playback-stop-symbolic", "Set Group Nodes to Never", workflow::MODE_NEVER), ("cg-go-next-symbolic", "Bypass Group Nodes", workflow::MODE_BYPASS)]
            .into_iter()
            .filter(|(_, _, m)| shared != Some(*m))
            .map(|(icon, l, m)| {
                let s = ids.to_vec();
                item_i(icon, l, true, self.act(doc, move |w, d| w.set_mode(d, &s, m)))
            })
            .collect()
    }

    fn group_color_sub(&self, doc: &DocRef, gi: usize) -> Entry {
        let (me, d) = (self.clone(), doc.clone());
        sub("Color", move || {
            vec![Entry::Swatches(
                NODE_COLORS
                    .iter()
                    .map(|(hex, rgb)| (*rgb, Box::new(me.act(&d, move |w, d| w.set_group_color(d, gi, hex))) as Box<dyn Fn()>))
                    .collect(),
            )]
        })
    }

    fn rename_group_dialog(&self, doc: &DocRef, gi: usize) {
        let title = graph_ref(&doc.borrow()).groups.get(gi).map(|g| g.title.clone()).unwrap_or_default();
        let (me, d) = (self.clone(), doc.clone());
        self.text_dialog("Group Title", &title, move |t| me.rename_group(&d, gi, t));
    }

    /// Core "Add Node >" tree from the node categories, built a level at a time.
    fn add_node_entries(&self, doc: &DocRef, pos: (f64, f64), prefix: String) -> Vec<Entry> {
        let reg = self.registry.borrow();
        let (mut subs, mut nodes): (Vec<String>, Vec<(String, String)>) = (Vec::new(), Vec::new());
        for (cat, defs) in reg.by_category() {
            if cat == prefix {
                nodes.extend(defs.iter().filter(|d| !d.dev_only).map(|d| (d.display_name.clone(), d.name.clone())));
                continue;
            }
            let rest = if prefix.is_empty() { Some(cat) } else { cat.strip_prefix(prefix.as_str()).and_then(|r| r.strip_prefix('/')) };
            if let Some(head) = rest.and_then(|r| r.split('/').next()).filter(|h| !h.is_empty()) {
                if !subs.iter().any(|s| s == head) {
                    subs.push(head.to_owned());
                }
            }
        }
        drop(reg);
        let mut e: Vec<Entry> = subs
            .into_iter()
            .map(|s| {
                let full = if prefix.is_empty() { s.clone() } else { format!("{prefix}/{s}") };
                let (me, d) = (self.clone(), doc.clone());
                sub(s, move || me.add_node_entries(&d, pos, full.clone()))
            })
            .collect();
        e.extend(nodes.into_iter().map(|(label, name)| {
            item(label, self.act(doc, move |w, d| {
                w.create_node(d, &name, pos);
            }))
        }));
        e
    }

    /// pysssss "Go to node >": node types, then their instances.
    fn goto_entries(&self, doc: &DocRef) -> Vec<Entry> {
        let mut by_ty: Vec<(String, Vec<(String, String)>)> = Vec::new();
        {
            let d = doc.borrow();
            let reg = self.registry.borrow();
            for n in &graph_ref(&d).nodes {
                let ty = d.wf.subgraphs.get(&n.ty).map(|s| s.name.clone()).or_else(|| reg.get(&n.ty).map(|d| d.display_name.clone())).unwrap_or(n.ty.clone());
                let label = format!("#{} {}", n.id, n.title.clone().unwrap_or_else(|| ty.clone()));
                match by_ty.iter_mut().find(|(t, _)| *t == ty) {
                    Some((_, v)) => v.push((label, n.id.clone())),
                    None => by_ty.push((ty, vec![(label, n.id.clone())])),
                }
            }
        }
        by_ty.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        if by_ty.is_empty() {
            return vec![off("No nodes")];
        }
        by_ty
            .into_iter()
            .map(|(ty, nodes)| {
                let (me, d) = (self.clone(), doc.clone());
                sub(ty, move || nodes.iter().map(|(l, id)| {
                    let id = id.clone();
                    item(l.clone(), me.act(&d, move |w, d| w.center_on(d, &id, true)))
                }).collect())
            })
            .collect()
    }

    /// Centre the view on node `id` at the current zoom.
    fn center_on(&self, doc: &DocRef, id: &str, select: bool) {
        let d = doc.borrow();
        let Some(n) = graph_ref(&d).node(id) else { return };
        let (zoom, _) = d.canvas.view();
        let (w, h) = (d.canvas.area.width() as f64, d.canvas.area.height() as f64);
        let (cx, cy) = (n.pos.0 + n.size.0 / 2.0, n.pos.1 + n.size.1 / 2.0);
        d.canvas.set_view(zoom, (w / 2.0 - cx * zoom, h / 2.0 - cy * zoom));
        if select {
            d.canvas.select(&[id.to_owned()]);
        }
    }

    /// "Follow execution": called for every executing node.
    pub(super) fn follow(&self, node: &str) {
        if !FOLLOW.get() {
            return;
        }
        let Some(doc) = self.run_doc.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        if doc.borrow().path.is_empty() {
            self.center_on(&doc, node.split(':').next().unwrap_or(node), false);
        }
    }

    fn arrange(&self, doc: &DocRef, right: bool) {
        self.sync_doc(doc);
        arrange(graph_mut(&mut doc.borrow_mut()), right);
        self.edited(doc);
        doc.borrow().canvas.fit_view();
    }

    fn free_vram(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let r = gio::spawn_blocking(move || client.post("/free", serde_json::json!({ "unload_models": true, "free_memory": true }))).await;
            me.append_log(match r {
                Ok(Ok(_)) => "[backend] models unloaded, memory freed",
                _ => "[backend] /free failed",
            });
        });
    }

    /// Restart the backend: ours is relaunched; a foreign one is asked through
    /// ComfyUI-Manager (`/api/manager/reboot`) or easy-use (`/easyuse/reboot`).
    fn reboot(&self) {
        let win = self.root.root().and_downcast::<gtk::Window>();
        let dlg = adw::AlertDialog::builder().heading("Reboot ComfyUI?").body("Running and queued jobs are lost.").close_response("cancel").build();
        dlg.add_response("cancel", "Cancel");
        dlg.add_response("reboot", "Reboot");
        dlg.set_response_appearance("reboot", adw::ResponseAppearance::Destructive);
        let me = self.clone();
        dlg.choose(win.as_ref(), gio::Cancellable::NONE, move |r| {
            if r.as_str() != "reboot" {
                return;
            }
            if me.proc.borrow().is_some() {
                me.stop_backend();
                me.start(backend::LaunchConfig::load());
                return;
            }
            let Some(client) = me.client.borrow().clone() else { return };
            let me2 = me.clone();
            glib::spawn_future_local(async move {
                let ok = gio::spawn_blocking(move || client.get("/api/manager/reboot").is_ok() || client.get("/easyuse/reboot").is_ok()).await.unwrap_or(false);
                me2.append_log(if ok { "[backend] reboot requested" } else { "[backend] reboot needs ComfyUI-Manager or easy-use on the backend" });
            });
        });
    }

    // -----------------------------------------------------------------------------------
    // Groups
    // -----------------------------------------------------------------------------------

    /// "Add Group For Selected Nodes": a group fitted around the selection.
    fn group_selected(&self, doc: &DocRef, ids: &[String]) {
        self.sync_doc(doc);
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(r) = group_rect(g.nodes.iter().filter(|n| ids.contains(&n.id))) else { return };
            let title = format!("Group {}", g.groups.len() + 1);
            g.groups.push(workflow::Group { title, bounding: r, color: Some("#3f789e".into()) });
        }
        self.edited(doc);
    }

    /// "Add Selected Nodes To Group": grow the group around its nodes plus the selection.
    fn add_to_group(&self, doc: &DocRef, gi: usize, ids: &[String]) {
        self.sync_doc(doc);
        let mut all = self.group_ids(doc, gi);
        all.extend(ids.iter().cloned());
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let Some(r) = group_rect(g.nodes.iter().filter(|n| all.contains(&n.id))) else { return };
            if let Some(grp) = g.groups.get_mut(gi) {
                grp.bounding = r;
            }
        }
        self.edited(doc);
    }

    fn fit_group_to_nodes(&self, doc: &DocRef, gi: usize) {
        self.add_to_group(doc, gi, &[]);
    }

    // -----------------------------------------------------------------------------------
    // Clipboard
    // -----------------------------------------------------------------------------------

    /// Ctrl+C: the selected nodes, or the selected group with its nodes.
    pub(super) fn copy(&self, doc: &DocRef) {
        self.sync_doc(doc);
        let gsel = doc.borrow().canvas.selected_group();
        let ids = match gsel {
            Some(gi) => self.group_ids(doc, gi),
            None => doc.borrow().canvas.selected(),
        };
        let d = doc.borrow();
        let g = graph_ref(&d);
        let clip = Clip {
            nodes: g.nodes.iter().filter(|n| ids.contains(&n.id)).cloned().collect(),
            links: g.links.values().filter(|l| ids.contains(&l.origin_id) && ids.contains(&l.target_id)).cloned().collect(),
            groups: gsel.and_then(|gi| g.groups.get(gi)).cloned().into_iter().collect(),
        };
        if !clip.nodes.is_empty() || !clip.groups.is_empty() {
            CLIP.set(Some(clip));
        }
    }

    /// Ctrl+V: paste at `at` (default: the pointer) with fresh ids, inner wires kept.
    pub(super) fn paste(&self, doc: &DocRef, at: Option<(f64, f64)>) {
        let Some(clip) = CLIP.with_borrow(Clone::clone) else { return };
        self.sync_doc(doc);
        let at = at.unwrap_or_else(|| doc.borrow().canvas.pointer());
        let fresh = paste_into(graph_mut(&mut doc.borrow_mut()), &clip, at);
        self.refresh_doc(doc);
        doc.borrow().canvas.select(&fresh);
        self.set_dirty(doc, true);
    }

    fn duplicate_group(&self, doc: &DocRef, gi: usize) {
        let saved = CLIP.with_borrow(Clone::clone);
        doc.borrow().canvas.select_group(Some(gi));
        self.copy(doc);
        let at = graph_ref(&doc.borrow()).groups.get(gi).map(|g| (g.bounding[0] + 30.0, g.bounding[1] + 30.0));
        self.paste(doc, at);
        CLIP.set(saved);
    }

    // -----------------------------------------------------------------------------------
    // Group toolbox (`reference/Group Select.png`)
    // -----------------------------------------------------------------------------------

    pub(super) fn group_toolbar(&self, doc: &DocRef) -> gtk::Widget {
        let canvas = doc.borrow().canvas.clone();
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bar.add_css_class("osd");
        bar.add_css_class("selection-toolbar");
        bar.set_halign(gtk::Align::Start);
        bar.set_valign(gtk::Align::Start);
        bar.set_visible(false);
        let icon = |name: &str, tip: &str| gtk::Button::builder().icon_name(name).tooltip_text(tip).css_classes(["flat"]).build();
        let cur = move |d: &DocRef| d.borrow().canvas.selected_group();

        let del = icon("cg-user-trash-symbolic", "Delete Group");
        del.connect_clicked({ let (me, doc) = (self.clone(), doc.clone()); move |_| if let Some(gi) = cur(&doc) { me.delete_group(&doc, gi) } });
        bar.append(&del);

        let chip = gtk::DrawingArea::new();
        chip.set_content_width(16);
        chip.set_content_height(16);
        let shown = Rc::new(Cell::new(u32::MAX));
        {
            let shown = shown.clone();
            chip.set_draw_func(move |_, cr, w, h| {
                let c = shown.get();
                cr.arc(w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0 - 1.5, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(((c >> 16) & 0xff) as f64 / 255.0, ((c >> 8) & 0xff) as f64 / 255.0, (c & 0xff) as f64 / 255.0);
                let _ = cr.fill();
            });
        }
        let color = gtk::Button::builder().child(&chip).tooltip_text("Group Color").css_classes(["flat"]).build();
        color.connect_clicked({ let (me, doc) = (self.clone(), doc.clone()); move |b| {
            let Some(gi) = cur(&doc) else { return };
            let Entry::Sub { items, .. } = me.group_color_sub(&doc, gi) else { return };
            me.show_menu(b, b.width() as f64 / 2.0, b.height() as f64, items());
        }});
        bar.append(&color);

        let conv = icon("cg-node-graph-symbolic", "Convert to Subgraph");
        conv.connect_clicked({ let (me, doc) = (self.clone(), doc.clone()); move |_| {
            let Some(gi) = cur(&doc) else { return };
            let ids = me.group_ids(&doc, gi);
            me.convert(&doc, &ids, Some(gi));
        }});
        bar.append(&conv);

        let more = icon("cg-view-more-symbolic", "More Options");
        more.connect_clicked({ let (me, doc) = (self.clone(), doc.clone()); move |b| {
            let Some(gi) = cur(&doc) else { return };
            let entries = me.group_menu(&doc, gi);
            me.show_menu(b, b.width() as f64 / 2.0, b.height() as f64, entries);
        }});
        bar.append(&more);

        {
            let (cv, bar2, chip2) = (canvas.clone(), bar.clone(), chip.clone());
            canvas.area.add_tick_callback(move |_, _| {
                let Some((x, y, w, c)) = cv.group_box() else {
                    if bar2.is_visible() {
                        bar2.set_visible(false);
                    }
                    return glib::ControlFlow::Continue;
                };
                let c = c.unwrap_or(0x3f789e);
                if c != shown.get() {
                    shown.set(c);
                    chip2.queue_draw();
                }
                let (mx, my) = (
                    (x + w / 2.0 - bar2.width() as f64 / 2.0).round().max(4.0) as i32,
                    (y - 12.0 - bar2.height() as f64).round().max(4.0) as i32,
                );
                if bar2.margin_start() != mx {
                    bar2.set_margin_start(mx);
                }
                if bar2.margin_top() != my {
                    bar2.set_margin_top(my);
                }
                if !bar2.is_visible() {
                    bar2.set_visible(true);
                }
                glib::ControlFlow::Continue
            });
        }
        bar.upcast()
    }

    /// The toolbox ⋮ menu (frontend `useGroupMenuOptions`).
    fn group_menu(&self, doc: &DocRef, gi: usize) -> Vec<Entry> {
        let ids = self.group_ids(doc, gi);
        let mut e = vec![
            item_i("", "Rename", true, self.act(doc, move |w, d| w.rename_group_dialog(d, gi))),
            item_i("", "Copy\tCtrl+C", true, self.act(doc, move |w, d| {
                d.borrow().canvas.select_group(Some(gi));
                w.copy(d);
            })),
            item_i("", "Duplicate\tCtrl+D", true, self.act(doc, move |w, d| w.duplicate_group(d, gi))),
        ];
        let s = ids.clone();
        e.push(item_i("cg-node-graph-symbolic", "Convert to Subgraph", !ids.is_empty(), self.act(doc, move |w, d| w.convert(d, &s, Some(gi)))));
        if let Entry::Sub { label, items, .. } = self.group_color_sub(doc, gi) {
            e.push(Entry::Sub { label, icon: Some("cg-applications-graphics-symbolic"), enabled: true, items });
        }
        if !ids.is_empty() {
            e.push(Entry::Header("Extensions".into()));
            e.extend(self.group_mode_entries(doc, &ids));
            e.push(item_i("cg-zoom-fit-best-symbolic", "Fit Group To Nodes", true, self.act(doc, move |w, d| w.fit_group_to_nodes(d, gi))));
        }
        e
    }
}

/// One menu page: rows for `entries`, with a back row when it is a submenu (`back` = its
/// title). Submenus slide in as further stack pages and are dropped again on the way back.
fn menu_page(pop: &gtk::Popover, stack: &gtk::Stack, back: Option<String>, entries: Vec<Entry>) -> gtk::Widget {
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for m in [6] {
        bx.set_margin_top(m);
        bx.set_margin_bottom(m);
        bx.set_margin_start(m);
        bx.set_margin_end(m);
    }
    let row = |icon: Option<&str>, label: &str, end: Option<gtk::Widget>| {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        r.set_margin_start(6);
        r.set_margin_end(6);
        if let Some(i) = icon {
            let im = gtk::Image::from_icon_name(i);
            im.set_pixel_size(16);
            im.set_margin_end(10);
            r.append(&im);
        }
        let (text, accel) = label.split_once('\t').map_or((label, None), |(a, b)| (a, Some(b)));
        let lb = gtk::Label::builder().label(text).xalign(0.0).hexpand(true).build();
        r.append(&lb);
        if let Some(a) = accel {
            let al = gtk::Label::builder().label(a).margin_start(24).css_classes(["dim-label", "caption"]).build();
            r.append(&al);
        }
        if let Some(w) = end {
            r.append(&w);
        }
        (r, lb)
    };
    let button = |child: &gtk::Box, enabled: bool| {
        let b = gtk::Button::builder().child(child).sensitive(enabled).css_classes(["flat", "cg-menu-row"]).build();
        b.set_halign(gtk::Align::Fill);
        b
    };
    if let Some(title) = back {
        let (r, lb) = row(Some("cg-go-previous-symbolic"), &title, None);
        lb.add_css_class("heading");
        let b = button(&r, true);
        let st = stack.downgrade();
        b.connect_clicked(move |_| {
            let Some(st) = st.upgrade() else { return };
            let Some(cur) = st.visible_child() else { return };
            // The page below this one is the one we came from.
            let pages: Vec<gtk::Widget> = std::iter::successors(st.first_child(), |w| w.next_sibling()).collect();
            let Some(at) = pages.iter().position(|p| *p == cur).filter(|&i| i > 0) else { return };
            st.set_transition_type(gtk::StackTransitionType::SlideRight);
            st.set_visible_child(&pages[at - 1]);
            let st2 = st.clone();
            glib::timeout_add_local_once(Duration::from_millis(250), move || st2.remove(&cur));
        });
        bx.append(&b);
        bx.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
    // Reserve the icon column only when some row on the page has an icon.
    let iconned = entries.iter().any(|e| matches!(e, Entry::Item { icon: Some(i), .. } | Entry::Sub { icon: Some(i), .. } if !i.is_empty()));
    let col = |i: Option<&'static str>| match i {
        Some(i) if iconned => Some(i),
        _ if iconned => Some(""),
        _ => None,
    };
    for e in entries {
        match e {
            Entry::Item { label, danger, enabled, icon, f } => {
                let (r, lb) = row(col(icon), &label, None);
                if danger {
                    lb.add_css_class("error");
                }
                let b = button(&r, enabled);
                let p = pop.downgrade();
                b.connect_clicked(move |_| {
                    if let Some(p) = p.upgrade() {
                        p.popdown();
                    }
                    f();
                });
                bx.append(&b);
            }
            Entry::Sub { label, icon, enabled, items } => {
                let chev = gtk::Image::from_icon_name("cg-lucide-chevron-right-symbolic");
                chev.set_margin_start(16);
                let (r, _) = row(col(icon), &label, Some(chev.upcast()));
                let b = button(&r, enabled);
                let (p, st) = (pop.downgrade(), stack.downgrade());
                b.connect_clicked(move |_| {
                    let (Some(p), Some(st)) = (p.upgrade(), st.upgrade()) else { return };
                    let page = menu_page(&p, &st, Some(label.clone()), items());
                    st.add_child(&page);
                    st.set_transition_type(gtk::StackTransitionType::SlideLeft);
                    st.set_visible_child(&page);
                });
                bx.append(&b);
            }
            Entry::Header(t) => {
                let lb = gtk::Label::builder().label(t).xalign(0.0).margin_start(12).margin_top(6).margin_bottom(2).css_classes(["dim-label", "caption"]).build();
                bx.append(&lb);
            }
            Entry::Line => {
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                sep.set_margin_top(3);
                sep.set_margin_bottom(3);
                bx.append(&sep);
            }
            Entry::Swatches(list) => {
                let flow = gtk::FlowBox::builder().max_children_per_line(5).min_children_per_line(5).selection_mode(gtk::SelectionMode::None).margin_start(6).margin_end(6).margin_top(4).margin_bottom(4).build();
                for (hex, f) in list {
                    let da = gtk::DrawingArea::new();
                    da.set_content_width(20);
                    da.set_content_height(20);
                    da.set_draw_func(move |_, cr, w, h| {
                        cr.set_source_rgb(((hex >> 16) & 0xff) as f64 / 255.0, ((hex >> 8) & 0xff) as f64 / 255.0, (hex & 0xff) as f64 / 255.0);
                        cr.arc(w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0, 0.0, std::f64::consts::TAU);
                        let _ = cr.fill();
                    });
                    let b = gtk::Button::builder().child(&da).css_classes(["flat", "circular"]).tooltip_text(format!("#{hex:06x}")).build();
                    let p = pop.downgrade();
                    b.connect_clicked(move |_| {
                        if let Some(p) = p.upgrade() {
                            p.popdown();
                        }
                        f();
                    });
                    flow.append(&b);
                }
                bx.append(&flow);
            }
        }
    }
    gtk::ScrolledWindow::builder()
        .child(&bx)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .propagate_natural_width(true)
        .max_content_height(620)
        .build()
        .upcast()
}

/// Insert `clip` into `g` with its top-left at `at`; returns the new node ids.
fn paste_into(g: &mut workflow::Graph, clip: &Clip, at: (f64, f64)) -> Vec<String> {
    let mut b = (f64::MAX, f64::MAX);
    for n in &clip.nodes {
        b = (b.0.min(n.pos.0), b.1.min(n.pos.1));
    }
    for gr in &clip.groups {
        b = (b.0.min(gr.bounding[0]), b.1.min(gr.bounding[1]));
    }
    if b.0 == f64::MAX {
        return Vec::new();
    }
    let (dx, dy) = (at.0 - b.0, at.1 - b.1);
    let mut map: HashMap<String, String> = HashMap::new();
    let mut fresh = Vec::new();
    for n in &clip.nodes {
        let mut n = n.clone();
        let id = g.next_id();
        map.insert(n.id.clone(), id.clone());
        n.id = id.clone();
        n.pos = (n.pos.0 + dx, n.pos.1 + dy);
        n.inputs.iter_mut().for_each(|i| i.link = None);
        n.outputs.iter_mut().for_each(|o| o.links.clear());
        g.nodes.push(n);
        fresh.push(id);
    }
    let mut ls = clip.links.clone();
    ls.sort_by_key(|l| l.id);
    for l in ls {
        if let (Some(a), Some(b)) = (map.get(&l.origin_id), map.get(&l.target_id)) {
            wire(g, a, l.origin_slot, b, l.target_slot);
        }
    }
    for gr in &clip.groups {
        let mut gr = gr.clone();
        gr.bounding[0] += dx;
        gr.bounding[1] += dy;
        g.groups.push(gr);
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> workflow::Graph {
        let v = serde_json::json!({
            "nodes": [
                {"id": 1, "type": "A", "pos": [0, 0], "size": [100, 50], "outputs": [{"name": "o", "type": "X", "links": [1]}]},
                {"id": 2, "type": "B", "pos": [0, 300], "size": [100, 50], "inputs": [{"name": "i", "type": "X", "link": 1}]}
            ],
            "links": [[1, 1, 0, 2, 0, "X"]]
        });
        workflow::Workflow::from_json(v).unwrap().root
    }

    #[test]
    fn arrange_and_paste() {
        let mut g = graph();
        arrange(&mut g, false);
        let (a, b) = (g.node("1").unwrap().pos, g.node("2").unwrap().pos);
        assert!(b.0 > a.0, "consumer goes in the next column");
        let clip = Clip { nodes: g.nodes.clone(), links: g.links.values().cloned().collect(), groups: Vec::new() };
        let fresh = paste_into(&mut g, &clip, (1000.0, 1000.0));
        assert_eq!(fresh, ["3", "4"]);
        assert_eq!(g.links.len(), 2);
        assert!(g.links.values().any(|l| l.origin_id == "3" && l.target_id == "4"));
        assert_eq!(g.node("4").unwrap().inputs[0].link.is_some(), true);
    }

    #[test]
    fn set_get_names_are_unique() {
        let mut g = graph();
        g.nodes.push(kj_node("SetNode", "9".into(), "o", "X", (0.0, 0.0)));
        assert_eq!(unique_set_name(&g, "o"), "o_0");
        assert_eq!(unique_set_name(&g, "p"), "p");
    }
}
