// SPDX-License-Identifier: GPL-3.0-only
//! Workflow graph → canvas scene (nodes, links, groups).
//!
//! Positions and sizes come straight from the file (litegraph coordinates).
//! Widget values are read-only text for now; Phase 3 replaces them with real editors.

use crate::canvas::{self, Group, Link, Node, Slot, parse_color};
use crate::prompt;
use crate::registry::Registry;
use crate::workflow::{self, Graph, SUBGRAPH_INPUT_ID, SUBGRAPH_OUTPUT_ID, Subgraph, SubgraphIo, Workflow};
use serde_json::Value;
use std::collections::HashMap;

fn socket(io: &[SubgraphIo]) -> Vec<Slot> {
    io.iter().map(|i| Slot { label: i.name.clone(), ty: i.ty.clone() }).collect()
}

/// Serialize one widget value the way the canvas shows it.
fn show(v: &Value) -> String {
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
        v => v.to_string(),
    }
}

/// Build the scene for `graph`, which is either the workflow root or a subgraph
/// definition (shown standalone: boundary slots become pseudo-nodes).
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
    }

    for n in &graph.nodes {
        let node_def = reg.get(&n.ty);
        let title = n.title.clone().or_else(|| node_def.map(|d| d.display_name.clone())).unwrap_or_else(|| n.ty.clone());
        let widgets = match node_def {
            Some(d) => prompt::widget_map(n, d).into_iter().map(|(k, v)| (k, show(&v))).collect(),
            None => Vec::new(),
        };
        // Saved sizes can be tiny/garbage in old files, and extension widgets may have
        // been added since saving: never shrink, but grow to fit slots + widget rows.
        let slots = n.inputs.len().max(n.outputs.len()) as f64;
        let needed = slots * 20.0 + widgets.len() as f64 * 24.0 + 8.0;
        let size = (n.size.0.max(120.0), n.size.1.max(needed).max(40.0));
        index.insert(n.id.clone(), nodes.len());
        nodes.push(Node {
            id: n.id.clone(),
            title,
            pos: n.pos,
            size,
            inputs: n
                .inputs
                .iter()
                .map(|i| Slot {
                    label: i.widget.clone().filter(|w| *w != i.name).unwrap_or_else(|| i.name.clone()),
                    ty: i.ty.clone(),
                })
                .collect(),
            outputs: n.outputs.iter().map(|o| Slot { label: o.name.clone(), ty: o.ty.clone() }).collect(),
            widgets,
            color: n.color.as_deref().and_then(parse_color),
            bgcolor: n.bgcolor.as_deref().and_then(parse_color),
            mode: n.mode,
            collapsed: n.collapsed,
            subgraph: wf.subgraphs.contains_key(&n.ty),
        });
    }

    let links = graph
        .links
        .values()
        .filter_map(|l| {
            let a = *index.get(l.origin_id.as_str())?;
            let b = *index.get(l.target_id.as_str())?;
            Some(Link { from: a, from_slot: l.origin_slot, to: b, to_slot: l.target_slot })
        })
        .collect();

    let groups = graph
        .groups
        .iter()
        .map(|g| Group { title: g.title.clone(), rect: g.bounding, color: g.color.as_deref().and_then(parse_color) })
        .collect();

    (nodes, links, groups)
}

/// Write the edited scene back into `graph` (positions + wiring), keeping link ids
/// of untouched wires. Links touching the subgraph boundary (-10 / -20) survive,
/// since those pseudo-nodes are not in `graph.nodes`.
pub fn sync(graph: &mut Graph, nodes: &[Node], links: &[Link]) {
    for n in nodes {
        if let Some(w) = graph.nodes.iter_mut().find(|w| w.id == n.id) {
            w.pos = n.pos;
        }
    }
    let mut slot_of: HashMap<String, usize> = HashMap::with_capacity(graph.nodes.len());
    for (i, n) in graph.nodes.iter().enumerate() {
        slot_of.insert(n.id.clone(), i);
    }

    let mut ids: HashMap<(String, usize, String, usize), i64> = graph
        .links
        .values()
        .map(|l| ((l.origin_id.clone(), l.origin_slot, l.target_id.clone(), l.target_slot), l.id))
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
        sync(&mut w.root, &nodes, &links);
        assert_eq!(w.root.nodes[1].pos, (400.0, 50.0));
        assert_eq!(w.root.links.len(), 1);
        assert_eq!(w.root.links[&7].target_id, "2"); // id kept for an unchanged endpoint pair
        assert_eq!(w.root.nodes[0].outputs[0].links, vec![7]);
        assert_eq!(w.root.nodes[1].inputs[0].link, Some(7));

        links.clear();
        sync(&mut w.root, &nodes, &links);;
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
        sync(&mut sg.graph, &[], &[]);
        sync_io(sg);
        assert!(sg.inputs[0].link_ids.is_empty(), "stale boundary link should be dropped");
    }
}
