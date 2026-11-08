// SPDX-License-Identifier: GPL-3.0-only
//! Workflow → API prompt (`POST /prompt` body `prompt`), including subgraph flattening.
//!
//! Re-implements ComfyUI_frontend `executionUtil.ts::graphToPrompt` and
//! `ExecutableNodeDTO.resolveInput/resolveOutput` (see docs/research/frontend-1.60-reference.md §4):
//! - execution id = subgraph host path + node id joined with ':' ("12:5");
//! - muted (2) / bypassed (4) nodes are not emitted; links out of muted nodes are dropped,
//!   links out of bypassed nodes pass through a type-matching input;
//! - links into a subgraph (-10) continue at the host's same-named input, links out of
//!   a subgraph instance continue at the inner node linked to the -20 output;
//! - frontend-only ("virtual") nodes are not emitted: Reroutes pass through,
//!   PrimitiveNode supplies its value, everything else unknown to `/object_info` is dropped;
//! - widget values map positionally onto the node definition's widget inputs.

use crate::registry::{Input, Kind, NodeDef, Registry};
use crate::workflow::{Graph, Link, Node, Subgraph, Workflow, MODE_BYPASS, MODE_NEVER, SUBGRAPH_INPUT_ID};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const MAX_DEPTH: usize = 64;

enum Resolved {
    Link(String, usize),
    Value(Value),
}

struct Ctx<'a> {
    graph: &'a Graph,
    /// Execution-id prefix: "" at root, "12:" inside host 12, "12:7:" nested.
    prefix: String,
    /// (parent ctx, host node, definition)
    parent: Option<(usize, &'a Node, &'a Subgraph)>,
}

struct Flattener<'a> {
    wf: &'a Workflow,
    reg: &'a Registry,
    ctxs: Vec<Ctx<'a>>,
    /// (ctx, host node id) → ctx of its expanded subgraph
    children: HashMap<(usize, &'a str), usize>,
    warnings: Vec<String>,
}

/// Returns the API prompt plus human-readable warnings (unknown node types, recursion…).
pub fn build(wf: &Workflow, reg: &Registry) -> (Value, Vec<String>) {
    let mut f = Flattener { wf, reg, ctxs: Vec::new(), children: HashMap::new(), warnings: Vec::new() };
    f.ctxs.push(Ctx { graph: &wf.root, prefix: String::new(), parent: None });
    f.expand(0, 0);

    let mut out = Map::new();
    for c in 0..f.ctxs.len() {
        for node in &f.ctxs[c].graph.nodes {
            if node.mode == MODE_NEVER || node.mode == MODE_BYPASS || f.children.contains_key(&(c, node.id.as_str())) {
                continue;
            }
            let Some(def) = reg.get(&node.ty) else {
                if !wf.subgraphs.contains_key(&node.ty) && !is_known_virtual(&node.ty) {
                    f.warnings.push(format!("skipping node {} \"{}\": not provided by the backend", node.id, node.ty));
                }
                continue;
            };
            let id = format!("{}{}", f.ctxs[c].prefix, node.id);
            let inputs = f.node_inputs(c, node, def);
            let title = node.title.clone().unwrap_or_else(|| def.display_name.clone());
            out.insert(id, json!({ "inputs": inputs, "class_type": node.ty, "_meta": { "title": title } }));
        }
    }
    // Drop links to nodes that were not emitted (frontend does the same).
    let ids: std::collections::HashSet<String> = out.keys().cloned().collect();
    for n in out.values_mut() {
        if let Some(inputs) = n["inputs"].as_object_mut() {
            inputs.retain(|_, v| match link_target(v) {
                Some(id) => ids.contains(id),
                None => true,
            });
        }
    }
    (Value::Object(out), f.warnings)
}

fn link_target(v: &Value) -> Option<&str> {
    let a = v.as_array()?;
    (a.len() == 2 && a[1].is_u64()).then_some(())?;
    a[0].as_str()
}

/// Widget name → value for a node, as the frontend would serialize it.
pub fn widget_map(node: &Node, def: &NodeDef) -> Map<String, Value> {
    if let Some(m) = extension_widgets(node, def) {
        return m;
    }
    let mut m = Map::new();
    match &node.widgets_values {
        Value::Array(vals) => {
            // A socket-typed input with no slot on the node is drawn as a widget by some
            // extension (e.g. RMBG's COLORCODE), so it owns a widgets_values entry.
            let inputs: Vec<Input> = def
                .inputs
                .iter()
                .map(|i| match &i.kind {
                    Kind::Socket(t) if !t.ends_with("_V3") && !node.inputs.iter().any(|n| n.name == i.name) => {
                        Input { kind: Kind::Custom(t.clone()), ..i.clone() }
                    }
                    _ => i.clone(),
                })
                .collect();
            map_widgets(&inputs, &mut vals.iter(), "", &mut m)
        }
        Value::Object(o) => {
            for i in def.inputs.iter().filter(|i| i.is_widget()) {
                if let Some(v) = o.get(&i.name) {
                    m.insert(i.name.clone(), wrap(v));
                }
            }
        }
        _ => {}
    }
    m
}

/// Nodes whose JS adds widgets the backend schema doesn't declare.
fn extension_widgets(node: &Node, def: &NodeDef) -> Option<Map<String, Value>> {
    let vals = node.widgets_values.as_array()?;
    let mut m = Map::new();
    if node.ty == "Power Lora Loader (rgthree)" {
        // [{} divider, header, {on,lora,strength,strengthTwo}…, {} divider, "" add-button]
        let mut n = 0;
        for v in vals {
            match v {
                Value::Object(o) if o.contains_key("lora") => {
                    n += 1;
                    let mut o = o.clone();
                    o.retain(|_, v| !v.is_null()); // JS `undefined` strengthTwo
                    m.insert(format!("lora_{n}"), Value::Object(o));
                }
                Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("PowerLoraLoaderHeaderWidget") => {
                    m.insert("PowerLoraLoaderHeaderWidget".into(), v.clone());
                }
                Value::String(_) => {
                    m.insert("\u{2795} Add Lora".into(), v.clone());
                }
                _ => {}
            }
        }
        return Some(m);
    }
    if node.ty.ends_with("(LoraManager)") {
        // LoRA Manager prepends an autocomplete-metadata widget and appends `loras`.
        let meta = vals.first()?.get("textWidgetName")?.as_str()?;
        m.insert(format!("__lm_autocomplete_meta_{meta}"), vals[0].clone());
        let mut rest = vals[1..].iter();
        map_widgets(&def.inputs, &mut rest, "", &mut m);
        if let Some(l) = rest.next() {
            m.insert("loras".into(), wrap(l));
        }
        return Some(m);
    }
    None
}

fn is_known_virtual(ty: &str) -> bool {
    matches!(ty, "Note" | "MarkdownNote" | "PrimitiveNode" | "Reroute" | "GetNode" | "SetNode") || ty.contains("(rgthree)") || ty.starts_with("LC")
}

/// Frontend serializes array widget values as `{"__value__": [...]}` so the backend
/// doesn't mistake them for links.
fn wrap(v: &Value) -> Value {
    if v.is_array() { json!({ "__value__": v }) } else { v.clone() }
}

/// UI-only `widgets_values` slots (not sent to the backend) that follow widget `i`:
/// (control_after_generate combo, upload button).
fn extra_kinds(i: &Input) -> (bool, bool) {
    let cag = &i.opts["control_after_generate"];
    let control = match i.kind {
        // useIntWidget: `control_after_generate ?? ['seed','noise_seed'].includes(name)`
        Kind::Int => match cag {
            Value::Null => matches!(i.name.as_str(), "seed" | "noise_seed"),
            v => truthy(v),
        },
        Kind::Combo(_) => truthy(cag),
        _ => false,
    };
    let upload = ["image_upload", "video_upload", "audio_upload"].iter().any(|k| truthy(&i.opts[k]));
    (control, upload)
}

fn extra_slots(i: &Input) -> usize {
    let (c, u) = extra_kinds(i);
    c as usize + u as usize
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Value,
    /// control_after_generate combo following `name`.
    Control,
    /// Upload button following `name`.
    Upload,
}

/// One positional `widgets_values` entry, resolved against the node definition.
pub struct WSlot {
    pub input: Input,
    /// Widget name (`parent.child` for dynamic-combo children).
    pub name: String,
    /// Index into `widgets_values`.
    pub idx: usize,
    pub value: Value,
    pub role: Role,
}

/// Mirrors `widget_map`'s array branch but keeps positions and the UI-only slots,
/// so the canvas can edit values in place.
pub fn widget_slots(node: &Node, def: &NodeDef) -> Vec<WSlot> {
    let Value::Array(vals) = &node.widgets_values else { return Vec::new() };
    let inputs: Vec<Input> = def
        .inputs
        .iter()
        .map(|i| match &i.kind {
            Kind::Socket(t) if !t.ends_with("_V3") && !node.inputs.iter().any(|n| n.name == i.name) => {
                Input { kind: Kind::Custom(t.clone()), ..i.clone() }
            }
            _ => i.clone(),
        })
        .collect();
    let (mut out, mut pos) = (Vec::new(), 0);
    collect_slots(&inputs, vals, &mut pos, "", &mut out);
    out
}

fn collect_slots(inputs: &[Input], vals: &[Value], pos: &mut usize, prefix: &str, out: &mut Vec<WSlot>) {
    for i in inputs.iter().filter(|i| i.is_widget()) {
        let idx = *pos;
        let Some(v) = vals.get(idx).or(i.opts.get("default")) else { continue };
        *pos += 1;
        let name = format!("{prefix}{}", i.name);
        let mk = |idx, value: &Value, role, name: &str| WSlot { input: i.clone(), name: name.to_owned(), idx, value: value.clone(), role };
        out.push(mk(idx, v, Role::Value, &name));
        if prefix.is_empty() {
            let (c, u) = extra_kinds(i);
            for role in [(c, Role::Control), (u, Role::Upload)].into_iter().filter(|r| r.0).map(|r| r.1) {
                out.push(mk(*pos, vals.get(*pos).unwrap_or(&Value::Null), role, &name));
                *pos += 1;
            }
        }
        if let Kind::DynCombo(opts) = &i.kind {
            if let Some((_, children)) = opts.iter().find(|o| Some(o.0.as_str()) == v.as_str()) {
                collect_slots(children, vals, pos, &format!("{name}."), out);
            }
        }
    }
}

/// `widgets_values` for a freshly created node: each widget's default followed by its
/// `control_after_generate` ("randomize") / upload slots, dynamic-combo children included.
pub fn default_widgets(def: &NodeDef) -> Vec<Value> {
    let mut out = Vec::new();
    push_defaults(&def.inputs, &mut out);
    out
}

fn push_defaults(inputs: &[Input], out: &mut Vec<Value>) {
    for i in inputs.iter().filter(|i| i.is_widget()) {
        if let Kind::DynCombo(opts) = &i.kind {
            if let Some((name, children)) = opts.first() {
                out.push(Value::from(name.clone()));
                push_defaults(children, out);
                continue;
            }
        }
        out.push(match &i.kind {
            Kind::Int => Value::from(i.opts["default"].as_i64().unwrap_or(0)),
            Kind::Float => Value::from(i.opts["default"].as_f64().unwrap_or(0.0)),
            Kind::String => Value::from(i.opts["default"].as_str().unwrap_or("")),
            Kind::Boolean => Value::from(i.opts["default"].as_bool().unwrap_or(false)),
            Kind::Combo(opts) => opts.first().map_or(Value::Null, |s| Value::from(s.clone())),
            _ => i.opts.get("default").cloned().unwrap_or(Value::Null),
        });
        for _ in 0..extra_slots(i) {
            out.push(Value::from("randomize"));
        }
    }
}

/// Map positional `widgets_values` onto widget names (`"parent.child"` for dynamic
/// combo children, which follow their combo).
pub fn map_widgets<'v>(inputs: &[Input], vals: &mut impl Iterator<Item = &'v Value>, prefix: &str, out: &mut Map<String, Value>) {
    for i in inputs.iter().filter(|i| i.is_widget()) {
        // Workflows saved before a widget existed are short: the frontend uses the default.
        let Some(v) = vals.next().or(i.opts.get("default")) else { continue };
        let name = format!("{prefix}{}", i.name);
        // Dynamic-combo children get no control_after_generate slot (golden data).
        if prefix.is_empty() {
            for _ in 0..extra_slots(i) {
                vals.next();
            }
        }
        if let Kind::DynCombo(opts) = &i.kind {
            if let Some((_, children)) = opts.iter().find(|o| Some(o.0.as_str()) == v.as_str()) {
                out.insert(name.clone(), wrap(v));
                map_widgets(children, vals, &format!("{name}."), out);
                continue;
            }
        }
        out.insert(name, wrap(v));
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    }
}

fn types_compatible(a: &str, b: &str) -> bool {
    a == b || a == "*" || b == "*" || a.is_empty() || b.is_empty() || a.split(',').any(|x| b.split(',').any(|y| x == y))
}

impl<'a> Flattener<'a> {
    fn expand(&mut self, ctx: usize, depth: usize) {
        let graph = self.ctxs[ctx].graph;
        for node in &graph.nodes {
            if node.mode == MODE_NEVER || node.mode == MODE_BYPASS {
                continue;
            }
            let Some(sg) = self.wf.subgraphs.get(&node.ty) else { continue };
            if depth >= 16 {
                self.warnings.push(format!("subgraph \"{}\" nested too deep (recursive?)", sg.name));
                continue;
            }
            let prefix = format!("{}{}:", self.ctxs[ctx].prefix, node.id);
            self.ctxs.push(Ctx { graph: &sg.graph, prefix, parent: Some((ctx, node, sg)) });
            let c = self.ctxs.len() - 1;
            self.children.insert((ctx, node.id.as_str()), c);
            self.expand(c, depth + 1);
        }
    }

    fn widget_values(&self, node: &Node) -> Map<String, Value> {
        match self.reg.get(&node.ty) {
            Some(def) => widget_map(node, def),
            None => Map::new(),
        }
    }

    fn node_inputs(&mut self, c: usize, node: &Node, def: &NodeDef) -> Map<String, Value> {
        let mut m = widget_map(node, def);
        for (idx, inp) in node.inputs.iter().enumerate() {
            if inp.link.is_none() {
                continue;
            }
            let name = inp.widget.clone().unwrap_or_else(|| inp.name.clone());
            match self.resolve_input(c, node, idx, 0) {
                Some(Resolved::Link(id, slot)) => {
                    m.insert(name, json!([id, slot]));
                }
                Some(Resolved::Value(v)) => {
                    m.insert(name, wrap(&v));
                }
                None => {}
            }
        }
        m
    }

    fn resolve_input(&mut self, c: usize, node: &Node, idx: usize, depth: usize) -> Option<Resolved> {
        let link_id = node.inputs.get(idx)?.link?;
        let link = self.ctxs[c].graph.links.get(&link_id)?;
        self.resolve_link(c, link, depth)
    }

    /// Follow `link` (inside ctx `c`) back to whatever produces its value.
    fn resolve_link(&mut self, c: usize, link: &Link, depth: usize) -> Option<Resolved> {
        if depth > MAX_DEPTH {
            self.warnings.push("link resolution too deep (cycle?)".into());
            return None;
        }
        if link.origin_id == SUBGRAPH_INPUT_ID {
            let (pc, host, sg) = self.ctxs[c].parent?;
            let name = &sg.inputs.get(link.origin_slot)?.name;
            let h = host.inputs.iter().position(|i| &i.name == name)?;
            if host.inputs[h].link.is_some() {
                return self.resolve_input(pc, host, h, depth + 1);
            }
            host.inputs[h].widget.as_ref()?;
            // Promoted widget: host widgets_values has one entry per widget input, in order.
            let k = host.inputs[..h].iter().filter(|i| i.widget.is_some()).count();
            if let Some(v) = host.widgets_values.get(k).filter(|v| !v.is_null()) {
                return Some(Resolved::Value(v.clone()));
            }
            // Legacy (properties.proxyWidgets): the value lives on the inner widget the
            // subgraph input feeds.
            let sgi = sg.inputs.get(link.origin_slot)?;
            return sgi.link_ids.iter().find_map(|lid| {
                let l = self.ctxs[c].graph.links.get(lid)?;
                let t = self.ctxs[c].graph.node(&l.target_id)?;
                let w = t.inputs.get(l.target_slot)?.widget.as_ref()?;
                self.widget_values(t).remove(w).map(Resolved::Value)
            });
        }
        let origin = self.ctxs[c].graph.node(&link.origin_id)?;
        self.resolve_output(c, origin, link.origin_slot, &link.ty, depth + 1)
    }

    fn resolve_output(&mut self, c: usize, node: &'a Node, slot: usize, ty: &str, depth: usize) -> Option<Resolved> {
        if depth > MAX_DEPTH {
            return None;
        }
        match node.mode {
            MODE_NEVER => return None,
            MODE_BYPASS => {
                let idx = bypass_input(node, slot, ty)?;
                return self.resolve_input(c, node, idx, depth + 1);
            }
            _ => {}
        }
        if let Some(&child) = self.children.get(&(c, node.id.as_str())) {
            let sg = self.ctxs[child].parent?.2;
            let lid = *sg.outputs.get(slot)?.link_ids.first()?;
            let link = sg.graph.links.get(&lid)?;
            return self.resolve_link(child, link, depth + 1);
        }
        if self.reg.get(&node.ty).is_some() {
            return Some(Resolved::Link(format!("{}{}", self.ctxs[c].prefix, node.id), slot));
        }
        // Virtual nodes
        if node.ty == "PrimitiveNode" {
            return node.widgets_values.get(0).cloned().map(Resolved::Value);
        }
        if node.ty.contains("Reroute") || node.ty == "SetNode" {
            return self.resolve_input(c, node, 0, depth + 1);
        }
        // KJNodes Get/Set: GetNode reads the SetNode with the same name (widget 0).
        if node.ty == "GetNode" {
            let key = node.widgets_values.get(0)?;
            let graph = self.ctxs[c].graph;
            let set = graph.nodes.iter().find(|n| n.ty == "SetNode" && n.widgets_values.get(0) == Some(key))?;
            return self.resolve_input(c, set, 0, depth + 1);
        }
        None
    }
}

/// `ExecutableNodeDTO._getBypassSlotIndex`.
fn bypass_input(node: &Node, slot: usize, ty: &str) -> Option<usize> {
    let ins = &node.inputs;
    if ty.is_empty() || ty == "*" {
        return if slot < ins.len() { Some(slot) } else { (!ins.is_empty()).then_some(0) };
    }
    if ins.get(slot).is_some_and(|i| types_compatible(&i.ty, ty)) {
        return Some(slot);
    }
    ins.iter().position(|i| i.ty == ty).or_else(|| ins.iter().position(|i| types_compatible(&i.ty, ty)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        Registry::from_object_info(&json!({
            "Src": {"input": {"required": {"seed": ["INT", {}], "steps": ["INT", {}]}},
                    "input_order": {"required": ["seed", "steps"]}, "output": ["LATENT"], "output_name": ["LATENT"]},
            "Dst": {"input": {"required": {"latent": ["LATENT", {}], "text": ["STRING", {}]}},
                    "input_order": {"required": ["latent", "text"]}, "output": [], "output_name": []},
        }))
    }

    #[test]
    fn flattens_subgraph_bypass_and_widgets() {
        // root: 1 Src -> 2 (subgraph host) -> 3 Dst ; inside subgraph: -10 -> 7 (bypassed Dst) ... -> 8 Dst
        //  and subgraph output 0 <- -10 passthrough
        let wf = Workflow::from_json(json!({
            "nodes": [
                {"id": 1, "type": "Src", "mode": 0, "widgets_values": [42, "fixed", 20],
                 "outputs": [{"name": "LATENT", "type": "LATENT", "links": [1]}]},
                {"id": 2, "type": "uuid-a", "mode": 0, "widgets_values": ["hello"],
                 "inputs": [{"name": "lat", "type": "LATENT", "link": 1}, {"name": "txt", "type": "STRING", "link": null, "widget": {"name": "txt"}}],
                 "outputs": [{"name": "out", "type": "LATENT", "links": [2]}]},
                {"id": 3, "type": "Dst", "mode": 0, "widgets_values": ["root"],
                 "inputs": [{"name": "latent", "type": "LATENT", "link": 2}]},
                {"id": 4, "type": "Note", "mode": 0},
                {"id": 5, "type": "Dst", "mode": 2, "widgets_values": ["muted"]},
            ],
            "links": [[1, 1, 0, 2, 0, "LATENT"], [2, 2, 0, 3, 0, "LATENT"]],
            "definitions": {"subgraphs": [{
                "id": "uuid-a", "name": "Sub",
                "inputs": [{"name": "lat", "type": "LATENT", "linkIds": [10]}, {"name": "txt", "type": "STRING", "linkIds": [12]}],
                "outputs": [{"name": "out", "type": "LATENT", "linkIds": [13]}],
                "nodes": [
                    {"id": 7, "type": "Dst", "mode": 4, "widgets_values": ["bypassed"],
                     "inputs": [{"name": "latent", "type": "LATENT", "link": 10}],
                     "outputs": [{"name": "x", "type": "LATENT", "links": [11]}]},
                    {"id": 8, "type": "Dst", "mode": 0, "widgets_values": ["inner"],
                     "inputs": [{"name": "latent", "type": "LATENT", "link": 11}, {"name": "text", "type": "STRING", "link": 12, "widget": {"name": "text"}}]},
                ],
                "links": [
                    {"id": 10, "origin_id": -10, "origin_slot": 0, "target_id": 7, "target_slot": 0, "type": "LATENT"},
                    {"id": 11, "origin_id": 7, "origin_slot": 0, "target_id": 8, "target_slot": 0, "type": "LATENT"},
                    {"id": 12, "origin_id": -10, "origin_slot": 1, "target_id": 8, "target_slot": 1, "type": "STRING"},
                    {"id": 13, "origin_id": -10, "origin_slot": 0, "target_id": -20, "target_slot": 0, "type": "LATENT"}
                ]
            }]}
        }))
        .unwrap();
        let (p, warnings) = build(&wf, &reg());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(p["1"]["inputs"], json!({"seed": 42, "steps": 20}), "control_after_generate slot skipped");
        // bypassed 7 passes Src through; promoted widget value comes from the host
        assert_eq!(p["2:8"]["inputs"], json!({"latent": ["1", 0], "text": "hello"}));
        // subgraph output wired straight from its input
        assert_eq!(p["3"]["inputs"], json!({"latent": ["1", 0], "text": "root"}));
        let keys: Vec<_> = p.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 3, "no host, no note, no muted/bypassed: {keys:?}");
    }
}

/// Compare against prompts the web frontend actually sent, stored in output PNGs.
/// Extract cases to /tmp/golden first (see docs/ARCHITECTURE.md), then
/// `cargo test golden -- --ignored --nocapture`.
#[cfg(test)]
mod golden {
    use super::*;

    /// JSON equality where 1 == 1.0 (JS has one number type).
    fn num_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| num_eq(a, b)),
            (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| num_eq(v, w))),
            _ => a == b,
        }
    }

    #[test]
    #[ignore]
    fn golden_prompts() {
        let reg = Registry::from_object_info(&serde_json::from_str(&std::fs::read_to_string("/tmp/oi.json").unwrap()).unwrap());
        let mut files: Vec<_> = std::fs::read_dir("/tmp/golden").unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        let (mut nodes_ok, mut nodes_total, mut exact) = (0, 0, 0);
        let mut by_type = std::collections::BTreeMap::new();
        for f in &files {
            // some PNGs carry NaN (Python json) -> not valid JSON; skip those
            let Ok(g) = serde_json::from_str::<Value>(&std::fs::read_to_string(f).unwrap()) else { continue };
            let wf = Workflow::from_json(g["workflow"].clone()).unwrap();
            let (ours, _) = build(&wf, &reg);
            let theirs = g["prompt"].as_object().unwrap();
            let ours = ours.as_object().unwrap();
            let mut diffs = Vec::new();
            for (id, t) in theirs {
                nodes_total += 1;
                // Only compare node types we know (the backend may have changed since).
                match ours.get(id) {
                    Some(o) if num_eq(&o["inputs"], &t["inputs"]) => nodes_ok += 1,
                    Some(o) => {
                        *by_type.entry(t["class_type"].as_str().unwrap_or("").to_owned()).or_insert(0) += 1;
                        diffs.push(format!("  {id} {} ours={} theirs={}", t["class_type"], o["inputs"], t["inputs"]))
                    }
                    None => diffs.push(format!("  {id} {} missing", t["class_type"])),
                }
            }
            for id in ours.keys().filter(|k| !theirs.contains_key(*k)) {
                diffs.push(format!("  {id} {} extra", ours[id]["class_type"]));
            }
            if diffs.is_empty() {
                exact += 1;
            } else {
                eprintln!("{}: {} diffs", f.display(), diffs.len());
                for d in diffs.iter() {
                    eprintln!("{d}");
                }
            }
        }
        eprintln!("mismatching class types: {by_type:?}");
        eprintln!("exact {exact}/{}  nodes {nodes_ok}/{nodes_total}", files.len());
    }
}
