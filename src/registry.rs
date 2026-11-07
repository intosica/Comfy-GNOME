// SPDX-License-Identifier: GPL-3.0-only
//! Node definition registry built from `GET /object_info`.
//!
//! Input spec forms (ComfyUI `INPUT_TYPES`):
//!   `["INT", {default,min,max,step,…}]`            primitive widget
//!   `[["a","b"], {default,…}]`                      legacy combo (options inline)
//!   `["COMBO", {options:[…], …}]`                   v3 combo
//!   `["MODEL", {…}]` / `["*", …]`                   link-only socket
//! Primitive types (INT/FLOAT/STRING/BOOLEAN/COMBO) become widgets; everything else a socket.

use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Int,
    Float,
    String,
    Boolean,
    Combo(Vec<String>),
    /// Link-only socket of this type (e.g. "MODEL", "*").
    Socket(String),
}

#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub kind: Kind,
    pub optional: bool,
    /// The full options dict (default/min/max/step/multiline/tooltip/…), kept raw.
    pub opts: Value,
}

impl Input {
    pub fn is_widget(&self) -> bool {
        !matches!(self.kind, Kind::Socket(_))
    }
    /// Socket type string as used for link compatibility checks.
    #[allow(dead_code)] // Phase 3 link validation
    pub fn type_name(&self) -> &str {
        match &self.kind {
            Kind::Int => "INT",
            Kind::Float => "FLOAT",
            Kind::String => "STRING",
            Kind::Boolean => "BOOLEAN",
            Kind::Combo(_) => "COMBO",
            Kind::Socket(t) => t,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeDef {
    pub name: String,
    pub display_name: String,
    pub category: String,
    pub description: String,
    pub python_module: String,
    pub inputs: Vec<Input>,
    /// (type, name)
    pub outputs: Vec<(String, String)>,
    pub output_node: bool,
    pub deprecated: bool,
}

#[derive(Debug, Default)]
pub struct Registry {
    pub nodes: BTreeMap<String, NodeDef>,
}

impl Registry {
    pub fn from_object_info(v: &Value) -> Self {
        let nodes = v
            .as_object()
            .map(|m| m.iter().filter_map(|(k, d)| Some((k.clone(), parse_node(k, d)?))).collect())
            .unwrap_or_default();
        Self { nodes }
    }

    pub fn get(&self, name: &str) -> Option<&NodeDef> {
        self.nodes.get(name)
    }

    /// Category tree: "loaders/model" → nodes, sorted. For the node library sidebar.
    pub fn by_category(&self) -> BTreeMap<&str, Vec<&NodeDef>> {
        let mut m: BTreeMap<&str, Vec<&NodeDef>> = BTreeMap::new();
        for n in self.nodes.values().filter(|n| !n.deprecated) {
            m.entry(n.category.as_str()).or_default().push(n);
        }
        for v in m.values_mut() {
            v.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }
        m
    }
}

fn parse_node(key: &str, d: &Value) -> Option<NodeDef> {
    let s = |k: &str| d[k].as_str().unwrap_or_default().to_owned();
    let input = &d["input"];
    let mut inputs = Vec::new();
    for (section, optional) in [("required", false), ("optional", true)] {
        let Some(sec) = input[section].as_object() else { continue };
        // input_order preserves Python declaration order; fall back to map order.
        let order: Vec<String> = d["input_order"][section]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_else(|| sec.keys().cloned().collect());
        for name in order {
            if let Some(spec) = sec.get(&name) {
                inputs.push(parse_input(name, spec, optional));
            }
        }
    }
    let types: Vec<String> = d["output"].as_array()?.iter().map(type_str).collect();
    let names = d["output_name"].as_array();
    let outputs = types
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n = names.and_then(|n| n.get(i)).and_then(Value::as_str).unwrap_or(t);
            (t.clone(), n.to_owned())
        })
        .collect();
    let display = s("display_name");
    Some(NodeDef {
        name: key.to_owned(),
        display_name: if display.is_empty() { key.to_owned() } else { display },
        category: s("category"),
        description: s("description"),
        python_module: s("python_module"),
        inputs,
        outputs,
        output_node: d["output_node"].as_bool().unwrap_or(false),
        deprecated: d["deprecated"].as_bool().unwrap_or(false),
    })
}

/// Output/socket type may be a string or (legacy combo output) a list.
fn type_str(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| "COMBO".into())
}

fn parse_input(name: String, spec: &Value, optional: bool) -> Input {
    let head = &spec[0];
    let opts = spec.get(1).cloned().unwrap_or(Value::Null);
    let strings = |a: &Value| -> Vec<String> {
        a.as_array()
            .map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect())
            .unwrap_or_default()
    };
    let kind = match head {
        Value::Array(_) => Kind::Combo(strings(head)),
        Value::String(t) => match t.as_str() {
            "INT" => Kind::Int,
            "FLOAT" => Kind::Float,
            "STRING" => Kind::String,
            "BOOLEAN" => Kind::Boolean,
            "COMBO" => Kind::Combo(strings(&opts["options"])),
            t => Kind::Socket(t.to_owned()),
        },
        _ => Kind::Socket("*".into()),
    };
    Input { name, kind, optional, opts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_info() {
        let v: Value = serde_json::from_str(
            r#"{"KSampler":{"input":{"required":{"model":["MODEL",{}],"seed":["INT",{"default":0}],
               "sampler_name":[["euler","ddim"],{}]},"optional":{"img":["COMBO",{"options":["a.png"]}]}},
               "input_order":{"required":["model","seed","sampler_name"],"optional":["img"]},
               "output":["LATENT"],"output_name":["LATENT"],"name":"KSampler","display_name":"KSampler",
               "category":"sampling","python_module":"nodes","output_node":false}}"#,
        )
        .unwrap();
        let r = Registry::from_object_info(&v);
        let k = r.get("KSampler").unwrap();
        assert_eq!(k.inputs.len(), 4);
        assert_eq!(k.inputs[0].kind, Kind::Socket("MODEL".into()));
        assert_eq!(k.inputs[1].kind, Kind::Int);
        assert_eq!(k.inputs[2].kind, Kind::Combo(vec!["euler".into(), "ddim".into()]));
        assert_eq!(k.inputs[3].kind, Kind::Combo(vec!["a.png".into()]));
        assert!(k.inputs[3].optional);
        assert_eq!(k.outputs, vec![("LATENT".into(), "LATENT".into())]);
        assert_eq!(r.by_category()["sampling"].len(), 1);
    }
}
