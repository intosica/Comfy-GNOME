// SPDX-License-Identifier: GPL-3.0-only
//! Convert a selection to a subgraph and unpack a subgraph instance back into its parent
//! (frontend `LGraph.convertToSubgraph` / `unpackSubgraph`). Wires crossing the selection
//! become subgraph inputs (one per source output) and outputs (one per inner output).

use super::*;
use std::collections::HashSet;

impl Workflow {
    fn graph_at_mut(&mut self, path: Option<&str>) -> Option<&mut Graph> {
        match path {
            Some(p) => self.subgraphs.get_mut(p).map(|s| &mut s.graph),
            None => Some(&mut self.root),
        }
    }

    /// Node JSON of the loaded graph at `path`, so moved nodes keep their unknown fields.
    fn raw_nodes(&self, path: Option<&str>) -> Vec<Value> {
        let g = match path {
            None => Some(&self.raw),
            Some(id) => self.raw["definitions"]["subgraphs"].as_array().and_then(|a| a.iter().find(|d| d["id"].as_str() == Some(id))),
        };
        g.and_then(|g| g["nodes"].as_array()).cloned().unwrap_or_default()
    }

    /// Move nodes `ids` of the graph at `path` into a new subgraph; returns the instance id.
    pub fn convert_to_subgraph(&mut self, path: Option<&str>, ids: &[String]) -> Result<String, String> {
        let sel: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let raw_nodes: Vec<Value> = self.raw_nodes(path).into_iter().filter(|n| id_of(&n["id"]).is_some_and(|i| sel.contains(i.as_str()))).collect();
        let uuid = fresh_uuid();
        let g = self.graph_at_mut(path).ok_or("graph not found")?;
        if !g.nodes.iter().any(|n| sel.contains(n.id.as_str())) {
            return Err("nothing to convert".into());
        }
        // Before the nodes leave: a fresh id must not reuse a moved node's (raw JSON matches by id).
        let inst_id = g.next_id();
        let mut outer_next = g.links.keys().max().copied().unwrap_or(0) + 1;
        let mut ls: Vec<Link> = g.links.values().cloned().collect();
        ls.sort_by_key(|l| l.id);
        let (mut inner, mut incoming, mut outgoing) = (Vec::new(), Vec::new(), Vec::new());
        for l in ls {
            match (sel.contains(l.origin_id.as_str()), sel.contains(l.target_id.as_str())) {
                (true, true) => inner.push(l),
                (false, true) => incoming.push(l),
                (true, false) => outgoing.push(l),
                _ => {}
            }
        }
        for l in incoming.iter().chain(&outgoing).chain(&inner) {
            g.links.remove(&l.id);
        }
        let moved: Vec<Node> = g.nodes.iter().filter(|n| sel.contains(n.id.as_str())).cloned().collect();
        g.nodes.retain(|n| !sel.contains(n.id.as_str()));
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for n in &moved {
            b = [b[0].min(n.pos.0), b[1].min(n.pos.1), b[2].max(n.pos.0 + n.size.0), b[3].max(n.pos.1 + n.size.1)];
        }
        let mut def = Graph { nodes: moved, links: inner.into_iter().map(|l| (l.id, l)).collect(), groups: Vec::new() };
        let mut next = def.links.keys().max().copied().unwrap_or(0) + 1;
        // Group crossing wires by their source output: one boundary slot each.
        let group = |ls: &[Link]| {
            let mut out: Vec<((String, usize), Vec<Link>)> = Vec::new();
            for l in ls {
                let k = (l.origin_id.clone(), l.origin_slot);
                match out.iter_mut().find(|(o, _)| *o == k) {
                    Some((_, v)) => v.push(l.clone()),
                    None => out.push((k, vec![l.clone()])),
                }
            }
            out
        };
        let mut inst = Node {
            id: inst_id.clone(),
            ty: uuid.clone(),
            title: None,
            pos: (b[0], b[1]),
            size: (240.0, 80.0),
            mode: 0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            widgets_values: Value::Array(Vec::new()),
            properties: serde_json::json!({ "proxyWidgets": [] }),
            color: None,
            bgcolor: None,
            collapsed: false,
            pinned: false,
            shape: 0,
        };
        let mut sg_inputs: Vec<SubgraphIo> = Vec::new();
        for (k, ((oid, oslot), links)) in group(&incoming).into_iter().enumerate() {
            let first = &links[0];
            let tn = def.nodes.iter().find(|n| n.id == first.target_id);
            let base = tn.and_then(|n| n.inputs.get(first.target_slot)).map(|i| i.label.clone().unwrap_or_else(|| i.name.clone())).unwrap_or_else(|| first.ty.clone());
            let name = unique_name(&sg_inputs, &base);
            let mut lids = Vec::new();
            for l in &links {
                let id = next;
                next += 1;
                def.links.insert(id, Link { id, origin_id: SUBGRAPH_INPUT_ID.into(), origin_slot: k, target_id: l.target_id.clone(), target_slot: l.target_slot, ty: l.ty.clone() });
                if let Some(i) = def.nodes.iter_mut().find(|n| n.id == l.target_id).and_then(|n| n.inputs.get_mut(l.target_slot)) {
                    i.link = Some(id);
                }
                lids.push(id);
            }
            // Outer side: one wire from the source into the instance's slot `k`.
            let id = outer_next;
            outer_next += 1;
            if let Some(o) = g.nodes.iter_mut().find(|n| n.id == oid).and_then(|n| n.outputs.get_mut(oslot)) {
                o.links.retain(|x| !links.iter().any(|l| l.id == *x));
                o.links.push(id);
            }
            g.links.insert(id, Link { id, origin_id: oid, origin_slot: oslot, target_id: inst_id.clone(), target_slot: k, ty: first.ty.clone() });
            inst.inputs.push(Input { name: name.clone(), ty: first.ty.clone(), link: Some(id), widget: None, label: None });
            sg_inputs.push(SubgraphIo { name, ty: first.ty.clone(), link_ids: lids });
        }
        let mut sg_outputs: Vec<SubgraphIo> = Vec::new();
        for (k, ((oid, oslot), links)) in group(&outgoing).into_iter().enumerate() {
            let ty = links[0].ty.clone();
            let base = def.nodes.iter().find(|n| n.id == oid).and_then(|n| n.outputs.get(oslot)).map(|o| o.name.clone()).unwrap_or_else(|| ty.clone());
            let name = unique_name(&sg_outputs, &base);
            let id = next;
            next += 1;
            def.links.insert(id, Link { id, origin_id: oid.clone(), origin_slot: oslot, target_id: SUBGRAPH_OUTPUT_ID.into(), target_slot: k, ty: ty.clone() });
            if let Some(o) = def.nodes.iter_mut().find(|n| n.id == oid).and_then(|n| n.outputs.get_mut(oslot)) {
                o.links.retain(|x| !links.iter().any(|l| l.id == *x));
                o.links.push(id);
            }
            let mut outs = Vec::new();
            for l in &links {
                let id = outer_next;
                outer_next += 1;
                if let Some(i) = g.nodes.iter_mut().find(|n| n.id == l.target_id).and_then(|n| n.inputs.get_mut(l.target_slot)) {
                    i.link = Some(id);
                }
                g.links.insert(id, Link { id, origin_id: inst_id.clone(), origin_slot: k, target_id: l.target_id.clone(), target_slot: l.target_slot, ty: ty.clone() });
                outs.push(id);
            }
            inst.outputs.push(Output { name: name.clone(), ty: ty.clone(), links: outs });
            sg_outputs.push(SubgraphIo { name, ty, link_ids: vec![id] });
        }
        g.nodes.push(inst);
        let (input_node, output_node) = ([b[0] - 200.0, b[1], 120.0, 60.0], [b[2] + 80.0, b[1], 120.0, 60.0]);
        let max_node = def.nodes.iter().filter_map(|n| n.id.parse::<i64>().ok()).max().unwrap_or(0);
        let raw = serde_json::json!({
            "id": uuid,
            "version": 1,
            "state": { "lastGroupId": 0, "lastNodeId": max_node, "lastLinkId": next - 1, "lastRerouteId": 0 },
            "revision": 0,
            "config": {},
            "name": "New Subgraph",
            "inputNode": { "id": -10, "bounding": input_node },
            "outputNode": { "id": -20, "bounding": output_node },
            "inputs": [],
            "outputs": sg_outputs.iter().map(|o| serde_json::json!({
                "id": fresh_uuid(), "name": o.name, "type": o.ty, "linkIds": o.link_ids, "localized_name": o.name, "pos": [0, 0]
            })).collect::<Vec<_>>(),
            "widgets": [],
            "nodes": raw_nodes,
            "groups": [],
            "links": [],
            "extra": {},
        });
        if !self.raw.is_object() {
            self.raw = serde_json::json!({});
        }
        if !self.raw["definitions"]["subgraphs"].is_array() {
            self.raw["definitions"]["subgraphs"] = Value::Array(Vec::new());
        }
        if let Some(a) = self.raw["definitions"]["subgraphs"].as_array_mut() {
            a.push(raw);
        }
        self.subgraphs.insert(
            uuid,
            Subgraph {
                name: "New Subgraph".into(),
                graph: def,
                inputs: sg_inputs,
                outputs: sg_outputs,
                input_node: Some(input_node),
                output_node: Some(output_node),
                auto: Vec::new(),
            },
        );
        Ok(inst_id)
    }

    /// Replace instance `id` (graph at `path`) by a copy of its subgraph's nodes. A promoted
    /// widget's value moves onto the inner widget through `set_widget(node, widget, value)`.
    /// Returns the new node ids.
    pub fn unpack_subgraph(&mut self, path: Option<&str>, id: &str, set_widget: impl Fn(&mut Node, &str, Value)) -> Result<Vec<String>, String> {
        let g = match path {
            Some(p) => &self.subgraphs.get(p).ok_or("graph not found")?.graph,
            None => &self.root,
        };
        let inst = g.node(id).ok_or("node not found")?.clone();
        let sg = self.subgraphs.get(&inst.ty).ok_or("not a subgraph")?.clone();
        let g = self.graph_at_mut(path).ok_or("graph not found")?;
        let mut b = [f64::MAX, f64::MAX];
        for n in &sg.graph.nodes {
            b = [b[0].min(n.pos.0), b[1].min(n.pos.1)];
        }
        let mut next_node = g.next_id().parse::<i64>().unwrap_or(1);
        let mut next_link = g.links.keys().max().copied().unwrap_or(0) + 1;
        let mut map: HashMap<String, String> = HashMap::new();
        let mut fresh = Vec::new();
        for n in &sg.graph.nodes {
            let mut n = n.clone();
            let nid = next_node.to_string();
            next_node += 1;
            map.insert(n.id.clone(), nid.clone());
            n.id = nid.clone();
            n.pos = (n.pos.0 - b[0] + inst.pos.0, n.pos.1 - b[1] + inst.pos.1);
            n.inputs.iter_mut().for_each(|i| i.link = None);
            n.outputs.iter_mut().for_each(|o| o.links.clear());
            fresh.push(nid);
            g.nodes.push(n);
        }
        // Outer wires of the instance go with it.
        let outer: Vec<Link> = g.links.values().filter(|l| l.origin_id == inst.id || l.target_id == inst.id).cloned().collect();
        for l in &outer {
            g.links.remove(&l.id);
            if let Some(o) = g.nodes.iter_mut().find(|n| n.id == l.origin_id).and_then(|n| n.outputs.get_mut(l.origin_slot)) {
                o.links.retain(|x| *x != l.id);
            }
        }
        g.nodes.retain(|n| n.id != inst.id);
        let mut connect = |g: &mut Graph, from: &str, fs: usize, to: &str, ts: usize, ty: &str| {
            let id = next_link;
            next_link += 1;
            g.links.insert(id, Link { id, origin_id: from.into(), origin_slot: fs, target_id: to.into(), target_slot: ts, ty: ty.into() });
            if let Some(o) = g.nodes.iter_mut().find(|n| n.id == from).and_then(|n| n.outputs.get_mut(fs)) {
                o.links.push(id);
            }
            if let Some(i) = g.nodes.iter_mut().find(|n| n.id == to).and_then(|n| n.inputs.get_mut(ts)) {
                i.link = Some(id);
            }
        };
        let mut ls: Vec<&Link> = sg.graph.links.values().collect();
        ls.sort_by_key(|l| l.id);
        for l in ls {
            if l.origin_id == SUBGRAPH_INPUT_ID {
                let Some(to) = map.get(&l.target_id) else { continue };
                let Some(io) = sg.inputs.get(l.origin_slot) else { continue };
                let Some(h) = inst.inputs.iter().position(|i| i.name == io.name) else { continue };
                match outer.iter().find(|o| o.target_id == inst.id && o.target_slot == h) {
                    Some(o) => connect(g, &o.origin_id, o.origin_slot, to, l.target_slot, &l.ty),
                    None if inst.inputs[h].widget.is_some() => {
                        let k = inst.inputs[..h].iter().filter(|i| i.widget.is_some()).count();
                        let v = inst.widgets_values.get(k).cloned().filter(|v| !v.is_null());
                        let n = g.nodes.iter_mut().find(|n| &n.id == to);
                        let w = n.as_ref().and_then(|n| n.inputs.get(l.target_slot)).and_then(|i| i.widget.clone());
                        if let (Some(n), Some(w), Some(v)) = (n, w, v) {
                            set_widget(n, &w, v);
                        }
                    }
                    None => {}
                }
            } else if l.target_id == SUBGRAPH_OUTPUT_ID {
                let Some(from) = map.get(&l.origin_id) else { continue };
                for o in outer.iter().filter(|o| o.origin_id == inst.id && o.origin_slot == l.target_slot) {
                    connect(g, from, l.origin_slot, &o.target_id, o.target_slot, &l.ty);
                }
            } else if let (Some(from), Some(to)) = (map.get(&l.origin_id), map.get(&l.target_id)) {
                connect(g, from, l.origin_slot, to, l.target_slot, &l.ty);
            }
        }
        Ok(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_then_unpack_keeps_wiring() {
        let v = serde_json::json!({
            "nodes": [
                {"id": 1, "type": "A", "pos": [0, 0], "size": [100, 50], "outputs": [{"name": "o", "type": "X", "links": [1]}]},
                {"id": 2, "type": "B", "pos": [200, 0], "size": [100, 50], "inputs": [{"name": "i", "type": "X", "link": 1}], "outputs": [{"name": "o", "type": "Y", "links": [2]}]},
                {"id": 3, "type": "C", "pos": [400, 0], "size": [100, 50], "inputs": [{"name": "i", "type": "Y", "link": 2}]}
            ],
            "links": [[1, 1, 0, 2, 0, "X"], [2, 2, 0, 3, 0, "Y"]]
        });
        let mut wf = Workflow::from_json(v).unwrap();
        let inst = wf.convert_to_subgraph(None, &["2".into()]).unwrap();
        assert_eq!(inst, "4");
        let n = wf.root.node(&inst).unwrap();
        let sg = &wf.subgraphs[&n.ty];
        assert_eq!((sg.inputs.len(), sg.outputs.len(), sg.graph.links.len()), (1, 1, 2));
        assert_eq!(wf.root.links.len(), 2);
        assert!(wf.root.links.values().any(|l| l.origin_id == "1" && l.target_id == "4"));
        assert!(wf.root.links.values().any(|l| l.origin_id == "4" && l.target_id == "3"));
        // Round-trips through the file format.
        let back = Workflow::from_json(wf.to_json()).unwrap();
        assert_eq!(back.subgraphs[&n.ty].graph.nodes.len(), 1);
        let fresh = wf.unpack_subgraph(None, &inst, |_, _, _| {}).unwrap();
        assert_eq!(fresh.len(), 1);
        let b = &fresh[0];
        assert!(wf.root.links.values().any(|l| l.origin_id == "1" && &l.target_id == b));
        assert!(wf.root.links.values().any(|l| &l.origin_id == b && l.target_id == "3"));
        assert_eq!(wf.root.links.len(), 2);
    }
}
