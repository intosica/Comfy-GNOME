// SPDX-License-Identifier: GPL-3.0-only
//! DaSiWa custom nodes: the behaviour their JS frontends add on top of litegraph.
//!
//! Three of them keep state outside the widgets the user sees, so a plain
//! render of `widgets_values` is either wrong or unreadable:
//!
//! * `DaSiWa_NodeStatusSwitch` — mutes or bypasses every node wired into its
//!   `target_XX` inputs, depending on `enabled` and `trigger_on`. Chained
//!   switches (one switch feeding another's `enabled` input) follow along.
//! * `DaSiWa_SeedControl` — `seed_control_state` holds `{mode,last_seed,recent}`;
//!   in Random mode the frontend rolls a fresh seed *before each queue*.
//! * `DaSiWa_LTX2LoraLoader` — `stack_data` holds the LoRA rows as a JSON string.
//!
//! Sources: custom_nodes/ComfyUI-DaSiWa-Nodes/js/{node_status_switch,
//! dasiwa_seed_control,advanced_lora_loader_ui}.js. Only the parts that change
//! what gets queued are reproduced; the custom-drawn panels are not.

use crate::workflow::{Graph, Workflow};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const SEED_CONTROL: &str = "DaSiWa_SeedControl";
pub const STATUS_SWITCH: &str = "DaSiWa_NodeStatusSwitch";
pub const LTX2_LORA: &str = "DaSiWa_LTX2LoraLoader";

/// Widget positions, fixed by each node's `/object_info` definition.
/// ponytail: the JS looks widgets up by name; we index the file format directly.
const SEED_VALUE: usize = 0;
const SEED_STATE: usize = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct SeedState {
    pub mode: String,
    pub last_seed: Option<String>,
    pub recent: Vec<String>,
}

impl Default for SeedState {
    fn default() -> Self {
        Self { mode: "random".into(), last_seed: None, recent: Vec::new() }
    }
}

impl SeedState {
    pub fn parse(raw: &str) -> Self {
        let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let digits = |x: &Value| {
            x.as_str()
                .map(str::to_owned)
                .or_else(|| x.as_u64().map(|n| n.to_string()))
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        };
        Self {
            mode: if v["mode"] == "fixed" { "fixed".into() } else { "random".into() },
            last_seed: digits(&v["last_seed"]),
            recent: v["recent"].as_array().map(|a| a.iter().filter_map(digits).take(10).collect()).unwrap_or_default(),
        }
    }

    pub fn to_json(&self) -> String {
        json!({ "mode": self.mode, "last_seed": self.last_seed, "recent": self.recent }).to_string()
    }
}

/// Index of `stack_data` in `DaSiWa_LTX2LoraLoader.widgets_values` (`model, clip, stack_data, …`).
const STACK_DATA: usize = 2;

fn stack_state(vals: &Value) -> Vec<Value> {
    vals.get(STACK_DATA)
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn stack_store(vals: &mut Value, rows: &[Value]) {
    if let Some(a) = vals.as_array_mut() {
        if a.len() > STACK_DATA {
            a[STACK_DATA] = Value::String(Value::Array(rows.to_vec()).to_string());
        }
    }
}

/// One `WKind::Lora` row per stack entry: `{on, lora, strength}` plus the original
/// row under `_row`, so `vs`/`as` survive an edit that only touched the base strength.
pub fn stack_rows(vals: &Value) -> Vec<Value> {
    stack_state(vals)
        .into_iter()
        .map(|row| {
            json!({
                "on": row["on"].as_bool().unwrap_or(true),
                "lora": row["lora"].as_str().unwrap_or("None"),
                "strength": row["str"].as_f64().unwrap_or(1.0),
                "_row": row,
            })
        })
        .collect()
}

/// Write row `i` back into `stack_data`; `Null` removes the row, `None` appends one.
pub fn stack_set(vals: &mut Value, i: Option<usize>, edited: &Value) {
    let mut rows = stack_state(vals);
    match (i, edited.is_null()) {
        (Some(i), true) if i < rows.len() => {
            rows.remove(i);
        }
        (Some(i), _) if i < rows.len() => {
            let mut row = edited["_row"].clone();
            if !row.is_object() {
                row = json!({});
            }
            row["on"] = json!(edited["on"].as_bool().unwrap_or(true));
            row["lora"] = json!(edited["lora"].as_str().unwrap_or("None"));
            row["str"] = json!(edited["strength"].as_f64().unwrap_or(1.0));
            row["vs"] = row["vs"].as_f64().map(Value::from).unwrap_or_else(|| json!(1.0));
            row["as"] = row["as"].as_f64().map(Value::from).unwrap_or_else(|| json!(1.0));
            rows[i] = row;
        }
        (None, _) => rows.push(json!({
            "on": edited["on"].as_bool().unwrap_or(true),
            "lora": edited["lora"].as_str().unwrap_or("None"),
            "str": 1.0,
            "vs": 1.0,
            "as": 1.0,
        })),
        _ => {}
    }
    stack_store(vals, &rows);
}

/// 64-bit seed, the range `DaSiWa_SeedControl` rolls in JS (`crypto.getRandomValues`).
pub fn rand_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

/// Apply every DaSiWa queue-time behaviour in the workflow. Called right before
/// `prompt::build`, once per queued copy; returns the lines to log.
pub fn before_queue(wf: &mut Workflow) -> Vec<String> {
    let mut log = Vec::new();
    let graphs = std::iter::once(&mut wf.root).chain(wf.subgraphs.values_mut().map(|s| &mut s.graph));
    for g in graphs {
        apply_switches(g, &mut log);
        apply_seeds(g, &mut log);
    }
    log
}

/// `enabled` → target mode. Three passes: a switch fed by another switch's
/// output sees the upstream decision from the previous pass.
fn apply_switches(g: &mut Graph, log: &mut Vec<String>) {
    let switches: Vec<usize> = g.nodes.iter().enumerate().filter(|(_, n)| n.ty == STATUS_SWITCH).map(|(i, _)| i).collect();
    if switches.is_empty() {
        return;
    }
    let mut known: HashMap<String, bool> = HashMap::new();
    for _ in 0..3 {
        for &i in &switches {
            // Read everything out of the node first: mutating g.nodes needs the borrow back.
            let (id, enabled, mode, targets) = {
                let n = &g.nodes[i];
                let vals = n.widgets_values.as_array();
                let widget_enabled = vals.and_then(|v| v.first()).and_then(Value::as_bool).unwrap_or(true);
                let upstream = n
                    .inputs
                    .iter()
                    .find(|i| i.name == "enabled")
                    .and_then(|i| i.link)
                    .and_then(|l| g.links.get(&l))
                    .and_then(|l| known.get(&l.origin_id).copied());
                let enabled = upstream.unwrap_or(widget_enabled);
                let trigger = vals.and_then(|v| v.get(1)).and_then(Value::as_str).unwrap_or("true → active");
                let action = vals.and_then(|v| v.get(2)).and_then(Value::as_str).unwrap_or("bypass");
                // "true → active" / "false → active" pick which `enabled` value activates targets.
                let targets_active = if trigger.starts_with("true") { enabled } else { !enabled };
                let mode = if targets_active {
                    0
                } else if action == "mute" {
                    crate::workflow::MODE_NEVER
                } else {
                    crate::workflow::MODE_BYPASS
                };
                let targets: Vec<String> = n
                    .inputs
                    .iter()
                    .filter(|i| i.name.starts_with("target_"))
                    .filter_map(|i| i.link)
                    .filter_map(|l| g.links.get(&l))
                    .map(|l| l.origin_id.clone())
                    .collect();
                (n.id.clone(), enabled, mode, targets)
            };
            let mut changed = 0;
            for t in &targets {
                if let Some(tn) = g.nodes.iter_mut().find(|n| &n.id == t) {
                    if tn.mode != mode {
                        tn.mode = mode;
                        changed += 1;
                    }
                }
            }
            if changed > 0 {
                let state = if mode == 0 { "active" } else if mode == crate::workflow::MODE_NEVER { "muted" } else { "bypassed" };
                log.push(format!("[dasiwa] switch {id}: {} target(s) → {state}", targets.len()));
            }
            known.insert(id, enabled);
        }
    }
}

/// Random mode: new seed into `seed_value`, pushed onto `recent` (max 10).
fn apply_seeds(g: &mut Graph, log: &mut Vec<String>) {
    for n in g.nodes.iter_mut().filter(|n| n.ty == SEED_CONTROL && n.mode == 0) {
        let Some(vals) = n.widgets_values.as_array_mut() else { continue };
        let raw = vals.get(SEED_STATE).and_then(Value::as_str).unwrap_or("").to_owned();
        let mut st = SeedState::parse(&raw);
        if st.mode != "random" {
            continue;
        }
        let seed = rand_seed();
        let text = seed.to_string();
        if vals.len() <= SEED_STATE {
            continue;
        }
        vals[SEED_VALUE] = json!(seed);
        st.last_seed = Some(text.clone());
        st.recent.insert(0, text.clone());
        st.recent.truncate(10);
        vals[SEED_STATE] = Value::String(st.to_json());
        log.push(format!("[dasiwa] seed {}: rolled {text}", n.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_node() -> Value {
        json!({
            "id": 7, "type": SEED_CONTROL, "mode": 0,
            "inputs": [{"name": "seed", "type": "INT", "link": null}],
            "widgets_values": [0, "{\"mode\":\"random\",\"last_seed\":null,\"recent\":[]}", 0]
        })
    }

    fn switch_node() -> Value {
        json!({
            "id": 9, "type": STATUS_SWITCH, "mode": 0,
            "inputs": [
                {"name": "enabled", "type": "BOOLEAN", "link": null},
                {"name": "target_00", "type": "*", "link": 5}
            ],
            "widgets_values": [true, "true → active", "mute"]
        })
    }

    fn workflow(nodes: Vec<Value>, links: Vec<Value>) -> Workflow {
        let v = json!({
            "nodes": nodes, "links": links, "groups": [],
            "extra": {}, "version": 0.4
        });
        Workflow::from_json(v).expect("workflow")
    }

    #[test]
    fn ltx2_stack_rows_round_trip() {
        let mut vals = json!([
            "m", "c",
            "[{\"on\":true,\"lora\":\"a.safetensors\",\"str\":0.8,\"vs\":0.9,\"as\":1.0}]",
            "Basic", true
        ]);
        let rows = stack_rows(&vals);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["strength"], json!(0.8));
        let mut edited = rows[0].clone();
        edited["strength"] = json!(0.5);
        stack_set(&mut vals, Some(0), &edited);
        let back = stack_rows(&vals);
        assert_eq!(back[0]["strength"], json!(0.5));
        assert_eq!(back[0]["_row"]["vs"], json!(0.9), "vs survives a strength edit");
        stack_set(&mut vals, None, &edited);
        assert_eq!(stack_rows(&vals).len(), 2, "Null appends");
        stack_set(&mut vals, Some(0), &Value::Null);
        assert_eq!(stack_rows(&vals).len(), 1, "Null removes the row");
    }

    #[test]
    fn seed_state_parses_and_round_trips() {
        let st = SeedState::parse("{\"mode\":\"fixed\",\"last_seed\":\"42\",\"recent\":[\"42\",\"7\",\"x\",1]}");
        assert_eq!(st.mode, "fixed");
        assert_eq!(st.last_seed.as_deref(), Some("42"));
        assert_eq!(st.recent, ["42", "7", "1"]);
        assert_eq!(SeedState::parse("junk").mode, "random");
        assert!(SeedState::parse(&st.to_json()).recent.contains(&"42".to_string()));
    }

    #[test]
    fn random_seed_control_rolls_on_queue() {
        let target = json!({"id": 3, "type": "SomeNode", "mode": 0, "inputs": [], "widgets_values": []});
        let mut wf = workflow(vec![seed_node(), target], vec![]);
        let log = before_queue(&mut wf);
        assert_eq!(log.len(), 1, "{log:?}");
        let vals = wf.root.nodes[0].widgets_values.as_array().unwrap();
        let seed = vals[0].as_u64().unwrap();
        assert_ne!(seed, 0);
        let st = SeedState::parse(vals[1].as_str().unwrap());
        assert_eq!(st.mode, "random");
        assert_eq!(st.last_seed.as_deref(), Some(seed.to_string().as_str()));
        assert_eq!(st.recent.first().unwrap(), &seed.to_string());
    }

    #[test]
    fn fixed_seed_control_keeps_its_seed() {
        let mut n = seed_node();
        n["widgets_values"][0] = json!(1234);
        n["widgets_values"][1] = Value::String("{\"mode\":\"fixed\",\"last_seed\":\"1234\"}".into());
        let mut wf = workflow(vec![n], vec![]);
        assert!(before_queue(&mut wf).is_empty());
        assert_eq!(wf.root.nodes[0].widgets_values[0], json!(1234));
    }

    #[test]
    fn status_switch_mutes_targets_when_disabled() {
        let target = json!({"id": 3, "type": "SomeNode", "mode": 0, "inputs": [], "widgets_values": []});
        // enabled=false + "false → active": targets must go active (mode 0), not muted.
        let mut n = switch_node();
        n["widgets_values"][0] = json!(false);
        n["widgets_values"][1] = Value::String("false → active".into());
        let mut wf = workflow(vec![n, target.clone()], vec![json!([5, 3, 0, 9, 1, "*"])]);
        before_queue(&mut wf);
        assert_eq!(wf.root.nodes[1].mode, 0);
        // enabled=false + "true → active" + mute: targets muted.
        let mut n = switch_node();
        n["widgets_values"][0] = json!(false);
        let mut wf = workflow(vec![n, target.clone()], vec![json!([5, 3, 0, 9, 1, "*"])]);
        let log = before_queue(&mut wf);
        assert_eq!(wf.root.nodes[1].mode, crate::workflow::MODE_NEVER);
        assert_eq!(log.len(), 1, "{log:?}");
        // ...and bypass mode flips the target to MODE_BYPASS instead.
        let mut n = switch_node();
        n["widgets_values"][0] = json!(false);
        n["widgets_values"][2] = Value::String("bypass".into());
        let mut wf = workflow(vec![n, target], vec![json!([5, 3, 0, 9, 1, "*"])]);
        before_queue(&mut wf);
        assert_eq!(wf.root.nodes[1].mode, crate::workflow::MODE_BYPASS);
    }
}
