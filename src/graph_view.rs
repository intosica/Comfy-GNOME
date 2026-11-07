// SPDX-License-Identifier: GPL-3.0-only
//! Workflow graph → canvas scene (nodes, links, groups).
//!
//! Positions and sizes come straight from the file (litegraph coordinates).
//! Widget values are read-only text for now; Phase 3 replaces them with real editors.

use crate::canvas::{self, Group, Node, parse_color};
use crate::prompt;
use crate::registry::Registry;
use crate::workflow::{Graph, SUBGRAPH_INPUT_ID, SUBGRAPH_OUTPUT_ID, Subgraph, Workflow};
use serde_json::Value;
use std::collections::HashMap;

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
        inputs: if out { io.iter().map(|i| i.name.clone()).collect() } else { Vec::new() },
        outputs: if out { Vec::new() } else { io.iter().map(|i| i.name.clone()).collect() },
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
            inputs: n.inputs.iter().map(|i| i.widget.clone().filter(|w| *w != i.name).unwrap_or_else(|| i.name.clone())).collect(),
            outputs: n.outputs.iter().map(|o| o.name.clone()).collect(),
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
            Some((a, l.origin_slot, b, l.target_slot))
        })
        .collect();

    let groups = graph
        .groups
        .iter()
        .map(|g| Group { title: g.title.clone(), rect: g.bounding, color: g.color.as_deref().and_then(parse_color) })
        .collect();

    (nodes, links, groups)
}
