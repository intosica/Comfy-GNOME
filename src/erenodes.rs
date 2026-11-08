// SPDX-License-Identifier: GPL-3.0-only
//! EreNodes (ComfyUI-EreNodes) prompt nodes: tag data lives in `properties._tagDataJSON`,
//! the node's single `text` widget holds the generated prompt (ported from web/prompt.js).

use serde_json::{json, Value};

pub const TYPES: [&str; 6] = ["ErePromptCloud", "ErePromptGallery", "ErePromptToggle", "ErePromptMultiSelect", "ErePromptRandomizer", "ErePromptMultiline"];

/// Display name → node type, for "Convert to …".
pub const CONVERT: [(&str, &str); 6] = [
    ("Prompt Cloud", "ErePromptCloud"),
    ("Prompt MultiSelect", "ErePromptMultiSelect"),
    ("Prompt Toggle", "ErePromptToggle"),
    ("Prompt Multiline", "ErePromptMultiline"),
    ("Prompt Randomizer", "ErePromptRandomizer"),
    ("Prompt Gallery", "ErePromptGallery"),
];

pub fn is_ere(ty: &str) -> bool {
    TYPES.contains(&ty)
}

/// The node's tags (empty when absent or unparsable).
pub fn tags(props: &Value) -> Value {
    props["_tagDataJSON"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()).filter(Value::is_array).unwrap_or_else(|| json!([]))
}

pub fn store(props: &mut Value, tags: &Value) {
    if !props.is_object() {
        *props = json!({});
    }
    props["_tagDataJSON"] = Value::String(serde_json::to_string_pretty(tags).unwrap_or_default());
}

pub fn name(t: &Value) -> &str {
    t["name"].as_str().unwrap_or("")
}

pub fn kind(t: &Value) -> &str {
    t["type"].as_str().unwrap_or("tag")
}

pub fn active(t: &Value) -> bool {
    t["active"].as_bool().unwrap_or(true)
}

/// Strength when it differs from 1.0 (and is non-zero, as in the frontend's truthiness test).
pub fn strength(t: &Value) -> Option<f64> {
    t["strength"].as_f64().filter(|s| *s != 0.0 && *s != 1.0)
}

pub fn triggers(t: &Value) -> Vec<String> {
    t["triggers"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect()
}

fn num(s: f64) -> String {
    if s.fract() == 0.0 { format!("{s:.1}") } else { s.to_string() }
}

fn file_name(t: &Value) -> String {
    format!("{}{}", name(t), t["extension"].as_str().unwrap_or(""))
}

fn format_tag(t: &Value) -> String {
    match kind(t) {
        "lora" => format!("<lora:{}:{}>", file_name(t), num(t["strength"].as_f64().filter(|s| *s != 0.0).unwrap_or(1.0))),
        "embedding" => format!("embedding:{}", name(t)),
        "group" => format!("group:{}", file_name(t)),
        _ => match strength(t) {
            Some(s) => format!("({}:{})", name(t), num(s)),
            None => name(t).to_owned(),
        },
    }
}

fn parts(t: &Value) -> Vec<String> {
    let mut v = vec![format_tag(t)];
    if kind(t) == "lora" {
        v.extend(triggers(t));
    }
    v
}

/// Group file name to fetch for a group tag.
pub fn group_file(t: &Value) -> String {
    let f = file_name(t);
    if t["extension"].is_null() && !f.ends_with(".json") { f + ".json" } else { f }
}

/// Generated prompt text. `group(file)` returns a group's tags (None = unavailable).
pub fn build_text(tags: &Value, sep: &str, group: &dyn Fn(&str) -> Option<Value>) -> String {
    let sep = sep.replacen("\\n", "\n", 1);
    let mut out: Vec<String> = Vec::new();
    let mut line: Vec<String> = Vec::new();
    let flush = |out: &mut Vec<String>, line: &mut Vec<String>| {
        if !line.is_empty() {
            if !out.is_empty() {
                out.push(sep.clone());
            }
            out.push(line.join(&sep));
            line.clear();
        }
    };
    for t in tags.as_array().into_iter().flatten().filter(|t| active(t) && !name(t).is_empty()) {
        if kind(t) != "group" {
            line.extend(parts(t));
            continue;
        }
        flush(&mut out, &mut line);
        let Some(g) = group(&group_file(t)) else { continue };
        let inner: Vec<String> = g.as_array().into_iter().flatten().filter(|t| active(t) && !name(t).is_empty()).flat_map(parts).collect();
        if inner.is_empty() {
            continue;
        }
        let mut joined = inner.join(&sep);
        if let Some(s) = strength(t) {
            joined = format!("({joined}:{s:.2})");
        }
        if !out.is_empty() {
            out.push(sep.clone());
        }
        out.push(joined);
    }
    out.join("")
}

/// Label shown on a pill.
pub fn display(t: &Value, keep_folders: bool) -> String {
    let n = name(t);
    match kind(t) {
        "lora" | "group" => {
            let base = if keep_folders { n } else { n.rsplit(['/', '\\']).next().unwrap_or(n) };
            base.to_owned()
        }
        "embedding" => n.strip_prefix("embedding:").unwrap_or(n).to_owned(),
        _ => n.to_owned(),
    }
}

/// Small info text: `[+N]` triggers and non-default strength.
pub fn info(t: &Value) -> String {
    let mut v = Vec::new();
    let n = triggers(t).len();
    if n > 0 {
        v.push(format!("[+{n}]"));
    }
    if let Some(s) = strength(t) {
        v.push(format!("{s:.2}"));
    }
    v.join(" ")
}

/// Server path (for `view_query`) of a tag's gallery image, if the tag type has any.
pub fn image_key(t: &Value, w: f64, h: f64) -> Option<String> {
    let ty = match kind(t) {
        "lora" => "lora",
        "embedding" => "embedding",
        "group" => "group",
        _ => return None,
    };
    let enc: String = name(t)
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    Some(format!("/erenodes/view/{ty}/{enc}?w={}&h={}&fit=cover", w as i64, h as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_from_tags_and_groups() {
        let tags = json!([
            {"name": "a", "type": "tag", "active": true},
            {"name": "off", "type": "tag", "active": false},
            {"name": "b", "type": "tag", "strength": 1.2},
            {"name": "x/L", "type": "lora", "extension": ".safetensors", "triggers": ["tr"]},
            {"name": "AIXG", "type": "group", "strength": 0.8},
        ]);
        let g = |f: &str| (f == "AIXG.json").then(|| json!([{"name": "g1", "active": true}, {"name": "g2", "active": false}]));
        assert_eq!(build_text(&tags, ", ", &g), "a, (b:1.2), <lora:x/L.safetensors:1.0>, tr, (g1:0.80)");
        assert_eq!(build_text(&json!([]), ", ", &g), "");
        assert_eq!(image_key(&tags[3], 100.0, 100.0).unwrap(), "/erenodes/view/lora/x/L?w=100&h=100&fit=cover");
    }
}
