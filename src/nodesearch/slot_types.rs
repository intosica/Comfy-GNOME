// SPDX-License-Identifier: GPL-3.0-only
//! Port of `src/schemas/nodeDef/searchableSlotTypes.ts` + the interpreter parts of
//! `inputSpecInterpreter.ts` (frontend 1.60.0), working on the raw `/object_info` JSON
//! instead of zod-parsed specs. Used for the Input/Output type filters of the node search.

use serde_json::Value;

/// `splitSlotTypes`: "IMAGE, MASK" → ["IMAGE","MASK"]; non-strings → [].
fn split(t: &Value) -> Vec<String> {
    t.as_str().map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

/// V1 spec `[type, opts]` → V2 type string (a list head is a combo).
fn spec_type(spec: &Value) -> &str {
    spec[0].as_str().unwrap_or("COMBO")
}

/// `toInputSpecsV2`: every spec of `{required, optional}`.
fn specs_of(inputs: &Value) -> impl Iterator<Item = &Value> {
    ["required", "optional"].into_iter().flat_map(move |s| inputs[s].as_object().into_iter().flat_map(|o| o.values()))
}

/// `nestedInputs` of each dynamic control (`dynamicControls` in the interpreter).
fn nested(spec: &Value) -> Vec<&Value> {
    let opts = &spec[1];
    match spec_type(spec) {
        "COMFY_DYNAMICGROUP_V3" => vec![&opts["template"]],
        "COMFY_AUTOGROW_V3" => vec![&opts["template"]["input"]],
        "COMFY_DYNAMICCOMBO_V3" => opts["options"].as_array().into_iter().flatten().map(|o| &o["inputs"]).collect(),
        _ => vec![],
    }
}

/// `inputSpecTree`: the spec and every spec nested in dynamic controls.
fn tree<'a>(spec: &'a Value, out: &mut Vec<&'a Value>) {
    out.push(spec);
    for inputs in nested(spec) {
        for child in specs_of(inputs) {
            tree(child, out);
        }
    }
}

/// `ownSlotTypes`: dynamic controls have no type of their own except MatchType.
fn own(spec: &Value) -> Vec<String> {
    match spec_type(spec) {
        "COMFY_DYNAMICGROUP_V3" | "COMFY_AUTOGROW_V3" | "COMFY_DYNAMICCOMBO_V3" => vec![],
        "COMFY_MATCHTYPE_V3" => split(&spec[1]["template"]["allowed_types"]),
        t => split(&Value::from(t)),
    }
}

fn uniq(v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// `collectSearchableInputTypes` over every input of a node.
pub fn input_types(node: &Value) -> Vec<String> {
    let mut specs = vec![];
    for s in specs_of(&node["input"]) {
        tree(s, &mut specs);
    }
    uniq(specs.into_iter().flat_map(own).collect())
}

/// `collectSearchableOutputTypes`: MatchType outputs stand for their template's allowed types.
pub fn output_types(node: &Value) -> Vec<String> {
    let mut by_template: Vec<(String, Vec<String>)> = vec![];
    let mut specs = vec![];
    for s in specs_of(&node["input"]) {
        tree(s, &mut specs);
    }
    for s in specs.into_iter().filter(|s| spec_type(s) == "COMFY_MATCHTYPE_V3") {
        let (Some(id), allowed) = (s[1]["template"]["template_id"].as_str(), split(&s[1]["template"]["allowed_types"])) else { continue };
        match by_template.iter_mut().find(|(k, _)| k == id) {
            Some((_, v)) => v.extend(allowed),
            None => by_template.push((id.to_owned(), allowed)),
        }
    }
    let types: Vec<Value> = node["output"].as_array().cloned().unwrap_or_default();
    let mut out = vec![];
    for (i, t) in types.iter().enumerate() {
        if t.as_str() == Some("COMFY_MATCHTYPE_V3") {
            if let Some(id) = node["output_matchtypes"][i].as_str() {
                if let Some((_, v)) = by_template.iter().find(|(k, _)| k == id) {
                    out.extend(v.clone());
                }
            }
        } else if t.is_array() {
            out.push("COMBO".into());
        } else {
            out.extend(split(t));
        }
    }
    uniq(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_and_matchtype() {
        let n = json!({"input":{"required":{
            "a":["MODEL",{}],"c":[["x","y"],{}],
            "d":["COMFY_DYNAMICCOMBO_V3",{"options":[{"key":"k","inputs":{"required":{"i":["IMAGE,MASK",{}]}}}]}],
            "m":["COMFY_MATCHTYPE_V3",{"template":{"template_id":"T","allowed_types":"INT,FLOAT"}}]}},
            "output":["COMFY_MATCHTYPE_V3","LATENT"],"output_matchtypes":["T",null]});
        assert_eq!(input_types(&n), ["MODEL", "COMBO", "IMAGE", "MASK", "INT", "FLOAT"]);
        assert_eq!(output_types(&n), ["INT", "FLOAT", "LATENT"]);
    }
}
