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
    /// v3 `COMFY_DYNAMICCOMBO_V3`: a combo whose selected option adds nested inputs,
    /// serialized right after it as `"<name>.<child>"`.
    DynCombo(Vec<(String, Vec<Input>)>),
    /// Widget type the frontend (or an extension) draws itself: MARKDOWN, COLOR,
    /// IMAGECOMPARE, EASY_PROMPT_STYLES, … Value is opaque JSON.
    Custom(String),
    /// Link-only socket of this type (e.g. "MODEL", "*").
    Socket(String),
}

/// Core frontend widget types beyond the primitives (`scripts/widgets.ts::ComfyWidgets`).
const CORE_WIDGETS: &[&str] = &[
    "MARKDOWN", "IMAGEUPLOAD", "COLOR", "IMAGECOMPARE", "BOUNDING_BOX", "CHART", "GALLERIA", "PAINTER",
    "COMPOSITOR", "TEXTAREA", "CURVE", "RANGE", "VIDEO_EDIT", "RESOLUTION_PREVIEW", "BOUNDING_BOXES",
    "LIGHT_INFO_PREVIEW", "COLORS",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub name: String,
    pub kind: Kind,
    pub optional: bool,
    /// The full options dict (default/min/max/step/multiline/tooltip/…), kept raw.
    pub opts: Value,
}

impl Input {
    /// Occupies a `widgets_values` slot. `forceInput` primitives are plain sockets.
    pub fn is_widget(&self) -> bool {
        !matches!(self.kind, Kind::Socket(_)) && self.opts["forceInput"].as_bool() != Some(true)
    }
    /// Socket type string as used for link compatibility checks.
    pub fn type_name(&self) -> &str {
        if let Some(t) = self.opts["__union"].as_str() {
            return t;
        }
        match &self.kind {
            Kind::Int => "INT",
            Kind::Float => "FLOAT",
            Kind::String => "STRING",
            Kind::Boolean => "BOOLEAN",
            Kind::Combo(_) | Kind::DynCombo(_) => "COMBO",
            Kind::Custom(t) | Kind::Socket(t) => t,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeDef {
    pub name: String,
    pub display_name: String,
    pub category: String,
    pub description: String,
    pub inputs: Vec<Input>,
    /// (type, name)
    pub outputs: Vec<(String, String)>,
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
        let mut r = Self { nodes };
        r.mark_custom_widgets();
        r
    }

    /// Extensions register widget constructors for their own types in JS, which we can't
    /// see. Heuristic: a socket type that no node outputs can never be linked, so it must
    /// be a widget (EASY_PROMPT_STYLES, COLORCODE, LORAS…). Union types ("A,B"), "*" and
    /// v3 dynamic-input markers are excluded.
    fn mark_custom_widgets(&mut self) {
        let produced: std::collections::HashSet<String> =
            self.nodes.values().flat_map(|n| n.outputs.iter().map(|o| o.0.clone())).collect();
        let fix = |i: &mut Input| {
            if let Kind::Socket(t) = &i.kind {
                let custom = CORE_WIDGETS.contains(&t.as_str())
                    || (!produced.contains(t) && !t.contains(',') && t != "*" && !t.ends_with("_V3"));
                if custom {
                    i.kind = Kind::Custom(t.clone());
                }
            }
        };
        for n in self.nodes.values_mut() {
            for i in &mut n.inputs {
                fix(i);
                if let Kind::DynCombo(opts) = &mut i.kind {
                    opts.iter_mut().flat_map(|o| o.1.iter_mut()).for_each(fix);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn get(&self, name: &str) -> Option<&NodeDef> {
        self.nodes.get(name)
    }

    /// Apply custom-node translations from ComfyUI's `GET /i18n`.
    ///
    /// The V3 node schema reports a `null` display_name (it ignores
    /// `NODE_DISPLAY_NAME_MAPPINGS`), so `/object_info` alone shows raw ids such as
    /// `easy wildcards`. The frontend overlays the locale's `nodeDefs.<id>.display_name`
    /// from the same endpoint; do the same here. English is the base locale the
    /// frontend itself falls back to.
    pub fn apply_display_names(&mut self, i18n: &Value) {
        let defs = i18n
            .get("en")
            .or_else(|| i18n.as_object().and_then(|o| o.values().next()))
            .and_then(|l| l.get("nodeDefs"));
        let Some(defs) = defs.and_then(Value::as_object) else { return };
        for (name, tr) in defs {
            if let (Some(def), Some(display)) = (self.nodes.get_mut(name), tr["display_name"].as_str()) {
                if !display.is_empty() {
                    def.display_name = display.to_owned();
                }
            }
        }
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
    let inputs = parse_inputs(&d["input"], &d["input_order"]);
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
        inputs,
        outputs,
        deprecated: d["deprecated"].as_bool().unwrap_or(false),
    })
}

/// Output/socket type may be a string or (legacy combo output) a list.
fn type_str(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| "COMBO".into())
}

/// `{required:{…}, optional:{…}}`, ordered by `input_order` when given, else by
/// map order (serde_json `preserve_order` keeps the server's declaration order).
fn parse_inputs(input: &Value, order: &Value) -> Vec<Input> {
    let mut inputs = Vec::new();
    for (section, optional) in [("required", false), ("optional", true)] {
        let Some(sec) = input[section].as_object() else { continue };
        let names: Vec<String> = order[section]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_else(|| sec.keys().cloned().collect());
        for name in names {
            if let Some(spec) = sec.get(&name) {
                inputs.push(parse_input(name, spec, optional));
            }
        }
    }
    inputs
}

fn parse_input(name: String, spec: &Value, optional: bool) -> Input {
    let mut head = spec[0].clone();
    let mut opts = spec.get(1).cloned().unwrap_or(Value::Null);
    // A union socket ("FLOAT,INT", "AUTOCOMPLETE_TEXT_PROMPT,STRING") with `widgetType`
    // is still a widget (frontend: `widgetType ?? type`); keep the union for linking.
    let union = head.as_str().filter(|u| u.contains(',')).map(str::to_owned);
    if let (Some(u), Some(w)) = (union, opts["widgetType"].as_str().map(str::to_owned)) {
        let w = if w.starts_with("AUTOCOMPLETE_TEXT") {
            opts["multiline"] = true.into(); // LoRA Manager prompt box: our text editor autocompletes
            "STRING".to_owned()
        } else {
            w
        };
        opts["__union"] = u.into();
        head = w.into();
    }
    let head = &head;
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
            "COMFY_DYNAMICCOMBO_V3" => Kind::DynCombo(
                opts["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| (o["key"].as_str().unwrap_or_default().to_owned(), parse_inputs(&o["inputs"], &Value::Null)))
                    .collect(),
            ),
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
               "category":"sampling","python_module":"nodes","output_node":false},
               "Loader":{"input":{},"output":["MODEL"],"category":"loaders"}}"#,
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

    #[test]
    fn union_with_widget_type_is_a_widget() {
        let i = parse_input("text".into(), &serde_json::json!(["AUTOCOMPLETE_TEXT_PROMPT,STRING", {"widgetType": "AUTOCOMPLETE_TEXT_PROMPT"}]), false);
        assert!(i.is_widget());
        assert_eq!(i.kind, Kind::String);
        assert_eq!(i.type_name(), "AUTOCOMPLETE_TEXT_PROMPT,STRING");
        let f = parse_input("fps".into(), &serde_json::json!(["FLOAT,INT", {"widgetType": "FLOAT"}]), false);
        assert_eq!(f.kind, Kind::Float);
        assert!(!parse_input("x".into(), &serde_json::json!(["A,B", {}]), false).is_widget());
    }
}
