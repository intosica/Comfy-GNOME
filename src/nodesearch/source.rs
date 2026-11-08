// SPDX-License-Identifier: GPL-3.0-only
//! Port of `packages/object-info-parser/src/classifiers/nodeSource.ts` (frontend 1.60.0).
//! Essentials (`essentials_category`) is not ported: the frontend gates it behind a nightly
//! feature flag; add an `Essentials` variant here and in `content.rs` if it ships.

pub const BLUEPRINT_CATEGORY: &str = "Subgraph Blueprints";
const CORE_NODE_MODULES: [&str; 3] = ["nodes", "comfy_extras", "comfy_api_nodes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Core,
    CustomNodes,
    Blueprint,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct NodeSource {
    pub kind: SourceType,
    pub display_text: String,
}

/// `shortenNodeName`: strip the Comfy prefix/suffix custom-node repos carry.
fn shorten(name: &str) -> String {
    let mut n = name;
    for p in ["ComfyUI-", "ComfyUI_", "Comfy-", "Comfy_"] {
        if let Some(r) = n.strip_prefix(p) {
            n = r;
            break;
        }
    }
    for s in ["-ComfyUI", "_ComfyUI", "-Comfy", "_Comfy"] {
        if let Some(r) = n.strip_suffix(s) {
            n = r;
            break;
        }
    }
    n.to_owned()
}

/// `getNodeSource(python_module)`.
pub fn node_source(python_module: &str) -> NodeSource {
    let unknown = NodeSource { kind: SourceType::Unknown, display_text: "Unknown".into() };
    let mut parts = python_module.split('.');
    match parts.next() {
        None | Some("") => unknown,
        Some(m) if CORE_NODE_MODULES.contains(&m) => NodeSource { kind: SourceType::Core, display_text: "Comfy Core".into() },
        Some("blueprint") => NodeSource { kind: SourceType::Blueprint, display_text: "Blueprint".into() },
        Some("custom_nodes") => match parts.next() {
            Some(m) if !m.is_empty() => NodeSource { kind: SourceType::CustomNodes, display_text: shorten(m.split('@').next().unwrap_or(m)) },
            _ => unknown,
        },
        _ => unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources() {
        assert_eq!(node_source("nodes").kind, SourceType::Core);
        assert_eq!(node_source("comfy_extras.nodes_mask").display_text, "Comfy Core");
        let c = node_source("custom_nodes.ComfyUI-Impact-Pack@1.0.nodes");
        assert_eq!((c.kind, c.display_text.as_str()), (SourceType::CustomNodes, "Impact-Pack"));
        assert_eq!(node_source("").kind, SourceType::Unknown);
    }
}
