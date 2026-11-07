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
    pub color: Option<String>,
    pub bgcolor: Option<String>,
    pub collapsed: bool,
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

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
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
    pub id: String,
    pub name: String,
    pub graph: Graph,
    pub inputs: Vec<SubgraphIo>,
    pub outputs: Vec<SubgraphIo>,
    /// Canvas rects [x, y, w, h] of the boundary input / output pseudo-nodes.
    pub input_node: Option<[f64; 4]>,
    pub output_node: Option<[f64; 4]>,
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
                    id.clone(),
                    Subgraph {
                        id,
                        name: s["name"].as_str().unwrap_or("Subgraph").to_owned(),
                        graph: parse_graph(s)?,
                        inputs: parse_io(&s["inputs"]),
                        outputs: parse_io(&s["outputs"]),
                        input_node: rect(&s["inputNode"]["bounding"]),
                        output_node: rect(&s["outputNode"]["bounding"]),
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
        if let Some(defs) = v["definitions"]["subgraphs"].as_array_mut() {
            for d in defs {
                let Some(sg) = d["id"].as_str().and_then(|id| self.subgraphs.get(id)) else { continue };
                write_graph(d, &sg.graph, true);
                for (key, ios) in [("inputs", &sg.inputs), ("outputs", &sg.outputs)] {
                    for (i, io) in d[key].as_array_mut().into_iter().flatten().enumerate() {
                        if let Some(src) = ios.get(i) {
                            io["linkIds"] = Value::from(src.link_ids.clone());
                        }
                    }
                }
            }
        }
        v
    }
}

/// Splice `g` back into its raw node/link arrays. Subgraph definitions store links as
/// objects, the root graph as 0.4 tuples.
fn write_graph(s: &mut Value, g: &Graph, object_links: bool) {
    for nv in s["nodes"].as_array_mut().into_iter().flatten() {
        let Some(n) = id_of(&nv["id"]).and_then(|id| g.nodes.iter().find(|n| n.id == id)) else { continue };
        nv["pos"] = serde_json::json!([n.pos.0, n.pos.1]);
        for (i, iv) in nv["inputs"].as_array_mut().into_iter().flatten().enumerate() {
            if let Some(src) = n.inputs.get(i) {
                iv["link"] = src.link.map(Value::from).unwrap_or(Value::Null);
            }
        }
        for (i, ov) in nv["outputs"].as_array_mut().into_iter().flatten().enumerate() {
            if let Some(src) = n.outputs.get(i) {
                ov["links"] = Value::from(src.links.clone());
            }
        }
    }

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
        color: n["color"].as_str().map(str::to_owned),
        bgcolor: n["bgcolor"].as_str().map(str::to_owned),
        collapsed: n["flags"]["collapsed"].as_bool().unwrap_or(false),
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
