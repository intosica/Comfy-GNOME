// SPDX-License-Identifier: GPL-3.0-only
//! The ComfyUI frontend's node search (the "Add Node" box opened by double-click or by
//! releasing a dragged link on the canvas), ported as pure logic with no GTK in it.
//!
//! Layout mirrors the frontend 1.60.0 files so a frontend update is a file-by-file diff:
//!   `fuse.rs`        ← fuse.js 7 (bitap + extended search + norm scoring)
//!   `fuse_util.rs`   ← src/utils/fuseUtil.ts (advanced scoring, FuseFilter.matches)
//!   `slot_types.rs`  ← src/schemas/nodeDef/searchableSlotTypes.ts (+ inputSpecInterpreter.ts)
//!   `source.rs`      ← packages/object-info-parser/src/classifiers/nodeSource.ts
//!   this file        ← src/services/nodeSearchService.ts, src/stores/nodeDefStore.ts
//!                      (visibleNodeDefs, topNodeDefs), src/stores/nodeFrequencyStore.ts,
//!                      ComfyNodeDefImpl.postProcessSearchScores
//!   `content.rs`     ← src/components/searchbox/v2/NodeSearchContent.vue (+ category sidebar)
//! The relevance data (`data/frontend/node-frequency.json`) is copied from the frontend by
//! `tools/sync_frontend_data.py`; see AGENT-HANDOFF.md "Session 24".

pub mod content;
pub mod fuse;
pub mod fuse_util;
pub mod slot_types;
pub mod source;

use crate::registry::{NodeDef, Registry};
use fuse_util::{calc_aux_scores, compare_aux, filter_matches, Aux, Entry};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// `TOP_NODE_DEF_LIMIT`.
const TOP_NODE_DEF_LIMIT: usize = 64;
/// `nodeFuseSearch` fuse option.
const THRESHOLD: f64 = 0.3;

/// `nodeFrequencyStore`: file key order = default relevance order, value = score.
fn frequency() -> &'static (Vec<String>, HashMap<String, f64>) {
    static F: OnceLock<(Vec<String>, HashMap<String, f64>)> = OnceLock::new();
    F.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../data/frontend/node-frequency.json")).unwrap_or_default();
        let m = v.as_object().cloned().unwrap_or_default();
        (m.keys().cloned().collect(), m.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0))).collect())
    })
}

/// `ComfyNodeDefImpl.postProcessSearchScores`: `[main, -frequency, ...rest]`.
impl Entry for NodeDef {
    fn values(&self) -> Vec<&str> {
        vec![&self.name, &self.display_name]
    }
    fn post_process(&self, s: Aux) -> Aux {
        let f = frequency().1.get(&self.name).copied().unwrap_or(0.0);
        let mut out = vec![s[0], -f];
        out.extend_from_slice(&s[1..]);
        out
    }
}

/// `nodeDefStore.visibleNodeDefs` with the default settings: deprecated and dev-only nodes
/// hidden, experimental shown, litegraph subgraph placeholders hidden. Server order.
pub fn visible(reg: &Registry) -> Vec<&NodeDef> {
    reg.order
        .iter()
        .filter_map(|n| reg.nodes.get(n))
        .filter(|d| !d.deprecated && !d.dev_only && !(d.category == "subgraph" && d.python_module == "nodes"))
        .collect()
}

/// `nodeDefStore.topNodeDefs`: the most used nodes, the empty-search default.
pub fn top_node_defs(reg: &Registry) -> Vec<&NodeDef> {
    frequency().0.iter().filter_map(|n| reg.nodes.get(n)).take(TOP_NODE_DEF_LIMIT).collect()
}

/// `inputTypeFilter` / `outputTypeFilter` ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Input,
    Output,
}

impl FilterKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Output => "Output",
        }
    }
    fn options(self, d: &NodeDef) -> &[String] {
        match self {
            Self::Input => &d.input_types,
            Self::Output => &d.output_types,
        }
    }
}

/// `FuseFilterWithValue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub kind: FilterKind,
    pub value: String,
}

/// Every option the filter can take over `defs` (`FuseFilter.getAllNodeOptions`).
pub fn filter_options(kind: FilterKind, defs: &[&NodeDef]) -> Vec<String> {
    let mut seen = HashSet::new();
    defs.iter().flat_map(|d| kind.options(d).iter()).filter(|o| seen.insert(o.as_str())).cloned().collect()
}

/// `nodeSearchService.searchNode(query, filters, {limit})` with wildcard matching on:
/// fuse match → advanced re-rank → type filters → append `*`-typed nodes that match
/// only through their wildcard slot.
pub fn search_node<'a>(defs: &[&'a NodeDef], query: &str, filters: &[Filter], limit: usize) -> Vec<&'a NodeDef> {
    // FuseSearch.search: no query → everything with score 0, else Fuse (keys name,
    // display_name, search_aliases; extended search; threshold .3). Data order kept.
    let searcher = fuse::Extended::new(query, THRESHOLD);
    let mut hits: Vec<(&NodeDef, f64)> = defs
        .iter()
        .filter_map(|d| {
            if query.is_empty() {
                return Some((*d, 0.0));
            }
            let aliases: Vec<&str> = d.search_aliases.iter().map(String::as_str).collect();
            fuse::score_object(&searcher, &[vec![&d.name], vec![&d.display_name], aliases]).map(|s| (*d, s))
        })
        .collect();
    let q = query.to_lowercase();
    let mut ranked: Vec<(Aux, &NodeDef)> = hits.drain(..).map(|(d, s)| (calc_aux_scores(&q, d, s), d)).collect();
    ranked.sort_by(|a, b| compare_aux(&a.0, &b.0)); // stable, like Array.sort
    let matched: Vec<&NodeDef> = ranked.into_iter().map(|(_, d)| d).collect();

    let mut results: Vec<&NodeDef> =
        matched.iter().copied().filter(|d| filters.iter().all(|f| filter_matches(f.kind.options(d), &f.value, None))).collect();
    let valid: HashSet<&str> = results.iter().map(|d| d.name.as_str()).collect();
    let wild: Vec<&NodeDef> = matched
        .iter()
        .copied()
        .filter(|d| !valid.contains(d.name.as_str()))
        .filter(|d| filters.iter().all(|f| filter_matches(f.kind.options(d), &f.value, Some("*"))))
        .collect();
    results.extend(wild);
    results.truncate(limit);
    results
}

/// Search used by the box: `searchResults` of NodeSearchContent.vue.
pub fn search_results<'a>(reg: &'a Registry, query: &str, filters: &[Filter]) -> Vec<&'a NodeDef> {
    if query.is_empty() && filters.is_empty() {
        return top_node_defs(reg);
    }
    search_node(&visible(reg), query, filters, 64)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn reg() -> Registry {
        let v = serde_json::json!({
            "KSampler": {"input":{"required":{"model":["MODEL",{}],"latent_image":["LATENT",{}]}},"output":["LATENT"],"category":"sampling","python_module":"nodes","display_name":"KSampler"},
            "KSamplerAdvanced": {"input":{"required":{"model":["MODEL",{}]}},"output":["LATENT"],"category":"sampling","python_module":"nodes","display_name":"KSampler (Advanced)"},
            "CheckpointLoaderSimple": {"input":{"required":{}},"output":["MODEL","CLIP","VAE"],"category":"loaders","python_module":"nodes","display_name":"Load Checkpoint"},
            "Reroute": {"input":{"required":{"v":["*",{}]}},"output":["*"],"category":"utils","python_module":"nodes","display_name":"Reroute"},
            "OldThing": {"input":{"required":{"model":["MODEL",{}]}},"output":["MODEL"],"category":"loaders","python_module":"nodes","display_name":"Old (DEPRECATED)"},
            "Gone": {"input":{"required":{}},"output":["MODEL"],"category":"loaders","python_module":"nodes","deprecated":true},
            "Impact": {"input":{"required":{}},"output":["MODEL"],"category":"impact/x","python_module":"custom_nodes.ComfyUI-Impact-Pack","display_name":"Impact Thing"}
        });
        Registry::from_object_info(&v)
    }

    #[test]
    fn empty_search_is_frequency_order() {
        let r = reg();
        let top: Vec<&str> = top_node_defs(&r).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(top, ["KSampler", "CheckpointLoaderSimple", "Reroute", "KSamplerAdvanced"]); // frequency file order
    }

    #[test]
    fn type_filter_ranks_by_frequency_and_wildcards_follow() {
        let r = reg();
        let f = [Filter { kind: FilterKind::Input, value: "MODEL".into() }];
        let names: Vec<&str> = search_node(&visible(&r), "", &f, 64).iter().map(|d| d.name.as_str()).collect();
        // KSampler (5390) before KSamplerAdvanced (unlisted); the `*` input node comes last;
        // deprecated-looking name sinks below the rest of the matches; hidden defs are gone.
        assert_eq!(names, ["KSampler", "KSamplerAdvanced", "OldThing", "Reroute"]);
    }

    #[test]
    fn query_search() {
        let r = reg();
        let n = |q: &str| search_node(&visible(&r), q, &[], 64).iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(n("ksampler")[0], "KSampler");
        assert_eq!(n("load ckpt")[0], "CheckpointLoaderSimple");
        assert!(n("zzzzzz").is_empty());
        assert!(!n("gone").contains(&"Gone".to_string()));
    }
}
