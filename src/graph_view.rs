// SPDX-License-Identifier: GPL-3.0-only
//! Workflow graph → canvas scene (nodes, links, groups).
//!
//! Positions and sizes come straight from the file (litegraph coordinates).
//! Widgets are built per node from the registry (`build_widgets`); edits are written
//! back by the workspace using the widget keys defined there.

use crate::api;
use crate::canvas::widgets::{WKind, Widget};
use crate::canvas::{self, Group, Link, Node, Slot, parse_color};
use crate::prompt::{self, Role, WSlot};
use crate::registry::{Input, Kind, Registry};
use crate::workflow::{self, Graph, SUBGRAPH_INPUT_ID, SUBGRAPH_OUTPUT_ID, Subgraph, SubgraphIo, Workflow};
use serde_json::{json, Value};
use std::collections::HashMap;

fn socket(io: &[SubgraphIo]) -> Vec<Slot> {
    io.iter().map(|i| Slot { label: i.name.clone(), ty: i.ty.clone() }).collect()
}

/// Canvas widget kind for a registry input (what the web frontend would draw).
fn kind_of(i: &Input, role: Role) -> WKind {
    match role {
        Role::Control => return WKind::Combo(["fixed", "increment", "decrement", "randomize"].map(String::from).to_vec()),
        Role::Upload => return WKind::Button,
        Role::Value => {}
    }
    let num = |k: &str, d: f64| i.opts[k].as_f64().unwrap_or(d);
    match &i.kind {
        Kind::Int => WKind::Int { min: num("min", -1e15), max: num("max", 1e15), step: num("step", 1.0).max(1.0) },
        Kind::Float => {
            let step = num("step", 0.1);
            let prec = i.opts["round"].as_f64().filter(|r| *r > 0.0).map(|r| -r.log10().floor()).unwrap_or(-step.log10().floor()).clamp(1.0, 6.0) as usize;
            WKind::Float { min: num("min", -1e15), max: num("max", 1e15), step, prec }
        }
        Kind::Boolean => WKind::Bool,
        Kind::Combo(o) => WKind::Combo(o.clone()),
        Kind::String => WKind::Text { multiline: i.opts["multiline"].as_bool().unwrap_or(false) },
        _ => WKind::ReadOnly,
    }
}

fn slot_widget(s: &WSlot, key: String) -> Widget {
    let (label, value) = match s.role {
        Role::Control => ("control_after_generate", if s.value.is_null() { Value::from("randomize") } else { s.value.clone() }),
        Role::Upload => ("choose file to upload", Value::Null),
        Role::Value => (s.name.as_str(), s.value.clone()),
    };
    let key = if s.role == Role::Upload { format!("upload:{}", s.idx) } else { key };
    let mut w = Widget::new(label, key, kind_of(&s.input, s.role), value);
    w.advanced = truthy_opt(&s.input, "advanced");
    w.default = s.default.clone();
    w
}

/// Display name of a node as the web frontend titles it (custom title, subgraph name,
/// registry display name, else the type).
fn node_label(wf: &Workflow, reg: &Registry, n: &workflow::Node) -> String {
    n.title
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| wf.subgraphs.get(&n.ty).map(|s| s.name.clone()))
        .or_else(|| reg.get(&n.ty).map(|d| d.display_name.clone()).filter(|d| !d.is_empty()))
        .unwrap_or_else(|| n.ty.clone())
}

/// Option list of the first `LoraLoader.lora_name` combo (feeds rgthree Power Lora rows).
fn loras(reg: &Registry) -> Vec<String> {
    reg.get("LoraLoader")
        .and_then(|d| d.inputs.iter().find(|i| i.name == "lora_name"))
        .map(|i| match &i.kind {
            Kind::Combo(o) => o.clone(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

/// Indices of the groups a rgthree "Fast Groups …" node lists, in its sort order.
pub fn fast_groups(graph: &Graph, node: &workflow::Node) -> Vec<usize> {
    let p = &node.properties;
    let re = p["matchTitle"].as_str().filter(|t| !t.is_empty()).and_then(|t| regex::RegexBuilder::new(t).case_insensitive(true).build().ok());
    let mut idx: Vec<usize> = (0..graph.groups.len())
        .filter(|&i| re.as_ref().is_none_or(|r| r.is_match(&graph.groups[i].title)))
        .collect();
    match p["sort"].as_str() {
        Some("alphanumeric") => idx.sort_by(|&a, &b| graph.groups[a].title.cmp(&graph.groups[b].title)),
        // "position": top-to-bottom then left-to-right
        _ => idx.sort_by(|&a, &b| {
            let (ga, gb) = (graph.groups[a].bounding, graph.groups[b].bounding);
            (ga[1], ga[0]).partial_cmp(&(gb[1], gb[0])).unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
    idx
}

/// Indices of the nodes whose centre lies inside group `gi`.
pub fn group_members(graph: &Graph, gi: usize) -> Vec<usize> {
    let [x, y, w, h] = graph.groups[gi].bounding;
    graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            let (cx, cy) = (n.pos.0 + n.size.0 / 2.0, n.pos.1 + n.size.1 / 2.0 - 15.0);
            cx >= x && cx <= x + w && cy >= y && cy <= y + h
        })
        .map(|(i, _)| i)
        .collect()
}

/// `a/b/c.png [output]` → `/view` query for a LoadImage-style value.
pub(crate) fn image_query(v: &str) -> String {
    let (path, kind) = match v.rsplit_once(" [") {
        Some((p, k)) if k.ends_with(']') => (p, &k[..k.len() - 1]),
        _ => (v, "input"),
    };
    let (sub, name) = path.rsplit_once('/').unwrap_or(("", path));
    api::OutputImage { filename: name.into(), subfolder: sub.into(), kind: kind.into() }.query()
}

/// pysssss ShowText stores its boxes as a list of lists of lines; one box per string.
fn show_lines(vals: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for v in vals.as_array().into_iter().flatten() {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => out.extend(a.iter().filter_map(Value::as_str).map(str::to_owned)),
            _ => {}
        }
    }
    out
}

/// `<lora:NAME:STRENGTH>` tags of a LoRA Manager prompt, in order.
fn lm_tags(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<lora:") {
        let Some(end) = rest[at..].find('>') else { break };
        let body = &rest[at + 6..at + end];
        let (name, strength) = match body.rsplit_once(':') {
            Some((n, s)) => (n, s.trim().parse::<f64>().unwrap_or(1.0)),
            None => (body, 1.0),
        };
        out.push((name.trim().to_owned(), strength));
        rest = &rest[at + end + 1..];
    }
    out
}

/// Lora Manager stores its hidden widgets in `properties.__lm_widget_ids`, in the same
/// order as `widgets_values`. Reading the indices from there (rather than hard-coding
/// 1/2) makes the text↔rows sync work for every Lora Manager loader, including ones
/// whose optional inputs shift the widget order.
fn lm_indices(n: &workflow::Node) -> Option<(usize, usize)> {
    if let Some(ids) = n.properties.get("__lm_widget_ids").and_then(Value::as_array) {
        let pos = |name: &str| ids.iter().position(|v| v.as_str() == Some(name));
        if let (Some(t), Some(l)) = (pos("text"), pos("loras")) {
            return Some((t, l));
        }
    }
    // Older saves have no id map, but still carry the same three widgets.
    let three = n.widgets_values.as_array().is_some_and(|a| a.len() >= 3);
    (three && n.inputs.iter().any(|i| i.ty == "AUTOCOMPLETE_TEXT_LORAS")).then_some((1, 2))
}

fn lm_name(e: &Value) -> &str {
    e["name"].as_str().or_else(|| e["lora"].as_str()).unwrap_or("")
}

/// Merge the tags currently in the prompt into the stored row array, keeping every
/// surviving row's `active`/`expanded`/`clipStrength` flags. Tags removed from the
/// prompt disappear from the array (and therefore from execution), exactly like
/// LoraManager's `mergeLoras`.
fn lm_merge_entries(text: &str, existing: &[Value]) -> Vec<Value> {
    let mut pool: Vec<Value> = existing.iter().filter(|e| !lm_name(e).is_empty()).cloned().collect();
    let mut out = Vec::new();
    for (name, strength) in lm_tags(text) {
        let at = pool.iter().position(|e| lm_name(e) == name);
        let mut e = match at {
            Some(i) => pool.remove(i),
            None => json!({"name": name, "strength": strength, "clipStrength": strength, "active": true, "expanded": false}),
        };
        e["name"] = json!(name);
        e["strength"] = json!(strength);
        if e.get("active").is_none() {
            e["active"] = json!(true);
        }
        out.push(e);
    }
    out
}

/// Rewrite the prompt's `<lora:…>` tags from the row array (LoraManager's
/// `applyLoraValuesToText`): tags whose row was removed disappear, strengths follow the
/// rows, and rows added by the picker are appended. Inactive rows keep their tag.
fn lm_text_apply(entries: &[Value], text: &str) -> String {
    let mut used = vec![false; entries.len()];
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("<lora:") {
        let Some(end) = rest[at..].find('>') else { break };
        let body = &rest[at + 6..at + end];
        let name = body.rsplit_once(':').map(|(n, _)| n).unwrap_or(body).trim();
        out.push_str(&rest[..at]);
        if let Some(i) = entries.iter().enumerate().position(|(i, e)| !used[i] && lm_name(e) == name) {
            used[i] = true;
            out.push_str(&format!("<lora:{}:{:.2}>", lm_name(&entries[i]), entries[i]["strength"].as_f64().unwrap_or(1.0)));
        }
        // else: the row is gone, so the tag goes with it.
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    for (i, e) in entries.iter().enumerate() {
        if !used[i] {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(&format!("<lora:{}:{:.2}>", lm_name(e), e["strength"].as_f64().unwrap_or(1.0)));
        }
    }
    // Do not leave a run of spaces where a deleted tag used to sit.
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.trim().to_owned()
}

/// Rows currently stored on a Lora Manager loader. Older files keep the tags only;
/// derive rows from the prompt so the first toggle/edit can populate the hidden array.
fn lm_entries(n: &workflow::Node) -> Vec<Value> {
    let Some((ti, li)) = lm_indices(n) else { return Vec::new() };
    let arr = n.widgets_values.get(li).and_then(Value::as_array).cloned().unwrap_or_default();
    if !arr.is_empty() {
        return arr;
    }
    let text = n.widgets_values.get(ti).and_then(Value::as_str).unwrap_or_default();
    lm_merge_entries(text, &[])
}

fn lm_store(n: &mut workflow::Node, entries: &[Value], text: Option<String>) {
    let Some((ti, li)) = lm_indices(n) else { return };
    set_at(&mut n.widgets_values, li, Value::Array(entries.to_vec()));
    if let Some(t) = text {
        set_at(&mut n.widgets_values, ti, json!(t));
    }
}

/// Editable widgets for one node (empty when nothing is known about it).
pub fn build_widgets(wf: &Workflow, graph: &Graph, n: &workflow::Node, reg: &Registry, images: &mut Vec<String>) -> Vec<Widget> {
    let shadow = |w: &mut Widget, name: &str| w.input = n.inputs.iter().position(|i| i.widget.as_deref() == Some(name));
    let vals = n.widgets_values.as_array();
    match n.ty.as_str() {
        "Note" | "MarkdownNote" => {
            return vals.and_then(|v| v.first()).map(|v| vec![Widget::new("note", "0", WKind::Text { multiline: true }, v.clone())]).unwrap_or_default();
        }
        ty if crate::erenodes::is_ere(ty) && ty != "ErePromptMultiline" => {
            use crate::canvas::widgets::CloudMode;
            let dim = |k: &str| n.properties[k].as_f64().filter(|&v| v >= 20.0).unwrap_or(100.0);
            let mode = match ty {
                "ErePromptGallery" => CloudMode::Gallery(dim("_tagImageWidth"), dim("_tagImageHeight")),
                "ErePromptToggle" => CloudMode::Toggle,
                "ErePromptMultiSelect" => CloudMode::Select { random: false },
                "ErePromptRandomizer" => CloudMode::Select { random: true },
                _ => CloudMode::Cloud,
            };
            let mut w = Widget::new("tags", "ere", WKind::Cloud(mode), crate::erenodes::tags(&n.properties));
            w.structural = true;
            return vec![w];
        }
        "ShowText|pysssss" => {
            // The web UI swaps the widget for one read-only box per text the node received;
            // the file keeps that as a list of lists.
            let mut out = Vec::new();
            for (i, line) in show_lines(&n.widgets_values).into_iter().enumerate() {
                out.push(Widget::new("text", format!("show:{i}"), WKind::Text { multiline: true }, Value::String(line)));
            }
            if out.is_empty() {
                out.push(Widget::new("text", "show:0", WKind::Text { multiline: true }, Value::String(String::new())));
            }
            return out;
        }
        _ if lm_indices(n).is_some() => {
            // Lora Manager keeps two sources of truth: the prompt text and a hidden
            // `loras` widget array. The array is what the Python loader executes
            // (`load_loras` ignores `text`), so rows are built from it and every edit
            // writes both sides.
            let (ti, _) = lm_indices(n).unwrap();
            let opts = loras(reg);
            let text = n.widgets_values.get(ti).and_then(Value::as_str).unwrap_or_default();
            let mut prompt = Widget::new("prompt", ti.to_string(), WKind::Text { multiline: true }, Value::String(text.to_owned()));
            prompt.structural = true; // a committed prompt adds/removes rows below it
            let mut out = vec![prompt];
            for (i, e) in lm_entries(n).iter().enumerate() {
                let name = lm_name(e).to_owned();
                if name.is_empty() {
                    continue;
                }
                let on = e["active"].as_bool().unwrap_or(false);
                let strength = e["strength"].as_f64().unwrap_or(1.0);
                let mut w = Widget::new("lora", format!("lm:{i}"), WKind::Lora(opts.clone()), json!({ "on": on, "lora": name, "strength": strength }));
                w.structural = true; // removing/adding a row re-numbers the rest
                out.push(w);
            }
            let mut add = Widget::new("➕ Add LoRA", "lm+", WKind::Button, Value::Null);
            add.structural = true;
            out.push(add);
            return out;
        }
        "ImageBBoxLayout" if bbox_slots(n, reg).is_some() => return bbox_widgets(n, reg, images),
        "Power Lora Loader (rgthree)" => {
            let opts = loras(reg);
            let mut out = vec![Widget::new("Loras", "header", WKind::Label, Value::Null)];
            for (i, v) in vals.into_iter().flatten().enumerate().filter(|(_, v)| v.get("lora").is_some()) {
                let mut w = Widget::new("lora", format!("lora:{i}"), WKind::Lora(opts.clone()), v.clone());
                w.structural = true; // removing a row re-numbers the rest
                out.push(w);
            }
            let mut add = Widget::new("➕ Add Lora", "lora+", WKind::Button, Value::Null);
            add.structural = true;
            out.push(add);
            return out;
        }
        ty if ty == crate::dasiwa::SEED_CONTROL => {
            // Mode + dice instead of the raw state string; `seed` only mirrors `seed_value`.
            let Some(def) = reg.get(ty) else { return Vec::new() };
            let mut out = Vec::new();
            for s in prompt::widget_slots(n, def) {
                if s.name == "seed" {
                    continue;
                }
                let mut w = if s.name == "seed_control_state" {
                    Widget::new("seed mode", s.idx.to_string(), WKind::Seed, s.value.clone())
                } else {
                    slot_widget(&s, s.idx.to_string())
                };
                shadow(&mut w, &s.name);
                out.push(w);
            }
            return out;
        }
        ty if ty == crate::dasiwa::LTX2_LORA => {
            // `stack_data` is a hidden JSON string in the web UI; show the rows instead.
            let Some(def) = reg.get(ty) else { return Vec::new() };
            let opts = loras(reg);
            let rows = crate::dasiwa::stack_rows(&n.widgets_values);
            let mut out = Vec::new();
            for s in prompt::widget_slots(n, def) {
                if s.name == "stack_data" {
                    for (i, row) in rows.iter().enumerate() {
                        let mut w = Widget::new("lora", format!("stack:{i}"), WKind::Lora(opts.clone()), row.clone());
                        w.structural = true;
                        out.push(w);
                    }
                    let mut add = Widget::new("➕ Add LoRA", "stack+", WKind::Button, Value::Null);
                    add.structural = true;
                    out.push(add);
                    continue;
                }
                let mut w = slot_widget(&s, s.idx.to_string());
                shadow(&mut w, &s.name);
                out.push(w);
            }
            return out;
        }
        "Fast Groups Bypasser (rgthree)" | "Fast Groups Muter (rgthree)" => {
            return fast_groups(graph, n)
                .into_iter()
                .map(|gi| {
                    let active = group_members(graph, gi).iter().any(|&k| graph.nodes[k].mode == 0);
                    let mut w = Widget::new(&graph.groups[gi].title, format!("group:{gi}"), WKind::Bool, Value::Bool(active));
                    w.structural = true; // flips other nodes' modes
                    w
                })
                .collect();
        }
        _ => {}
    }

    if let Some(sg) = wf.subgraphs.get(&n.ty) {
        // Promoted widgets as the 1.60 frontend stores them: the instance input socket
        // shadows an inner node widget (`{"widget": {"name": …}}`) and its value sits in
        // `widgets_values` at the input's widget position.
        let mut rows = Vec::new();
        let mut promoted: Vec<(String, String)> = Vec::new();
        let mut modern: Vec<(String, String)> = Vec::new();
        let mut at = 0;
        for (i, inp) in n.inputs.iter().enumerate() {
            let Some(widget) = inp.widget.as_deref() else { continue };
            let vi = at;
            at += 1;
            let Some(io) = sg.inputs.iter().find(|s| s.name == inp.name) else { continue };
            let Some(l) = sg.graph.links.values().find(|l| io.link_ids.contains(&l.id)) else { continue };
            let Some(inner) = sg.graph.node(&l.target_id) else { continue };
            promoted.push((inner.id.clone(), widget.to_owned()));
            modern.push((inner.id.clone(), widget.to_owned()));
            if inp.link.is_some() {
                continue; // wired on the instance: the value is ignored, the socket shows it
            }
            let Some(def) = reg.get(&inner.ty) else { continue };
            let slots = prompt::widget_slots(inner, def);
            let found = if widget == "control_after_generate" {
                slots.iter().find(|s| s.role == Role::Control)
            } else {
                slots.iter().find(|s| s.role == Role::Value && s.name == widget)
            };
            let Some(s) = found else { continue };
            let mut w = slot_widget(s, format!("w:{vi}"));
            w.label = inp.label.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| inp.name.clone());
            w.input = Some(i);
            w.source = node_label(wf, reg, inner);
            if let Some(v) = vals.and_then(|a| a.get(vi)).filter(|v| !v.is_null()) {
                w.value = v.clone();
            }
            rows.push(w);
        }
        // Older subgraphs: promoted widgets listed in properties.proxyWidgets (rgthree
        // style). Keep them alongside modern inputs: promotion on such a file appends
        // sockets while the stored proxy list still names the same widgets.
        for p in n.properties["proxyWidgets"].as_array().into_iter().flatten() {
            let (Some(inner_id), Some(name)) = (p[0].as_str(), p[1].as_str()) else { continue };
            // The same widget may be listed both as a modern input and a proxy (the file
            // was promoted in 1.60 and later opened in an older frontend, or vice versa).
            if modern.iter().any(|(id, w)| id == inner_id && w == name) {
                continue;
            }
            promoted.push((inner_id.to_owned(), name.to_owned()));
            let Some(inner) = sg.graph.node(inner_id) else { continue };
            let Some(def) = reg.get(&inner.ty) else { continue };
            let slots = prompt::widget_slots(inner, def);
            let found = if name == "control_after_generate" {
                slots.iter().find(|s| s.role == Role::Control)
            } else {
                slots.iter().find(|s| s.role == Role::Value && s.name == name)
            };
            if let Some(s) = found {
                let mut w = slot_widget(s, format!("p:{inner_id}:{}", s.idx));
                shadow(&mut w, name);
                let shown = w.input.and_then(|i| n.inputs.get(i)).and_then(|i| i.label.clone());
                w.label = shown.filter(|l| !l.is_empty()).unwrap_or_else(|| name.to_owned());
                w.source = node_label(wf, reg, inner);
                rows.push(w);
            }
        }
        if !rows.is_empty() {
            return rows;
        }
    }

    let Some(def) = reg.get(&n.ty) else { return Vec::new() };
    if vals.is_none() {
        // Object-style / extension layouts: show values, don't edit.
        return prompt::widget_map(n, def).into_iter().map(|(k, v)| Widget::new(&k, k.clone(), WKind::ReadOnly, v)).collect();
    }
    let mut out = Vec::new();
    let slots = prompt::widget_slots(n, def);
    // Picker combos (easy wildcards / easy prompt / …): a STRING widget to append to.
    let text_idx = slots
        .iter()
        .filter(|s| s.role == Role::Value && matches!(s.input.kind, Kind::String))
        .min_by_key(|s| s.name != "text")
        .map(|s| s.idx);
    for s in slots {
        let mut w = slot_widget(&s, s.idx.to_string());
        if let (Some(t), Some(ph)) = (text_idx, picker_placeholder(&s.input)) {
            // A pick rewrites the text widget and snaps back: re-render from the model.
            w.key = format!("pick:{}:{t}:{ph}", s.idx);
            w.structural = true;
        }
        if s.role == Role::Value {
            shadow(&mut w, &s.name);
            if matches!(s.input.kind, Kind::Custom(_) | Kind::DynCombo(_)) {
                w.kind = WKind::ReadOnly;
            }
            if truthy_opt(&s.input, "image_upload") {
                w.structural = true; // preview follows the value
                if let Some(v) = s.value.as_str().filter(|v| !v.is_empty()) {
                    images.push(image_query(v));
                }
            }
        }
        out.push(w);
    }
    out
}

/// Interior widgets of a subgraph instance that are not promoted to its sockets: the
/// property panel's ADVANCED INPUTS rows. Writes go through the same `p:<inner>:<idx>`
/// keys `apply_widget` already handles.
pub fn subgraph_advanced_widgets(wf: &Workflow, host: &workflow::Node, reg: &Registry) -> Vec<Widget> {
    let Some(sg) = wf.subgraphs.get(&host.ty) else { return Vec::new() };
    let mut promoted: Vec<(String, String)> = host
        .inputs
        .iter()
        .filter_map(|inp| {
            let widget = inp.widget.clone()?;
            let io = sg.inputs.iter().find(|s| s.name == inp.name)?;
            let l = sg.graph.links.values().find(|l| io.link_ids.contains(&l.id))?;
            Some((l.target_id.clone(), widget))
        })
        .collect();
    for p in host.properties["proxyWidgets"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (p[0].as_str(), p[1].as_str()) {
            promoted.push((id.to_owned(), name.to_owned()));
        }
    }
    let mut out = Vec::new();
    for inner in &sg.graph.nodes {
        let Some(def) = reg.get(&inner.ty) else { continue };
        let label = node_label(wf, reg, inner);
        for s in prompt::widget_slots(inner, def).into_iter().filter(|s| s.role == Role::Value) {
            if promoted.iter().any(|(id, name)| id == &inner.id && name == &s.name) {
                continue;
            }
            // Driven by a wire into the inner node: nothing to edit here.
            if inner.inputs.iter().any(|i| i.widget.as_deref() == Some(s.name.as_str()) && i.link.is_some()) {
                continue;
            }
            let mut w = slot_widget(&s, format!("p:{}:{}", inner.id, s.idx));
            w.advanced = true;
            w.source = label.clone();
            let renamed = inner.inputs.iter().find(|i| i.widget.as_deref() == Some(s.name.as_str())).and_then(|i| i.label.clone());
            if let Some(l) = renamed.filter(|l| !l.is_empty()) {
                w.label = l;
            }
            out.push(w);
        }
    }
    out
}

/// `p:<inner>:<idx>` row of subgraph type `sg` → (inner node id, widget name).
pub fn interior_slot(wf: &Workflow, sg: &str, key: &str, reg: &Registry) -> Option<(String, String)> {
    let (inner_id, idx) = key.strip_prefix("p:")?.split_once(':')?;
    let idx: usize = idx.parse().ok()?;
    let inner = wf.subgraphs.get(sg)?.graph.node(inner_id)?;
    let s = prompt::widget_slots(inner, reg.get(&inner.ty)?).into_iter().find(|s| s.idx == idx)?;
    Some((inner_id.to_owned(), s.name))
}

/// Image BBox Layout keeps its picture, boxes and mask in hidden widgets; the web
/// extension draws an upload button, a preview with the boxes and an editor instead.
/// Slots by name, so a reordered node definition still lines up.
pub(crate) fn bbox_slots(n: &workflow::Node, reg: &Registry) -> Option<HashMap<String, WSlot>> {
    let m: HashMap<String, WSlot> = prompt::widget_slots(n, reg.get(&n.ty)?).into_iter().map(|s| (s.name.clone(), s)).collect();
    (m.contains_key("image") && m.contains_key("bbox_data")).then_some(m)
}

fn bbox_str<'a>(m: &'a HashMap<String, WSlot>, k: &str) -> &'a str {
    m.get(k).and_then(|s| s.value.as_str()).unwrap_or_default()
}

fn bbox_widgets(n: &workflow::Node, reg: &Registry, images: &mut Vec<String>) -> Vec<Widget> {
    let m = bbox_slots(n, reg).unwrap_or_default();
    const HIDDEN: [&str; 6] = ["image", "bbox_data", "mask_file", "mask_mode", "mask_grow", "mask_feather"];
    let image = bbox_str(&m, "image");
    let boxes = crate::maskedit::parse_boxes(bbox_str(&m, "bbox_data"));
    let painted = !bbox_str(&m, "mask_file").is_empty();
    let show_prompt = n.properties["show_prompt"].as_bool().unwrap_or(false);
    // The web extension shows either the prompt box or the region editor, never both.
    let mut up = Widget::new("📁 Upload image…", "bbox:upload", WKind::Button, Value::Null);
    up.structural = true;
    let mut out = vec![up, Widget::new("image", "bbox:image", WKind::ReadOnly, Value::from(if image.is_empty() { "(none)" } else { image }))];
    if !image.is_empty() && !show_prompt {
        images.push(image_query(image));
    }
    let mut slots: Vec<&WSlot> = m.values().filter(|s| !HIDDEN.contains(&s.name.as_str()) && s.name != "prompt").collect();
    slots.sort_by_key(|s| s.idx);
    out.extend(slots.into_iter().map(|s| slot_widget(s, s.idx.to_string())));
    let mut toggle = Widget::new("prompt box", "bbox:show_prompt", WKind::Bool, Value::Bool(show_prompt));
    toggle.structural = true;
    out.push(toggle);
    if show_prompt {
        if let Some(s) = m.get("prompt") {
            out.push(slot_widget(s, s.idx.to_string()));
        }
        return out;
    }
    let mut edit = Widget::new("✏ Edit boxes & mask", "bbox:edit", WKind::Button, Value::Null);
    edit.structural = true;
    out.push(edit);
    let summary = format!("{} box{}{}", boxes.len(), if boxes.len() == 1 { "" } else { "es" }, if painted { " · mask painted" } else { "" });
    out.push(Widget::new("regions", "bbox:summary", WKind::ReadOnly, Value::from(summary)));
    out
}

/// Boxes and painted mask drawn over the Image BBox Layout preview.
fn bbox_overlay(n: &workflow::Node, reg: &Registry) -> Option<canvas::Overlay> {
    if n.ty != "ImageBBoxLayout" {
        return None;
    }
    let m = bbox_slots(n, reg)?;
    let mask = Some(bbox_str(&m, "mask_file")).filter(|v| !v.is_empty()).map(image_query);
    let boxes = crate::maskedit::parse_boxes(bbox_str(&m, "bbox_data")).iter().map(|b| [b.x, b.y, b.w, b.h]).collect();
    Some(canvas::Overlay { mask, boxes, alpha_mask: false })
}

pub(crate) fn truthy_opt(i: &Input, k: &str) -> bool {
    i.opts[k].as_bool().unwrap_or(false)
}

fn set_at(vals: &mut Value, idx: usize, v: Value) {
    if let Value::Array(a) = vals {
        if a.len() <= idx {
            a.resize(idx + 1, Value::Null);
        }
        a[idx] = v;
    }
}

/// Write a canvas widget edit (see `build_widgets` for the key scheme) into the model.
/// `graph` is the one on screen; `first_lora` seeds new Power Lora rows.
/// Returns false when the key is not a model edit (e.g. an upload button).
pub fn apply_widget(wf: &mut Workflow, path: &[String], node_id: &str, key: &str, v: Value, first_lora: &str) -> bool {
    let graph = match path.last() {
        Some(p) => match wf.subgraphs.get_mut(p) {
            Some(sg) => &mut sg.graph,
            None => return false,
        },
        None => &mut wf.root,
    };
    let Some(ni) = graph.nodes.iter().position(|n| n.id == node_id) else { return false };
    if key == "ere" {
        let node = &mut graph.nodes[ni];
        let mut tags = v.clone();
        if tags.is_object() {
            // Ops handled here: shuffle (Randomizer): keep the active count, move it to random tags.
            if tags["op"] != "random" {
                return false;
            }
            let cur = crate::erenodes::tags(&node.properties);
            let mut list: Vec<Value> = cur.as_array().cloned().unwrap_or_default();
            let count = list.iter().filter(|t| crate::erenodes::active(t)).count();
            for i in (1..list.len()).rev() {
                list.swap(i, (rand_u64() % (i as u64 + 1)) as usize);
            }
            for (i, t) in list.iter_mut().enumerate() {
                t["active"] = Value::Bool(i < count);
            }
            tags = Value::Array(list);
        }
        crate::erenodes::store(&mut node.properties, &tags);
        return true;
    }
    if let Some(g) = key.strip_prefix("group:").and_then(|g| g.parse::<usize>().ok()) {
        // rgthree "always one": picking a group turns the others off, the last one can't be turned off.
        let shown = fast_groups(graph, &graph.nodes[ni]);
        let on = v.as_bool().unwrap_or(false);
        let others_on = shown.iter().filter(|&&o| o != g).any(|&o| group_members(graph, o).iter().any(|&k| graph.nodes[k].mode == 0));
        let always_one = graph.nodes[ni].properties["toggleRestriction"].as_str() == Some("always one");
        if !on && always_one && !others_on {
            return true;
        }
        let todo: Vec<(usize, bool)> = if on && always_one {
            shown.iter().map(|&o| (o, o == g)).collect()
        } else {
            vec![(g, on)]
        };
        for (gi, on) in todo {
            for k in group_members(graph, gi) {
                if k != ni {
                    graph.nodes[k].mode = if on { 0 } else { workflow::MODE_BYPASS };
                }
            }
        }
        return true;
    }
    if let Some(vi) = key.strip_prefix("w:").and_then(|i| i.parse::<usize>().ok()) {
        // Promoted widget on a subgraph instance: the value lives on the instance, at the
        // widget position of its input socket.
        set_at(&mut graph.nodes[ni].widgets_values, vi, v);
        return true;
    }
    if let Some(inner) = key.strip_prefix("p:") {
        let (Some((id, idx)), Some(ty)) = (inner.rsplit_once(':'), Some(graph.nodes[ni].ty.clone())) else { return false };
        let Ok(idx) = idx.parse() else { return false };
        if let Some(n) = wf.subgraphs.get_mut(&ty).and_then(|sg| sg.graph.nodes.iter_mut().find(|n| n.id == id)) {
            set_at(&mut n.widgets_values, idx, v);
        }
        return true;
    }
    let node = &mut graph.nodes[ni];
    if key == "lora+" {
        if let Value::Array(a) = &mut node.widgets_values {
            // [{} divider, header, loras…, {} divider, "" add-button]: new rows go before the second divider.
            let at = a.iter().rposition(|v| v.as_object().is_some_and(|o| !o.contains_key("lora") && !o.contains_key("type"))).filter(|&i| i > 1).unwrap_or(a.len());
            a.insert(at, json!({"on": true, "lora": first_lora, "strength": 1, "strengthTwo": null}));
        }
        return true;
    }
    if let Some(i) = key.strip_prefix("lora:").and_then(|i| i.parse::<usize>().ok()) {
        if let Value::Array(a) = &mut node.widgets_values {
            match v {
                Value::Null if i < a.len() => {
                    a.remove(i);
                }
                v => set_at(&mut node.widgets_values, i, v),
            }
        }
        return true;
    }
    if key == "stack+" || key.starts_with("stack:") {
        let i = key.strip_prefix("stack:").and_then(|i| i.parse::<usize>().ok());
        // "➕ Add LoRA" emits Null: append a row on the first available LoRA.
        let row = if i.is_none() && v.is_null() { json!({"on": true, "lora": first_lora, "strength": 1.0}) } else { v };
        crate::dasiwa::stack_set(&mut node.widgets_values, i, &row);
        // The web UI mirrors the hidden string into `properties.stack_data` too.
        if let Some(s) = node.widgets_values.get(2).and_then(Value::as_str) {
            node.properties["stack_data"] = Value::String(s.to_owned());
        }
        return true;
    }
    if let Some((ti, _)) = lm_indices(node) {
        // Prompt edits, row toggles, strength changes and removals all update the
        // hidden `loras` array. Toggling only flips `active`; text is untouched, which
        // matches LoraManager (an inactive row keeps its tag).
        if key == ti.to_string() {
            let text = v.as_str().unwrap_or_default().to_owned();
            let existing = node.widgets_values.get(lm_indices(node).unwrap().1).and_then(Value::as_array).cloned().unwrap_or_default();
            let entries = lm_merge_entries(&text, &existing);
            lm_store(node, &entries, Some(text));
            return true;
        }
        if key == "lm+" || key.starts_with("lm:") {
            let i = key.strip_prefix("lm:").and_then(|i| i.parse::<usize>().ok());
            let mut entries = lm_entries(node);
            let old_text = node.widgets_values.get(ti).and_then(Value::as_str).unwrap_or_default().to_owned();
            match (i, v) {
                (None, _) => entries.push(json!({
                    "name": first_lora, "strength": 1.0, "clipStrength": 1.0,
                    "active": true, "expanded": false
                })),
                (Some(i), Value::Null) if i < entries.len() => { entries.remove(i); }
                (Some(i), v) if i < entries.len() => {
                    let old_name = lm_name(&entries[i]).to_owned();
                    let old_strength = entries[i]["strength"].as_f64().unwrap_or(1.0);
                    let name = v["lora"].as_str().unwrap_or(&old_name).to_owned();
                    let strength = v["strength"].as_f64().unwrap_or(old_strength);
                    if let Some(on) = v["on"].as_bool() {
                        entries[i]["active"] = json!(on);
                    }
                    entries[i]["name"] = json!(name);
                    entries[i]["strength"] = json!(strength);
                    if entries[i].get("clipStrength").is_none() {
                        entries[i]["clipStrength"] = json!(strength);
                    }
                }
                _ => return true,
            }
            // Replace/remove/add changes the text; toggling does not.
            let text = lm_text_apply(&entries, &old_text);
            lm_store(node, &entries, Some(text));
            return true;
        }
    }
    if let Some(rest) = key.strip_prefix("pick:") {
        let mut it = rest.splitn(3, ':');
        let (Some(Ok(idx)), Some(Ok(t)), Some(ph)) = (it.next().map(str::parse), it.next().map(str::parse), it.next()) else { return false };
        set_at(&mut node.widgets_values, idx, v);
        picker_append(node, idx, t, ph);
        return true;
    }
    if let Some(i) = key.strip_prefix("show:").and_then(|i| i.parse::<usize>().ok()) {
        set_at(&mut node.widgets_values, i, v);
        return true;
    }
    match key.parse::<usize>() {
        Ok(idx) => {
            set_at(&mut node.widgets_values, idx, v);
            // DaSiWa seed panel: keep `seed_value` showing the panel's last seed.
            if node.ty == crate::dasiwa::SEED_CONTROL && idx == 1 {
                let raw = node.widgets_values[1].as_str().unwrap_or("").to_owned();
                if let Some(n) = crate::dasiwa::SeedState::parse(&raw).last_seed.and_then(|s| s.parse::<u64>().ok()) {
                    set_at(&mut node.widgets_values, 0, json!(n));
                }
            }
            true
        }
        Err(_) => false,
    }
}

/// Blanket rule for "picker" combos (easy wildcards, easy prompt, wildcardsMatrix, …): the
/// first option is an instruction like "Select the LoRA to add to the text"; the node's JS
/// appends a pick to the text widget and snaps back. Returns that placeholder.
fn picker_placeholder(i: &Input) -> Option<&str> {
    let Kind::Combo(o) = &i.kind else { return None };
    let ph = o.first()?.as_str();
    let l = ph.to_lowercase();
    (o.len() > 1 && l.contains("select") && l.contains("add") && l.contains("text")).then_some(ph)
}

/// Append the pick at `idx` to the text at `t` and reset the combo to `placeholder`.
/// LoRA pickers add `<lora:NAME>` (".safetensors" dropped, no separator, as easy-use does);
/// anything else joins with ", " unless the text is empty or already ends in a separator.
/// ponytail: easy prompt's JS uses " " for prefix/subject/background; ", " is close enough for CLIP.
fn picker_append(n: &mut workflow::Node, idx: usize, t: usize, placeholder: &str) {
    let Some(pick) = n.widgets_values.get(idx).and_then(Value::as_str).filter(|p| *p != placeholder).map(str::to_owned) else { return };
    let mut text = n.widgets_values.get(t).and_then(Value::as_str).unwrap_or_default().to_owned();
    if placeholder.to_lowercase().contains("lora") {
        text += &format!("<lora:{}>", pick.strip_suffix(".safetensors").unwrap_or(&pick));
    } else {
        if !text.is_empty() && !text.ends_with([',', ' ', '\n']) {
            text += ", ";
        }
        text += &pick;
    }
    set_at(&mut n.widgets_values, t, Value::String(text));
    set_at(&mut n.widgets_values, idx, Value::String(placeholder.to_owned()));
}

pub(crate) fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

/// Apply every `control_after_generate` (fixed / increment / decrement / randomize), as the
/// web frontend does after queueing. Muted/bypassed nodes are left alone.
/// ponytail: subgraph-internal nodes bump even when their host is bypassed; track instances if that matters.
pub fn bump_controls(wf: &mut Workflow, reg: &Registry) {
    let graphs = std::iter::once(&mut wf.root).chain(wf.subgraphs.values_mut().map(|s| &mut s.graph));
    for g in graphs {
        for n in g.nodes.iter_mut().filter(|n| n.mode == 0) {
            let Some(def) = reg.get(&n.ty) else { continue };
            let slots = prompt::widget_slots(n, def);
            for pair in slots.windows(2) {
                let (v, c) = (&pair[0], &pair[1]);
                if c.role != Role::Control || v.role != Role::Value {
                    continue;
                }
                let num = |k: &str, d: f64| v.input.opts[k].as_f64().unwrap_or(d);
                let (min, max) = (num("min", 0.0), num("max", 9007199254740991.0).min(9007199254740991.0));
                let cur = v.value.as_i64().unwrap_or(0);
                let new = match c.value.as_str() {
                    Some("increment") => cur.saturating_add(num("step", 1.0) as i64),
                    Some("decrement") => cur.saturating_sub(num("step", 1.0) as i64),
                    Some("randomize") => min as i64 + (rand_u64() % ((max - min) as u64 + 1)) as i64,
                    _ => continue,
                };
                set_at(&mut n.widgets_values, v.idx, Value::from(new.clamp(min as i64, max as i64)));
            }
        }
    }
}

/// Build the scene for `graph`, which is either the workflow root or a subgraph
/// definition (shown standalone: boundary slots become pseudo-nodes).
/// Like litegraph, the input holding the link id wins over a stale `target_slot`
/// (files saved after a node's inputs were reordered disagree).
fn target_slot(graph: &Graph, l: &workflow::Link) -> usize {
    graph.node(&l.target_id).and_then(|n| n.inputs.iter().position(|i| i.link == Some(l.id))).unwrap_or(l.target_slot)
}

pub fn scene(
    wf: &Workflow,
    graph: &Graph,
    def: Option<&Subgraph>,
    reg: &Registry,
) -> (Vec<Node>, Vec<canvas::Link>, Vec<Group>) {
    let mut nodes: Vec<Node> = Vec::with_capacity(graph.nodes.len() + 2);
    let mut index: HashMap<String, usize> = HashMap::new();

    // Subgraph boundary pseudo-nodes (litegraph draws them as two-column nodes).
    let pseudo = |id: &str, io: &[crate::workflow::SubgraphIo], rect: [f64; 4], out: bool| Node {
        id: id.to_owned(),
        title: if out { "Outputs" } else { "Inputs" }.to_owned(),
        pos: (rect[0], rect[1]),
        size: (rect[2], rect[3]),
        inputs: if out { socket(io) } else { Vec::new() },
        outputs: if out { Vec::new() } else { socket(io) },
        color: Some(0x2a6a3a),
        ..Node::default()
    };
    if let Some(d) = def {
        let mut add = |id: &str, io: &[crate::workflow::SubgraphIo], rect: Option<[f64; 4]>, out: bool| {
            let Some(rect) = rect else { return };
            index.insert(id.to_owned(), nodes.len());
            nodes.push(pseudo(id, io, rect, out));
        };
        add(SUBGRAPH_INPUT_ID, &d.inputs, d.input_node, false);
        add(SUBGRAPH_OUTPUT_ID, &d.outputs, d.output_node, true);
        // The always-present "plug a new input here" socket below the last one.
        if let Some(&i) = index.get(SUBGRAPH_INPUT_ID) {
            nodes[i].outputs.push(Slot { label: String::new(), ty: String::new() });
        }
    }

    for n in &graph.nodes {
        let node_def = reg.get(&n.ty);
        // Subgraph instances have the definition uuid as `type`; litegraph titles them by the definition name.
        let title = n
            .title
            .clone()
            .or_else(|| wf.subgraphs.get(&n.ty).map(|s| s.name.clone()))
            .or_else(|| node_def.map(|d| d.display_name.clone()))
            .unwrap_or_else(|| n.ty.clone());
        let mut images = Vec::new();
        let widgets = build_widgets(wf, graph, n, reg, &mut images);
        // Saved sizes can be tiny/garbage in old files; `Node::relayout` grows the height to fit.
        let size = (n.size.0.max(120.0), n.size.1.max(40.0));
        let mut inputs: Vec<Slot> = n
            .inputs
            .iter()
            .map(|i| Slot {
                label: i.widget.clone().filter(|w| *w != i.name).unwrap_or_else(|| i.name.clone()),
                ty: i.ty.clone(),
            })
            .collect();
        let base = inputs.len();
        // The web UI gives every declared widget an input socket when it loads a file; the
        // file only stores those once converted, so old files lose the dot. Add them back.
        if let Some(d) = node_def {
            for i in d.inputs.iter().filter(|i| i.is_widget()) {
                if !inputs.iter().any(|s| s.label == i.name) {
                    inputs.push(Slot { label: i.name.clone(), ty: i.type_name().to_owned() });
                }
            }
        }
        // Load Image masks (`clipspace/…`) live in the picture's alpha: tint them like the web preview.
        let overlay = bbox_overlay(n, reg).or_else(|| {
            let clip = n.ty != "ImageBBoxLayout" && images.first().is_some_and(|q| q.contains("subfolder=clipspace"));
            clip.then(|| canvas::Overlay { alpha_mask: true, ..Default::default() })
        });
        let mut node = Node {
            id: n.id.clone(),
            title,
            pos: n.pos,
            size,
            inputs,
            outputs: n.outputs.iter().map(|o| Slot { label: o.name.clone(), ty: o.ty.clone() }).collect(),
            widgets,
            images,
            overlay,
            color: n.color.as_deref().and_then(parse_color),
            bgcolor: n.bgcolor.as_deref().and_then(parse_color),
            mode: n.mode,
            collapsed: n.collapsed,
            pinned: n.pinned,
            subgraph: wf.subgraphs.contains_key(&n.ty),
            shape: n.shape,
            ..Node::default()
        };
        // A widget sits on the socket whose name it carries, so its dot is on that row.
        for w in &mut node.widgets {
            if w.input.is_none() {
                w.input = node.inputs.iter().position(|s| s.label == w.label);
            }
        }
        grow_widget_slots(&mut node);
        prune_extra_slots(&mut node, base);
        index.insert(n.id.clone(), nodes.len());
        nodes.push(node);
    }

    let links = graph
        .links
        .values()
        .filter_map(|l| {
            let a = *index.get(l.origin_id.as_str())?;
            let b = *index.get(l.target_id.as_str())?;
            Some(Link { from: a, from_slot: l.origin_slot, to: b, to_slot: target_slot(graph, l) })
        })
        .collect();

    let groups = graph
        .groups
        .iter()
        .map(|g| Group { title: g.title.clone(), rect: g.bounding, color: g.color.as_deref().and_then(parse_color) })
        .collect();

    (nodes, links, groups)
}

/// Drop the sockets added for declared widgets that a special-cased builder never drew.
fn prune_extra_slots(n: &mut Node, base: usize) {
    let mut i = base;
    while i < n.inputs.len() {
        if n.widgets.iter().any(|w| w.input == Some(i)) {
            i += 1;
            continue;
        }
        n.inputs.remove(i);
        for w in &mut n.widgets {
            if let Some(k) = w.input.filter(|k| *k > i) {
                w.input = Some(k - 1);
            }
        }
    }
}

/// rgthree's Power Lora rows are built in JS, so the file has no input behind them and
/// the row gets no socket. Name one per row like the widget (`lora_1`…, any type) so the
/// row can be linked; `sync` materialises the real `Input` once a wire lands on it.
fn grow_widget_slots(n: &mut Node) {
    let mut row = 0;
    for w in &mut n.widgets {
        if !w.key.starts_with("lora:") { continue; }
        row += 1;
        if w.input.is_some() { continue; }
        let name = format!("lora_{row}");
        let idx = match n.inputs.iter().position(|s| s.label == name) {
            Some(i) => i,
            None => {
                n.inputs.push(Slot { label: name, ty: "*".to_owned() });
                n.inputs.len() - 1
            }
        };
        w.input = Some(idx);
    }
}

/// Write the edited scene back into `graph` (positions + wiring), keeping link ids
/// of untouched wires. Links touching the subgraph boundary (-10 / -20) survive,
/// since those pseudo-nodes are not in `graph.nodes`.
pub fn sync(graph: &mut Graph, nodes: &[Node], links: &[Link], groups: &[Group]) {
    for (i, n) in nodes.iter().enumerate() {
        if let Some(w) = graph.nodes.iter_mut().find(|w| w.id == n.id) {
            w.pos = n.pos;
            w.size = n.size;
            w.collapsed = n.collapsed;
            // A wire on a JS-built widget socket (LoRA rows) needs a real input, otherwise
            // the prompt and the saved file would drop it.
            let wired = links.iter().filter(|l| l.to == i).map(|l| l.to_slot + 1).max().unwrap_or(0);
            let end = wired.min(n.inputs.len());
            if end > w.inputs.len() {
                let extra: Vec<crate::workflow::Input> = n.inputs[w.inputs.len()..end]
                    .iter()
                    .map(|s| crate::workflow::Input {
                        name: s.label.clone(),
                        ty: s.ty.clone(),
                        link: None,
                        widget: None,
                        label: None,
                    })
                    .collect();
                w.inputs.extend(extra);
            }
        }
    }
    for (g, s) in graph.groups.iter_mut().zip(groups) {
        g.bounding = s.rect;
    }
    let mut slot_of: HashMap<String, usize> = HashMap::with_capacity(graph.nodes.len());
    for (i, n) in graph.nodes.iter().enumerate() {
        slot_of.insert(n.id.clone(), i);
    }

    let mut ids: HashMap<(String, usize, String, usize), i64> = graph
        .links
        .values()
        .map(|l| ((l.origin_id.clone(), l.origin_slot, l.target_id.clone(), target_slot(graph, l)), l.id))
        .collect();
    let mut next = graph.links.keys().copied().max().unwrap_or(0) + 1;
    for n in &mut graph.nodes {
        for i in &mut n.inputs {
            i.link = None;
        }
        for o in &mut n.outputs {
            o.links.clear();
        }
    }

    let mut wired = HashMap::with_capacity(links.len());
    for l in links {
        let (Some(from), Some(to)) = (nodes.get(l.from), nodes.get(l.to)) else { continue };
        let id = match ids.remove(&(from.id.clone(), l.from_slot, to.id.clone(), l.to_slot)) {
            Some(id) => id,
            None => {
                next += 1;
                next - 1
            }
        };
        if let Some(&i) = slot_of.get(&from.id) {
            if let Some(o) = graph.nodes[i].outputs.get_mut(l.from_slot) {
                o.links.push(id);
            }
        }
        if let Some(&i) = slot_of.get(&to.id) {
            if let Some(inp) = graph.nodes[i].inputs.get_mut(l.to_slot) {
                inp.link = Some(id);
            }
        }
        let ty = from.outputs.get(l.from_slot).map(|s| s.ty.clone()).unwrap_or_default();
        wired.insert(
            id,
            workflow::Link {
                id,
                origin_id: from.id.clone(),
                origin_slot: l.from_slot,
                target_id: to.id.clone(),
                target_slot: l.to_slot,
                ty,
            },
        );
    }
    // ComfyUI stores an output's link ids in ascending order; keep it that way so a
    // save produces no diff noise in the user's own files.
    for n in &mut graph.nodes {
        for o in &mut n.outputs {
            o.links.sort_unstable();
        }
    }
    graph.links = wired;
}

/// A wire dropped on the Inputs node's trailing bare socket (`ty: ""` accepts anything)
/// becomes a real subgraph input, named after the node input it feeds and mirrored onto
/// every instance, so the new parameter shows up on the subgraph node.
pub fn grow_inputs(wf: &mut Workflow, id: &str) {
    let Some(sg) = wf.subgraphs.get(id) else { return };
    let Some(want) = sg
        .graph
        .links
        .values()
        .filter(|l| l.origin_id == SUBGRAPH_INPUT_ID)
        .map(|l| l.origin_slot + 1)
        .max()
    else {
        return;
    };
    if want <= sg.inputs.len() {
        return;
    }
    let mut fresh: Vec<(String, String)> = Vec::new();
    for i in sg.inputs.len()..want {
        let src = sg.graph.links.values().find(|l| l.origin_id == SUBGRAPH_INPUT_ID && l.origin_slot == i);
        let (base, ty) = src
            .and_then(|l| sg.graph.node(&l.target_id).and_then(|n| n.inputs.get(l.target_slot)).map(|inp| (inp.name.clone(), inp.ty.clone())))
            .unwrap_or((format!("input{i}"), String::new()));
        let mut name = base.clone();
        let mut k = 2;
        while sg.inputs.iter().any(|io| io.name == name) || fresh.iter().any(|(n, _)| *n == name) {
            name = format!("{base}_{k}");
            k += 1;
        }
        fresh.push((name, ty));
    }
    if let Some(sg) = wf.subgraphs.get_mut(id) {
        for (i, (name, ty)) in fresh.iter().enumerate() {
            let slot = sg.inputs.len() + i;
            for l in sg.graph.links.values_mut().filter(|l| l.origin_id == SUBGRAPH_INPUT_ID && l.origin_slot == slot) {
                l.ty = ty.clone();
            }
            sg.inputs.push(SubgraphIo { name: name.clone(), ty: ty.clone(), link_ids: Vec::new() });
            sg.auto.push(name.clone());
        }
    }
    let mut graphs: Vec<&mut crate::workflow::Graph> = vec![&mut wf.root];
    graphs.extend(wf.subgraphs.values_mut().map(|s| &mut s.graph));
    for g in graphs {
        for n in g.nodes.iter_mut().filter(|n| n.ty == id) {
            for (name, ty) in &fresh {
                n.inputs.push(crate::workflow::Input { name: name.clone(), ty: ty.clone(), link: None, widget: None, label: None });
            }
        }
    }
}

/// Drop subgraph inputs that only exist because a wire was dropped on the Inputs node,
/// once their last link is severed: the Inputs node row and the host node's socket go
/// away again, so the parameter does not linger after a disconnect.
pub fn prune_inputs(wf: &mut Workflow, id: &str) -> bool {
    let Some(sg) = wf.subgraphs.get(id) else { return false };
    let dead: Vec<usize> = sg
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, io)| sg.auto.iter().any(|a| *a == io.name) && io.link_ids.is_empty())
        .map(|(i, _)| i)
        .collect();
    if dead.is_empty() {
        return false;
    }
    let names: Vec<String>;
    {
        let Some(sg) = wf.subgraphs.get_mut(id) else { return false };
        names = dead.iter().filter_map(|&i| sg.inputs.get(i).map(|io| io.name.clone())).collect();
        for &i in dead.iter().rev() {
            if i < sg.inputs.len() {
                sg.inputs.remove(i);
            }
        }
        sg.auto.retain(|a| !names.contains(a));
        // Slots to the right of a removed input move left; the remaining wires follow.
        for l in sg.graph.links.values_mut().filter(|l| l.origin_id == SUBGRAPH_INPUT_ID) {
            let below = dead.iter().filter(|&&d| d < l.origin_slot).count();
            l.origin_slot = l.origin_slot.saturating_sub(below);
        }
    }
    // The parameters are mirrored onto every instance node, where they are now unused.
    let mut graphs: Vec<&mut Graph> = vec![&mut wf.root];
    graphs.extend(wf.subgraphs.values_mut().map(|s| &mut s.graph));
    for g in graphs {
        for n in g.nodes.iter_mut().filter(|n| n.ty == id) {
            n.inputs.retain(|inp| inp.link.is_some() || !names.contains(&inp.name));
        }
    }
    true
}

/// Swap two entries of a node's `widgets_values` (Power Lora rows move up / down).
pub fn swap_widget_rows(wf: &mut Workflow, path: &[String], node_id: &str, a: usize, b: usize) -> bool {
    let graph = match path.last() {
        Some(p) => match wf.subgraphs.get_mut(p) {
            Some(sg) => &mut sg.graph,
            None => return false,
        },
        None => &mut wf.root,
    };
    let Some(n) = graph.nodes.iter_mut().find(|n| n.id == node_id) else { return false };
    let Value::Array(v) = &mut n.widgets_values else { return false };
    if a.max(b) >= v.len() {
        return false;
    }
    v.swap(a, b);
    true
}

/// Rebuild the subgraph's boundary `linkIds` from the wiring (they must agree).
pub fn sync_io(sg: &mut Subgraph) {
    let links = &sg.graph.links;
    for (i, io) in sg.inputs.iter_mut().enumerate() {
        io.link_ids = links
            .values()
            .filter(|l| l.origin_id == SUBGRAPH_INPUT_ID && l.origin_slot == i)
            .map(|l| l.id)
            .collect();
    }
    for (i, io) in sg.outputs.iter_mut().enumerate() {
        io.link_ids = links
            .values()
            .filter(|l| l.target_id == SUBGRAPH_OUTPUT_ID && l.target_slot == i)
            .map(|l| l.id)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wf() -> Workflow {
        Workflow::from_json(json!({
            "nodes": [
                {"id": 1, "type": "A", "pos": [0, 0], "inputs": [], "outputs": [{"name": "a", "type": "IMAGE", "links": [7]}]},
                {"id": 2, "type": "B", "pos": [300, 0], "inputs": [{"name": "x", "type": "IMAGE", "link": 7}], "outputs": []}
            ],
            "links": [[7, 1, 0, 2, 0, "IMAGE"]],
            "groups": []
        }))
        .unwrap()
    }

    #[test]
    fn sync_reuses_ids_and_rewires() {
        let mut w = wf();
        let mut nodes = vec![
            Node { id: "1".into(), pos: (0.0, 0.0), ..Default::default() },
            Node { id: "2".into(), pos: (300.0, 0.0), ..Default::default() },
        ];
        nodes[1].pos = (400.0, 50.0);
        let mut links = vec![Link { from: 0, from_slot: 0, to: 1, to_slot: 0 }];
        sync(&mut w.root, &nodes, &links, &[]);
        assert_eq!(w.root.nodes[1].pos, (400.0, 50.0));
        assert_eq!(w.root.links.len(), 1);
        assert_eq!(w.root.links[&7].target_id, "2"); // id kept for an unchanged endpoint pair
        assert_eq!(w.root.nodes[0].outputs[0].links, vec![7]);
        assert_eq!(w.root.nodes[1].inputs[0].link, Some(7));

        links.clear();
        sync(&mut w.root, &nodes, &links, &[]);
        assert!(w.root.links.is_empty());
        assert_eq!(w.root.nodes[1].inputs[0].link, None);
        assert!(w.root.nodes[0].outputs[0].links.is_empty());
    }

    #[test]
    fn sync_keeps_boundary_links() {
        let mut w = Workflow::from_json(json!({
            "nodes": [],
            "links": [],
            "definitions": {"subgraphs": [{
                "id": "u1", "name": "Sub", "nodes": [], "links": [],
                "inputs": [{"name": "in", "type": "IMAGE", "linkIds": [3]}],
                "outputs": []
            }]}
        }))
        .unwrap();
        let sg = w.subgraphs.get_mut("u1").unwrap();
        sync(&mut sg.graph, &[], &[], &[]);
        sync_io(sg);
        assert!(sg.inputs[0].link_ids.is_empty(), "stale boundary link should be dropped");
    }

    #[test]
    fn lora_rows_get_sockets_and_a_wire_materialises_an_input() {
        let mut w = Workflow::from_json(json!({
            "nodes": [
                {"id": 1, "type": "Power Lora Loader (rgthree)", "pos": [0, 0], "size": [300, 100],
                 "inputs": [{"name": "model", "type": "MODEL", "link": null}, {"name": "clip", "type": "CLIP", "link": null}],
                 "outputs": [], "widgets_values": [{}, {"type": "PowerLoraLoaderHeaderWidget"},
                    {"on": true, "lora": "a", "strength": 1}, {"on": true, "lora": "b", "strength": 1}, {}, ""]},
                {"id": 2, "type": "KSampler", "pos": [400, 0], "size": [300, 100],
                 "outputs": [{"name": "MODEL", "type": "MODEL", "links": null}], "widgets_values": []}
            ],
            "links": []
        }))
        .unwrap();
        let reg = Registry::default();
        let (nodes, mut links, groups) = scene(&w, &w.root, None, &reg);
        let to = nodes.iter().position(|n| n.id == "1").unwrap();
        let from = nodes.iter().position(|n| n.id == "2").unwrap();
        let lora = &nodes[to];
        assert_eq!(lora.inputs.len(), 4, "model, clip + one socket per LoRA row");
        assert_eq!((lora.inputs[2].label.as_str(), lora.inputs[3].label.as_str()), ("lora_1", "lora_2"));
        // The row's dot hangs on the row itself, so a wire lands exactly where the widget is.
        assert!(lora.widgets.iter().any(|w| w.key == "lora:3" && w.input == Some(3)));
        links.push(Link { from, from_slot: 0, to, to_slot: 3 });
        sync(&mut w.root, &nodes, &links, &groups);
        let m = w.root.nodes.iter().find(|n| n.id == "1").unwrap();
        assert_eq!(m.inputs.len(), 4, "the wire needs a real input, or the prompt would drop it");
        assert_eq!(m.inputs[3].name, "lora_2");
        assert!(m.inputs[3].link.is_some());
    }

    #[test]
    fn stale_target_slot_follows_the_input_holding_the_link() {
        let mut w = Workflow::from_json(json!({
            "nodes": [
                {"id": 1, "type": "A", "pos": [0, 0], "size": [100, 100], "outputs": [{"name": "s", "type": "STRING", "links": [7]}]},
                {"id": 2, "type": "B", "pos": [200, 0], "size": [100, 100],
                 "inputs": [{"name": "seed", "type": "INT", "link": null}, {"name": "words", "type": "STRING", "link": 7}]}
            ],
            "links": [[7, 1, 0, 2, 0, "STRING"]]
        }))
        .unwrap();
        let (nodes, links, groups) = scene(&w, &w.root, None, &Registry::default());
        assert_eq!(links[0].to_slot, 1);
        sync(&mut w.root, &nodes, &links, &groups);
        let n = w.root.node("2").unwrap();
        assert_eq!((n.inputs[0].link, n.inputs[1].link), (None, Some(7)));
        assert_eq!(w.root.links[&7].target_slot, 1);
    }

    #[test]
    fn lora_rows_edit_in_place() {
        let mut wf = Workflow::from_json(json!({"nodes": [{"id": 1, "type": "Power Lora Loader (rgthree)", "pos": [0, 0], "size": [300, 100],
            "widgets_values": [{}, {"type": "PowerLoraLoaderHeaderWidget"}, {"on": true, "lora": "a", "strength": 1}, {}, ""]}], "links": []})).unwrap();
        assert!(apply_widget(&mut wf, &[], "1", "lora+", Value::Null, "b"));
        let v = wf.root.nodes[0].widgets_values.clone();
        assert_eq!(v.as_array().unwrap().len(), 6);
        assert_eq!(v[3]["lora"], "b");
        assert!(apply_widget(&mut wf, &[], "1", "lora:2", Value::Null, ""));
        assert_eq!(wf.root.nodes[0].widgets_values[2]["lora"], "b");
        assert!(apply_widget(&mut wf, &[], "1", "7", json!(5), ""));
        assert_eq!(wf.root.nodes[0].widgets_values.as_array().unwrap().len(), 8);
    }

    #[test]
    fn controls_bump_seed() {
        let reg = Registry::from_object_info(&json!({"KSampler": {"input": {"required": {"seed": ["INT", {"default": 0, "min": 0, "max": 100}]}}, "output": [], "name": "KSampler", "display_name": "K", "category": "x"}}));
        let mut wf = Workflow::from_json(json!({"nodes": [{"id": 1, "type": "KSampler", "pos": [0, 0], "size": [1, 1], "widgets_values": [5, "increment"]}], "links": []})).unwrap();
        bump_controls(&mut wf, &reg);
        assert_eq!(wf.root.nodes[0].widgets_values, json!([6, "increment"]));
        wf.root.nodes[0].widgets_values = json!([5, "randomize"]);
        bump_controls(&mut wf, &reg);
        assert!(wf.root.nodes[0].widgets_values[0].as_i64().unwrap() <= 100);
    }

    #[test]
    fn view_query_parses_annotation() {
        assert_eq!(image_query("Intosica/a b.png [output]"), "filename=a%20b.png&subfolder=Intosica&type=output");
        assert_eq!(image_query("x.png"), "filename=x.png&subfolder=&type=input");
    }

    #[test]
    fn lora_manager_tags_round_trip() {
        let text = "a girl, <lora:MOSS/x_v1:0.60> <lora:MOSS/y:0.7> car";
        let tags = lm_tags(text);
        assert_eq!(tags, vec![("MOSS/x_v1".to_string(), 0.60), ("MOSS/y".to_string(), 0.7)]);
    }

    #[test]
    fn showtext_lines_flatten() {
        assert_eq!(show_lines(&json!([["Swiss,", "rain"]])), vec!["Swiss,".to_string(), "rain".to_string()]);
        assert_eq!(show_lines(&json!(["one"])), vec!["one".to_string()]);
    }

    #[test]
    fn wildcards_pick_appends_to_prompt() {
        let mut n = workflow::Node {
            id: "1".into(),
            ty: "easy wildcards".into(),
            title: None,
            pos: (0.0, 0.0),
            size: (1.0, 1.0),
            mode: 0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            widgets_values: json!(["start", "Select the LoRA to add to the text", "Select the Wildcard to add to the text", 1, "randomize", false]),
            properties: json!({}),
            color: None,
            bgcolor: None,
            collapsed: false,
            pinned: false,
            shape: 0,
        };
        let lora = "Select the LoRA to add to the text";
        n.widgets_values[1] = json!("MOSS/x.safetensors");
        picker_append(&mut n, 1, 0, lora);
        assert_eq!(n.widgets_values[0], json!("start<lora:MOSS/x>"));
        assert_eq!(n.widgets_values[1], json!("Select the LoRA to add to the text"));
        n.widgets_values[2] = json!("__detail__");
        picker_append(&mut n, 2, 0, "Select the Wildcard to add to the text");
        assert_eq!(n.widgets_values[2], json!("Select the Wildcard to add to the text"));
        // placeholder pick is a no-op
        picker_append(&mut n, 1, 0, lora);
        assert_eq!(n.widgets_values[0], json!("start<lora:MOSS/x>, __detail__"));
        let opts = |o: &[&str]| Input { name: "x".into(), kind: Kind::Combo(o.iter().map(|s| s.to_string()).collect()), optional: false, opts: json!({}) };
        assert_eq!(picker_placeholder(&opts(&["👤Select the subject add to the text", "a man"])), Some("👤Select the subject add to the text"));
        assert_eq!(picker_placeholder(&opts(&["euler", "dpm"])), None);
        assert_eq!(n.widgets_values[0], json!("start<lora:MOSS/x>, __detail__"));
    }

    #[test]
    fn grow_inputs_adds_slot_and_mirrors_host() {
        let mut w = Workflow::from_json(json!({
            "nodes": [{"id": 5, "type": "sg-uuid", "pos": [0, 0], "inputs": [{"name": "prompt", "type": "STRING", "link": null}], "outputs": []}],
            "links": [],
            "definitions": {"subgraphs": [{
                "id": "sg-uuid", "name": "Inner",
                "nodes": [{"id": 2, "type": "B", "pos": [0, 0],
                           "inputs": [{"name": "image", "type": "IMAGE", "link": 9}], "outputs": []}],
                "links": [{"id": 9, "origin_id": "-10", "origin_slot": 1, "target_id": "2", "target_slot": 0, "type": ""}],
                "inputs": [{"name": "prompt", "type": "STRING", "linkIds": []}],
                "outputs": []
            }]}
        }))
        .unwrap();
        grow_inputs(&mut w, "sg-uuid");
        let sg = &w.subgraphs["sg-uuid"];
        assert_eq!(sg.inputs.len(), 2);
        assert_eq!((sg.inputs[1].name.as_str(), sg.inputs[1].ty.as_str()), ("image", "IMAGE"));
        assert_eq!(sg.graph.links[&9].ty, "IMAGE");
        assert_eq!(w.root.nodes[0].inputs[1].name, "image");
        // A second pass is a no-op.
        grow_inputs(&mut w, "sg-uuid");
        assert_eq!(w.subgraphs["sg-uuid"].inputs.len(), 2);
    }

    #[test]
    fn subgraph_rows_merge_proxies_and_list_unpromoted_interior_widgets() {
        let reg = Registry::from_object_info(&json!({
            "Loader": {"input": {"required": {"clip_name": [["a.safetensors", "b.safetensors"], {}]}},
                       "output": ["CLIP"], "name": "Loader", "display_name": "Load CLIP"},
            "Sampler": {"input": {"required": {"steps": ["INT", {"default": 20}],
                                               "cfg": ["FLOAT", {"default": 7.0}]}},
                        "output": ["LATENT"], "name": "Sampler", "display_name": "KSampler"}
        }));
        let w = Workflow::from_json(json!({
            "nodes": [{
                "id": 446, "type": "sg", "pos": [0, 0], "size": [200, 200],
                "inputs": [{"name": "clip_name", "type": "COMBO", "link": null, "widget": {"name": "clip_name"}}],
                "outputs": [], "widgets_values": ["b.safetensors"],
                "properties": {"proxyWidgets": [["437", "clip_name"], ["438", "steps"]]}
            }],
            "links": [],
            "definitions": {"subgraphs": [{
                "id": "sg", "name": "Sub",
                "nodes": [
                    {"id": 437, "type": "Loader", "pos": [0, 0],
                     "inputs": [{"name": "clip_name", "type": "COMBO", "link": null, "widget": {"name": "clip_name"}}],
                     "widgets_values": ["a.safetensors"]},
                    {"id": 438, "type": "Sampler", "pos": [0, 0],
                     "inputs": [{"name": "steps", "type": "INT", "link": null, "widget": {"name": "steps"}},
                                {"name": "cfg", "type": "FLOAT", "link": null, "widget": {"name": "cfg"}}],
                     "widgets_values": [20, 7.0]}
                ],
                "links": [[1, -10, 0, 437, 0, "COMBO"]],
                "inputs": [{"name": "clip_name", "type": "COMBO", "linkIds": [1]}],
                "outputs": []
            }]}
        }))
        .unwrap();
        let host = w.root.node("446").unwrap().clone();
        let mut images = Vec::new();
        let rows = build_widgets(&w, &w.root, &host, &reg, &mut images);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["clip_name", "steps"]);
        assert_eq!(rows[0].key, "w:0");
        assert_eq!(rows[0].value, json!("b.safetensors"), "the instance value wins");
        assert_eq!(rows[0].source, "Load CLIP");
        assert_eq!(rows[1].key, "p:438:0");
        assert_eq!(rows[1].source, "KSampler");
        let adv = subgraph_advanced_widgets(&w, &host, &reg);
        assert_eq!(adv.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["cfg"]);
        assert_eq!(adv[0].key, "p:438:1");
        assert!(adv[0].advanced);
        assert_eq!(adv[0].default, json!(7.0));
    }
}
