// SPDX-License-Identifier: GPL-3.0-only
//! ComfyUI workflow JSON model (the file format saved by the web frontend).
//!
//! Root graphs are format 0.4 (tuple links `[id, origin, oslot, target, tslot, type]`);
//! `definitions.subgraphs[]` are format 1 (object links). Both are accepted everywhere.
//! A subgraph *instance* is a node whose `type` is the definition's uuid. Inside a
//! definition, links from node id -10 come from the subgraph inputs, links to -20 go
//! to its outputs. Schema: ComfyUI_frontend `workflowSchema.ts` (see
//! docs/research/frontend-1.60-reference.md §3).
//!
//! The parsed model is edited in place (positions, links); `raw` keeps the original JSON so
//! saving can round-trip unknown fields (extra, groups, reroutes, properties…).

use serde_json::Value;
use std::collections::HashMap;

/// Node ids are numbers in files, except nodes unpacked from subgraphs ("39:7"),
/// so they are kept as strings.
pub const SUBGRAPH_INPUT_ID: &str = "-10";
#[allow(dead_code)]
pub const SUBGRAPH_OUTPUT_ID: &str = "-20";

/// litegraph `LGraphEventMode`.
pub const MODE_NEVER: i64 = 2; // "mute"
pub const MODE_BYPASS: i64 = 4;

#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub ty: String,
    pub link: Option<i64>,
    /// Set when this input is (or shadows) a widget: `{"widget": {"name": …}}`.
    pub widget: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub ty: String,
    pub links: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub ty: String,
    pub title: Option<String>,
    pub pos: (f64, f64),
    pub size: (f64, f64),
    pub mode: i64,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    /// Array (usual) or object (e.g. VideoHelperSuite) of widget values.
    pub widgets_values: Value,
    /// Raw `properties` (rgthree matchTitle, proxyWidgets…), read-only.
    pub properties: Value,
    pub color: Option<String>,
    pub bgcolor: Option<String>,
    pub collapsed: bool,
    /// `flags.pinned`: the node cannot be dragged or resized.
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub id: i64,
    pub origin_id: String,
    pub origin_slot: usize,
    pub target_id: String,
    pub target_slot: usize,
    pub ty: String,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub title: String,
    /// x, y, w, h
    pub bounding: [f64; 4],
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub links: HashMap<i64, Link>,
    pub groups: Vec<Group>,
}

impl Node {
    /// Fresh, unlinked node from a registry definition, laid out the way litegraph does:
    /// every input (widget-backed or not) becomes a slot, widget inputs shadowed by a widget,
    /// and `widgets_values` follows widget-input declaration order.
    pub fn from_def(def: &crate::registry::NodeDef, id: String, pos: (f64, f64)) -> Node {
        Node {
            id,
            ty: def.name.clone(),
            title: None,
            pos,
            size: (240.0, 100.0),
            mode: 0,
            inputs: def
                .inputs
                .iter()
                .map(|i| Input {
                    name: i.name.clone(),
                    ty: i.type_name().to_string(),
                    link: None,
                    widget: i.is_widget().then(|| i.name.clone()),
                })
                .collect(),
            outputs: def
                .outputs
                .iter()
                .map(|(ty, name)| Output { name: name.clone(), ty: ty.clone(), links: Vec::new() })
                .collect(),
            widgets_values: Value::Array(crate::prompt::default_widgets(def)),
            properties: Value::Null,
            color: None,
            bgcolor: None,
            collapsed: false,
            pinned: false,
        }
    }
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Next free node id (ComfyUI ids are numeric strings; max + 1 keeps them dense).
    pub fn next_id(&self) -> String {
        let max = self.nodes.iter().filter_map(|n| n.id.parse::<i64>().ok()).max().unwrap_or(0);
        (max + 1).to_string()
    }
}

/// A subgraph boundary slot.
#[derive(Debug, Clone)]
pub struct SubgraphIo {
    pub name: String,
    pub ty: String,
    pub link_ids: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct Subgraph {
    pub name: String,
    pub graph: Graph,
    pub inputs: Vec<SubgraphIo>,
    pub outputs: Vec<SubgraphIo>,
    /// Canvas rects [x, y, w, h] of the boundary input / output pseudo-nodes.
    pub input_node: Option<[f64; 4]>,
    pub output_node: Option<[f64; 4]>,
    /// Inputs this session added by dropped wires (never loaded from a file): they are
    /// removed again as soon as their last link is severed.
    pub auto: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Workflow {
    pub root: Graph,
    /// By uuid (= instance node `type`).
    pub subgraphs: HashMap<String, Subgraph>,
    pub raw: Value,
}

impl Workflow {
    pub fn from_json(v: Value) -> Result<Self, String> {
        let root = parse_graph(&v).ok_or("not a ComfyUI workflow (no \"nodes\" array)")?;
        let subgraphs = v["definitions"]["subgraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let id = s["id"].as_str()?.to_owned();
                Some((
                    id,
                    Subgraph {
                        name: s["name"].as_str().unwrap_or("Subgraph").to_owned(),
                        graph: parse_graph(s)?,
                        inputs: parse_io(&s["inputs"]),
                        outputs: parse_io(&s["outputs"]),
                        input_node: rect(&s["inputNode"]["bounding"]),
                        output_node: rect(&s["outputNode"]["bounding"]),
                        auto: Vec::new(),
                    },
                ))
            })
            .collect();
        Ok(Self { root, subgraphs, raw: v })
    }

    #[allow(dead_code)] // used by sidebar loading (next step)
    pub fn from_str(s: &str) -> Result<Self, String> {
        Self::from_json(serde_json::from_str(s).map_err(|e| e.to_string())?)
    }

    /// Original JSON with positions and wiring refreshed from the parsed model.
    /// Nodes the model does not track (group nodes, reroutes…) keep their saved values.
    pub fn to_json(&self) -> Value {
        let mut v = self.raw.clone();
        write_graph(&mut v, &self.root, false);
        // `v["definitions"]` would *insert* a null subtree, so look the definitions up
        // without indexing: a workflow without subgraphs must keep its key set.
        let defs = v.get_mut("definitions").and_then(|d| d.get_mut("subgraphs")).and_then(Value::as_array_mut);
        for d in defs.into_iter().flatten() {
            let Some(sg) = d.get("id").and_then(Value::as_str).and_then(|id| self.subgraphs.get(id)) else { continue };
            write_graph(d, &sg.graph, true);
            // Inputs are rebuilt: promotion adds/removes them and they carry an `id`.
            let mut stored = d["inputs"].as_array().cloned().unwrap_or_default();
            let mut inputs = Vec::with_capacity(sg.inputs.len());
            for src in &sg.inputs {
                let at = stored.iter().position(|iv| iv["name"].as_str() == Some(src.name.as_str()));
                let mut iv = at.map(|p| stored.remove(p)).unwrap_or_else(|| serde_json::json!({}));
                if iv.get("id").is_none() {
                    iv["id"] = Value::from(fresh_uuid());
                }
                iv["name"] = Value::from(src.name.as_str());
                iv["type"] = Value::from(src.ty.as_str());
                iv["linkIds"] = Value::from(src.link_ids.clone());
                if iv.get("pos").is_none() {
                    iv["pos"] = serde_json::json!([0, 0]);
                }
                inputs.push(iv);
            }
            if d.get("inputs").is_some() || !inputs.is_empty() {
                d["inputs"] = Value::Array(inputs);
            }
            let arr = d.get_mut("outputs").and_then(Value::as_array_mut);
            for (i, io) in arr.into_iter().flatten().enumerate() {
                if let Some(src) = sg.outputs.get(i) {
                    io["linkIds"] = Value::from(src.link_ids.clone());
                }
            }
        }
        v
    }

    /// Every graph of the workflow (root first) — promotion mirrors onto all instances.
    fn graphs_mut(&mut self) -> impl Iterator<Item = &mut Graph> {
        std::iter::once(&mut self.root).chain(self.subgraphs.values_mut().map(|s| &mut s.graph))
    }

    /// Expose inner widget `widget` of `node` in subgraph `sg` as a subgraph input (the
    /// frontend's "Show input"). The host input shadows the widget, so its value is kept
    /// in the instance's `widgets_values`; `seed` is the value the inner widget shows now.
    /// Mirrored onto every instance, because the tab path does not name one.
    pub fn promote(&mut self, sg: &str, node: &str, widget: &str, seed: Value) -> Result<String, String> {
        let Some(def) = self.subgraphs.get_mut(sg) else { return Err("unknown subgraph".into()) };
        let inner = def.graph.nodes.iter_mut().find(|n| n.id == node).ok_or("node is not in the subgraph")?;
        let slot = inner.inputs.iter().position(|i| i.widget.as_deref() == Some(widget)).ok_or("not a widget")?;
        if inner.inputs[slot].link.is_some() {
            return Err("input is already shown".into());
        }
        let name = unique_name(&def.inputs, widget);
        let ty = inner.inputs[slot].ty.clone();
        let id = def.graph.links.keys().max().copied().unwrap_or(0) + 1;
        let at = def.inputs.len();
        def.graph.links.insert(
            id,
            Link { id, origin_id: SUBGRAPH_INPUT_ID.into(), origin_slot: at, target_id: node.into(), target_slot: slot, ty: ty.clone() },
        );
        inner.inputs[slot].link = Some(id);
        def.inputs.push(SubgraphIo { name: name.clone(), ty: ty.clone(), link_ids: vec![id] });
        // The instance side: appending keeps `widgets_values` aligned with the widget inputs.
        let sg = sg.to_owned();
        for g in self.graphs_mut() {
            for n in g.nodes.iter_mut().filter(|n| n.ty == sg) {
                n.inputs.push(Input { name: name.clone(), ty: ty.clone(), link: None, widget: Some(widget.to_owned()) });
                if let Some(vals) = n.widgets_values.as_array_mut() {
                    vals.push(seed.clone());
                }
            }
        }
        Ok(name)
    }

    /// Remove subgraph input `at` (definition, interior link, and every instance socket).
    /// External wires into those instance sockets go with it, as in the frontend.
    pub fn remove_input(&mut self, sg: &str, at: usize) -> bool {
        let Some(def) = self.subgraphs.get_mut(sg) else { return false };
        if at >= def.inputs.len() {
            return false;
        }
        let io = def.inputs.remove(at);
        for id in &io.link_ids {
            if let Some(l) = def.graph.links.remove(id) {
                if let Some(n) = def.graph.nodes.iter_mut().find(|n| n.id == l.target_id) {
                    if let Some(slot) = n.inputs.get_mut(l.target_slot) {
                        slot.link = None;
                    }
                }
            }
        }
        // Remaining boundary links slide up with their slots.
        for l in def.graph.links.values_mut().filter(|l| l.origin_id == SUBGRAPH_INPUT_ID && l.origin_slot > at) {
            l.origin_slot -= 1;
        }
        def.auto.retain(|n| n != &io.name);
        let sg = sg.to_owned();
        for g in self.graphs_mut() {
            // Sockets of every instance, and the values their widget shadowed.
            let mut drift: Vec<(String, usize, Option<i64>)> = Vec::new();
            for n in g.nodes.iter_mut().filter(|n| n.ty == sg) {
                let Some(h) = n.inputs.iter().position(|i| i.name == io.name) else { continue };
                let shadowed = n.inputs[h].widget.is_some();
                let value_at = n.inputs[..h].iter().filter(|i| i.widget.is_some()).count();
                let link = n.inputs[h].link;
                n.inputs.remove(h);
                if shadowed {
                    if let Some(vals) = n.widgets_values.as_array_mut() {
                        if value_at < vals.len() {
                            vals.remove(value_at);
                        }
                    }
                }
                drift.push((n.id.clone(), h, link));
            }
            for (id, h, link) in drift {
                if let Some(l) = link {
                    g.links.remove(&l);
                }
                for l in g.links.values_mut().filter(|l| l.target_id == id) {
                    if l.target_slot == h {
                        // Only wires are left with that slot when `link` was null but the
                        // frontend had stale ids; they are dropped with the socket.
                        continue;
                    }
                    if l.target_slot > h {
                        l.target_slot -= 1;
                    }
                }
            }
        }
        true
    }
}

/// Splice `g` back into its raw node/link arrays. Subgraph definitions store links as
/// objects, the root graph as 0.4 tuples.
fn write_graph(s: &mut Value, g: &Graph, object_links: bool) {
    {
        // Reconcile the node list: drop nodes deleted in the editor, append new ones.
        // Nodes created in the app exist only in the model, not in the loaded `raw`.
        let known: std::collections::HashSet<String> = g.nodes.iter().map(|n| n.id.clone()).collect();
        let arr = s["nodes"].as_array_mut();
        if let Some(arr) = arr {
            arr.retain(|nv| id_of(&nv["id"]).is_some_and(|id| known.contains(&id)));
            let present: std::collections::HashSet<String> = arr.iter().filter_map(|nv| id_of(&nv["id"])).collect();
            for n in g.nodes.iter().filter(|n| !present.contains(&n.id)) {
                arr.push(fresh_node_json(n));
            }
        }
    }
    for nv in s["nodes"].as_array_mut().into_iter().flatten() {
        let Some(n) = id_of(&nv["id"]).and_then(|id| g.nodes.iter().find(|n| n.id == id)) else { continue };
        nv["pos"] = serde_json::json!([n.pos.0, n.pos.1]);
        // Only when it changed: untouched nodes keep the size the file already had.
        let same = nv["size"].as_array().is_some_and(|a| a.len() == 2 && a[0].as_f64() == Some(n.size.0) && a[1].as_f64() == Some(n.size.1));
        if !same {
            nv["size"] = serde_json::json!([n.size.0, n.size.1]);
        }
        // Node state the canvas can change. ComfyUI always serialises `mode`, folds
        // "collapsed" into `flags`, and only writes a title that differs from the type.
        nv["mode"] = Value::from(n.mode);
        if !n.widgets_values.is_null() {
            nv["widgets_values"] = n.widgets_values.clone(); // edited in the canvas
        }
        match &n.title {
            Some(t) if !t.is_empty() => nv["title"] = Value::from(t.as_str()),
            _ => {
                if let Some(o) = nv.as_object_mut() {
                    o.remove("title");
                }
            }
        }
        if n.collapsed {
            nv["flags"]["collapsed"] = Value::Bool(true);
        } else if nv["flags"]["collapsed"].as_bool() == Some(true) {
            nv["flags"]["collapsed"] = Value::Bool(false);
        }
        if n.pinned {
            nv["flags"]["pinned"] = Value::Bool(true);
        } else if nv["flags"]["pinned"].as_bool() == Some(true) {
            nv["flags"]["pinned"] = Value::Bool(false);
        }
        // Inputs are rebuilt (promotion adds/removes them, and a widget input must carry
        // `widget`), keeping whatever extras the stored entry had (label, shape, pos…).
        if nv.get("inputs").is_some() || !n.inputs.is_empty() {
            let mut stored = nv["inputs"].as_array().cloned().unwrap_or_default();
            let mut out = Vec::with_capacity(n.inputs.len());
            for src in &n.inputs {
                let at = stored.iter().position(|iv| iv["name"].as_str() == Some(src.name.as_str()));
                let mut iv = at.map(|p| stored.remove(p)).unwrap_or_else(|| serde_json::json!({}));
                iv["name"] = Value::from(src.name.as_str());
                iv["type"] = Value::from(src.ty.as_str());
                iv["link"] = src.link.map(Value::from).unwrap_or(Value::Null);
                match &src.widget {
                    Some(w) => iv["widget"] = serde_json::json!({ "name": w }),
                    None => {
                        if let Some(o) = iv.as_object_mut() {
                            o.remove("widget");
                        }
                    }
                }
                out.push(iv);
            }
            nv["inputs"] = Value::Array(out);
        }
        for (i, ov) in nv["outputs"].as_array_mut().into_iter().flatten().enumerate() {
            if let Some(src) = n.outputs.get(i) {
                // ComfyUI writes `null` for "nothing connected"; keep that instead of
                // rewriting it to `[]` (minimal diff on save).
                let was_null = ov.get("links").is_some_and(Value::is_null);
                ov["links"] = if src.links.is_empty() && was_null { Value::Null } else { Value::from(src.links.clone()) };
            }
        }
    }

    // Groups: rebuilt from the model (created/deleted/renamed/moved in the editor).
    let old_ids: Vec<Value> = s["groups"].as_array().map(|a| a.iter().map(|g| g["id"].clone()).collect()).unwrap_or_default();
    let mut next_id = old_ids.iter().filter_map(Value::as_i64).max().unwrap_or(0) + 1;
    let groups: Vec<Value> = g
        .groups
        .iter()
        .enumerate()
        .map(|(i, gr)| {
            let id = match old_ids.get(i) {
                Some(v) if v.is_i64() => v.clone(),
                _ => {
                    let v = Value::from(next_id);
                    next_id += 1;
                    v
                }
            };
            // Start from the stored group (keeps font_size, int-vs-float bounds, extras).
            let mut v = s["groups"].get(i).cloned().filter(|o| o.is_object()).unwrap_or_else(|| serde_json::json!({"font_size": 24, "flags": {}}));
            v["id"] = id;
            v["title"] = gr.title.clone().into();
            let same = v["bounding"].as_array().is_some_and(|b| b.len() == 4 && b.iter().zip(&gr.bounding).all(|(a, n)| a.as_f64() == Some(*n)));
            if !same {
                v["bounding"] = serde_json::json!(gr.bounding);
            }
            v["color"] = gr.color.clone().unwrap_or_else(|| "#3f789e".to_string()).into();
            v
        })
        .collect();
    s["groups"] = Value::Array(groups);

    // Node ids are stored as strings in the model; write them back as numbers when they
    // were numbers (plain ids), keeping "39:7" style ids from unpacked subgraphs.
    let idv = |id: &str| id.parse::<i64>().map_or_else(|_| Value::from(id), Value::from);
    let mut links: Vec<&Link> = g.links.values().collect();
    links.sort_by_key(|l| l.id);
    s["links"] = Value::Array(
        links
            .iter()
            .map(|l| {
                if object_links {
                    serde_json::json!({"id": l.id, "origin_id": idv(&l.origin_id), "origin_slot": l.origin_slot,
                                       "target_id": idv(&l.target_id), "target_slot": l.target_slot, "type": l.ty})
                } else {
                    serde_json::json!([l.id, idv(&l.origin_id), l.origin_slot, idv(&l.target_id), l.target_slot, l.ty])
                }
            })
            .collect(),
    );
}

/// Serialise a node that has no counterpart in the loaded JSON (created in the editor).
fn fresh_node_json(n: &Node) -> Value {
    let idv = |id: &str| id.parse::<i64>().map_or_else(|_| Value::from(id), Value::from);
    serde_json::json!({
        "id": idv(&n.id),
        "type": n.ty,
        "pos": [n.pos.0, n.pos.1],
        "size": [n.size.0, n.size.1],
        "flags": {},
        "order": 0,
        "mode": n.mode,
        "inputs": n.inputs.iter().map(|i| {
            let mut iv = serde_json::json!({
                "name": i.name,
                "type": i.ty,
                "link": i.link.map(Value::from).unwrap_or(Value::Null),
            });
            // Widget-backed inputs are shadowed by their widget in the frontend.
            if let Some(w) = &i.widget {
                iv["widget"] = serde_json::json!({"name": w});
            }
            iv
        }).collect::<Vec<_>>(),
        "outputs": n.outputs.iter().enumerate().map(|(i, o)| serde_json::json!({
            "name": o.name,
            "type": o.ty,
            "slot_index": i,
            "links": if o.links.is_empty() { Value::Null } else { Value::from(o.links.clone()) },
        })).collect::<Vec<_>>(),
        // ComfyUI records the class type here; some nodes rely on it for "Node name for S&R".
        "properties": {"Node name for S&R": n.ty},
        "widgets_values": n.widgets_values,
    })
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(), // e.g. legacy combo output type given as a list
    }
}

fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// `name`, `name 2`, `name 3`… — the frontend's `nextUniqueName` for promoted inputs.
fn unique_name(existing: &[SubgraphIo], name: &str) -> String {
    let taken = |n: &str| existing.iter().any(|i| i.name == n);
    if !taken(name) {
        return name.to_owned();
    }
    (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
}

/// v4-shaped id for a subgraph input created here (the frontend keys inputs by uuid).
fn fresh_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let n = t.rotate_left(19) ^ N.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!("{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", (n >> 32) as u32, (n >> 16) as u16, n as u16 & 0xfff, (n >> 44) as u16 & 0xfff, n & 0xffff_ffff_ffff)
}

fn rect(v: &Value) -> Option<[f64; 4]> {
    let b = v.as_array()?;
    let f = |i: usize| b.get(i).and_then(Value::as_f64).unwrap_or(0.0);
    Some([f(0), f(1), f(2), f(3)])
}

fn pair(v: &Value) -> Option<(f64, f64)> {
    // [x, y] or {"0": x, "1": y} (very old files)
    Some((v[0].as_f64().or_else(|| v["0"].as_f64())?, v[1].as_f64().or_else(|| v["1"].as_f64())?))
}

fn parse_graph(v: &Value) -> Option<Graph> {
    let nodes = v["nodes"].as_array()?.iter().filter_map(parse_node).collect();
    let links = v["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_link)
        .map(|l| (l.id, l))
        .collect();
    let groups = v["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            Some(Group {
                title: g["title"].as_str().unwrap_or_default().to_owned(),
                bounding: rect(&g["bounding"])?,
                color: g["color"].as_str().map(str::to_owned),
            })
        })
        .collect();
    Some(Graph { nodes, links, groups })
}

fn parse_node(n: &Value) -> Option<Node> {
    let inputs = n["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| Input {
            name: str_of(&i["name"]),
            ty: str_of(&i["type"]),
            link: i["link"].as_i64(),
            widget: i["widget"]["name"].as_str().map(str::to_owned),
        })
        .collect();
    let outputs = n["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| Output {
            name: str_of(&o["name"]),
            ty: str_of(&o["type"]),
            links: o["links"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect(),
        })
        .collect();
    Some(Node {
        id: id_of(&n["id"])?,
        ty: str_of(&n["type"]),
        title: n["title"].as_str().map(str::to_owned),
        pos: pair(&n["pos"]).unwrap_or_default(),
        size: pair(&n["size"]).unwrap_or((200.0, 100.0)),
        mode: n["mode"].as_i64().unwrap_or(0),
        inputs,
        outputs,
        widgets_values: n["widgets_values"].clone(),
        properties: n["properties"].clone(),
        color: n["color"].as_str().map(str::to_owned),
        bgcolor: n["bgcolor"].as_str().map(str::to_owned),
        collapsed: n["flags"]["collapsed"].as_bool().unwrap_or(false),
        pinned: n["flags"]["pinned"].as_bool().unwrap_or(false),
    })
}

fn parse_link(l: &Value) -> Option<Link> {
    if let Some(a) = l.as_array() {
        // [id, origin_id, origin_slot, target_id, target_slot, type]
        return Some(Link {
            id: a.first()?.as_i64()?,
            origin_id: id_of(a.get(1)?)?,
            origin_slot: a.get(2)?.as_u64()? as usize,
            target_id: id_of(a.get(3)?)?,
            target_slot: a.get(4)?.as_u64()? as usize,
            ty: a.get(5).map(str_of).unwrap_or_default(),
        });
    }
    Some(Link {
        id: l["id"].as_i64()?,
        origin_id: id_of(&l["origin_id"])?,
        origin_slot: l["origin_slot"].as_u64()? as usize,
        target_id: id_of(&l["target_id"])?,
        target_slot: l["target_slot"].as_u64()? as usize,
        ty: str_of(&l["type"]),
    })
}

fn parse_io(v: &Value) -> Vec<SubgraphIo> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|io| SubgraphIo {
            name: str_of(&io["name"]),
            ty: str_of(&io["type"]),
            link_ids: io["linkIds"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nodes and groups created in the editor exist only in the model, and nodes/groups
    /// deleted there still exist in the loaded `raw`. `write_graph` has to reconcile both
    /// directions — that bug shipped once and silently dropped every new node on save.
    #[test]
    fn to_json_reconciles_nodes_and_groups() {
        let src = r##"{"version":0.4,"nodes":[
            {"id":1,"type":"A","pos":[0,0],"size":[100,50],"inputs":[],"outputs":[]},
            {"id":2,"type":"B","pos":[10,10],"size":[100,50],"inputs":[],"outputs":[]}],
            "links":[],"groups":[{"id":5,"title":"Old","bounding":[0,0,10,10],"color":"#3f789e"}]}"##;
        let mut wf = Workflow::from_str(src).expect("parse");
        // Delete node 1, keep 2 where it is, add a brand-new node 3.
        wf.root.nodes.retain(|n| n.id != "1");
        let mut fresh = wf.root.nodes[0].clone();
        fresh.id = "3".into();
        fresh.ty = "C".into();
        fresh.pos = (5.0, 6.0);
        fresh.widgets_values = serde_json::json!([1, "x"]);
        wf.root.nodes.push(fresh);
        // A renamed old group plus a new one.
        wf.root.groups[0].title = "Renamed".into();
        wf.root.groups.push(fresh_group("Fresh", [1.0, 2.0, 3.0, 4.0]));

        let out = wf.to_json();
        let ids: Vec<&str> = out["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .map(|n| n["id"].as_str().unwrap_or("?"))
            .collect();
        assert_eq!(ids.len(), 2, "deleted node still in file: {out}");
        assert!(!ids.contains(&"1"), "node 1 came back");
        let added = out["nodes"].as_array().unwrap().iter().find(|n| n["id"] == 3).expect("node 3 written");
        assert_eq!(added["type"], "C");
        assert_eq!(added["pos"], serde_json::json!([5.0, 6.0]));
        assert_eq!(added["widgets_values"], serde_json::json!([1, "x"]));
        assert_eq!(added["properties"]["Node name for S&R"], "C");

        let groups = out["groups"].as_array().expect("groups");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["title"], "Renamed");
        assert_eq!(groups[0]["id"], 5, "existing group id preserved");
        assert_eq!(groups[1]["title"], "Fresh");
        assert_eq!(groups[1]["bounding"], serde_json::json!([1.0, 2.0, 3.0, 4.0]));
    }

    /// `promote` writes both halves (definition input + interior link + host socket/value)
    /// and the file has to survive a save/reload; `remove_input` has to undo all of it.
    #[test]
    fn promote_and_demote_round_trip() {
        let src = r##"{"version":0.4,"nodes":[{"id":1,"type":"sg-1","pos":[0,0],"size":[200,100],
            "inputs":[{"name":"a","type":"INT","widget":{"name":"a"},"link":null}],
            "outputs":[],"widgets_values":[7]}],
            "links":[],
            "definitions":{"subgraphs":[{"id":"sg-1","name":"Sub","inputNode":{"bounding":[0,0,10,10]},
            "outputNode":{"bounding":[0,20,10,10]},"inputs":[],"outputs":[],
            "nodes":[{"id":5,"type":"Inner","pos":[0,0],"size":[100,50],
                "inputs":[{"name":"a","type":"INT","widget":{"name":"a"},"link":null},
                          {"name":"b","type":"INT","widget":{"name":"b"},"link":null}],
                "outputs":[],"widgets_values":[7,3]}],"links":[]}]}}"##;
        let mut wf = Workflow::from_str(src).expect("parse");

        let name = wf.promote("sg-1", "5", "b", serde_json::json!(3)).expect("promote");
        assert_eq!(name, "b");
        assert_eq!(wf.subgraphs["sg-1"].inputs.len(), 1);
        assert_eq!(wf.subgraphs["sg-1"].graph.nodes[0].inputs[1].link, Some(1));
        assert_eq!(wf.root.nodes[0].inputs[1].widget.as_deref(), Some("b"));
        assert_eq!(wf.root.nodes[0].widgets_values, serde_json::json!([7, 3]));

        // Saving and reloading keeps the promoted socket, its widget shadow and the link.
        let out = wf.to_json();
        let mut again = Workflow::from_json(out.clone()).expect("reload");
        assert_eq!(again.subgraphs["sg-1"].inputs[0].name, "b");
        assert_eq!(again.subgraphs["sg-1"].inputs[0].link_ids, vec![1]);
        assert_eq!(again.root.nodes[0].inputs[1].widget.as_deref(), Some("b"));
        let def = &out["definitions"]["subgraphs"][0];
        assert_eq!(def["inputs"][0]["linkIds"], serde_json::json!([1]));
        assert!(def["nodes"][0]["inputs"][1]["widget"]["name"] == "b");

        // Demoting the promoted input drops its socket, value and interior link.
        assert!(again.remove_input("sg-1", 0));
        assert!(again.subgraphs["sg-1"].inputs.is_empty());
        assert_eq!(again.subgraphs["sg-1"].graph.nodes[0].inputs[1].link, None);
        assert_eq!(again.root.nodes[0].inputs.len(), 1);
        assert_eq!(again.root.nodes[0].inputs[0].name, "a");
        assert_eq!(again.root.nodes[0].widgets_values, serde_json::json!([7]));
    }

    fn fresh_group(title: &str, rect: [f64; 4]) -> Group {
        Group { title: title.into(), bounding: rect, color: None }
    }

    /// Saving must not perturb coordinates: drag one node, write the file back, and
    /// the untouched nodes must keep their exact `f64`s (serde_json needs
    /// `float_roundtrip` for this — its default parser is off by an ulp on long
    /// mantissas, which silently rewrote every node position on save).
    #[test]
    fn to_json_keeps_coordinates_bit_exact() {
        let src = r##"{"version":0.4,"nodes":[
            {"id":6,"type":"X","pos":[950.2978891664006,104.62510134174657],
             "size":[315,98],"inputs":[],"outputs":[]},
            {"id":7,"type":"Y","pos":[-44.260121259699716,776.6053367103586],
             "size":[315,98],"inputs":[],"outputs":[]}],
            "links":[],"groups":[]}"##;
        let out = Workflow::from_str(src).expect("parse").to_json().to_string();
        for want in ["104.62510134174657", "-44.260121259699716", "776.6053367103586", "950.2978891664006"] {
            assert!(out.contains(want), "{want} lost in round trip:\n{out}");
        }
    }
}
