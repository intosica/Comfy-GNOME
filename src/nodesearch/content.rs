// SPDX-License-Identifier: GPL-3.0-only
//! Port of the state logic in `src/components/searchbox/v2/NodeSearchContent.vue` and
//! `NodeSearchCategorySidebar.vue` (frontend 1.60.0): root-category chips, the category tree
//! and which nodes the results list shows. No GTK here; `workspace/addnode.rs` renders it.
//! Not ported: Favorites (the frontend's node bookmarks have no native counterpart yet)
//! and Essentials (see `source.rs`).

use super::source::{node_source, SourceType, BLUEPRINT_CATEGORY};
use super::{search_results, top_node_defs, visible, Filter};
use crate::registry::{NodeDef, Registry};
use std::collections::HashSet;

/// `DEFAULT_CATEGORY`.
pub const DEFAULT_CATEGORY: &str = "most-relevant";

/// `RootCategory` ids (`rootCategories.ts`) that are available here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Blueprint,
    Comfy,
    Partner,
    Custom,
}

impl Root {
    /// `rootFilter` value, used as the tree key prefix (`rootKey`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Blueprint => BLUEPRINT_CATEGORY,
            Self::Comfy => "comfy",
            Self::Partner => "partner-nodes",
            Self::Custom => "custom",
        }
    }
    /// `rootFilterLabel` / chip text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Blueprint => "Blueprints",
            Self::Comfy => "Comfy",
            Self::Partner => "Partner",
            Self::Custom => "Extensions",
        }
    }
    /// `sourceCategoryFilters` + the switch in `rootFilteredNodeDefs`.
    fn contains(self, d: &NodeDef) -> bool {
        match self {
            Self::Blueprint => d.category.starts_with(BLUEPRINT_CATEGORY),
            Self::Comfy => node_source(&d.python_module).kind == SourceType::Core,
            Self::Partner => d.api_node,
            Self::Custom => node_source(&d.python_module).kind == SourceType::CustomNodes,
        }
    }
}

/// `nodeAvailability`: which chips exist, in the frontend's chip order (Comfy always).
pub fn available_roots(reg: &Registry) -> Vec<Root> {
    let vis = visible(reg);
    [Root::Blueprint, Root::Comfy, Root::Partner, Root::Custom]
        .into_iter()
        .filter(|r| *r == Root::Comfy || vis.iter().any(|d| r.contains(d)))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Content {
    pub query: String,
    pub category: String,
    pub root: Option<Root>,
    pub filters: Vec<Filter>,
}

impl Default for Content {
    fn default() -> Self {
        Self { query: String::new(), category: DEFAULT_CATEGORY.into(), root: None, filters: vec![] }
    }
}

/// `CategoryNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct CatNode {
    pub key: String,
    pub label: String,
    pub children: Vec<CatNode>,
}

impl Content {
    /// `onSelectCategory(root)`: radio toggle, resets the category.
    pub fn toggle_root(&mut self, r: Root) {
        self.root = if self.root == Some(r) { None } else { Some(r) };
        self.category = DEFAULT_CATEGORY.into();
    }

    /// `rootFilteredNodeDefs`.
    pub fn root_filtered<'a>(&self, reg: &'a Registry) -> Vec<&'a NodeDef> {
        let all = visible(reg);
        match self.root {
            Some(r) => all.into_iter().filter(|d| r.contains(d)).collect(),
            None => all,
        }
    }

    fn has_search(&self) -> bool {
        !self.query.is_empty() || !self.filters.is_empty()
    }

    /// `displayedResults`.
    pub fn displayed<'a>(&self, reg: &'a Registry) -> Vec<&'a NodeDef> {
        let base = self.root_filtered(reg);
        let in_root = |res: Vec<&'a NodeDef>| -> Vec<&'a NodeDef> {
            if self.root.is_none() {
                return res;
            }
            let set: HashSet<&str> = base.iter().map(|d| d.name.as_str()).collect();
            res.into_iter().filter(|d| set.contains(d.name.as_str())).collect()
        };
        if self.category == DEFAULT_CATEGORY {
            // getMostRelevantResults
            if self.has_search() {
                return in_root(search_results(reg, &self.query, &self.filters));
            }
            return if self.root.is_some() { base } else { top_node_defs(reg) };
        }
        let source = if self.has_search() { in_root(search_results(reg, &self.query, &self.filters)) } else { base.clone() };
        // sourceCategoryFilters: the root key itself selected = every node of that source.
        match self.category.as_str() {
            "comfy" => return source.into_iter().filter(|d| Root::Comfy.contains(d)).collect(),
            "custom" => return source.into_iter().filter(|d| Root::Custom.contains(d)).collect(),
            _ => {}
        }
        // getCategoryResults
        if self.root.is_some_and(|r| r.id() == self.category) {
            return source;
        }
        let prefix = self.root.map(|r| format!("{}/", r.id())).unwrap_or_default();
        let path = self.category.strip_prefix(&prefix).unwrap_or(&self.category);
        source
            .into_iter()
            .filter(|d| {
                let c = d.category.strip_prefix(&prefix).unwrap_or(&d.category);
                c == path || c.starts_with(&format!("{path}/"))
            })
            .collect()
    }

    /// `anyTreeCategoryHasChildren` (chevrons are hidden when no category is nested).
    pub fn any_nested(&self, reg: &Registry) -> bool {
        self.root_filtered(reg).iter().any(|d| d.category.contains('/'))
    }

    /// `categoryTree` of NodeSearchCategorySidebar.vue: categories in first-seen (server)
    /// order, wrapped in one labelled root when a chip is active and several roots exist.
    pub fn tree(&self, reg: &Registry) -> Vec<CatNode> {
        let mut roots: Vec<CatNode> = vec![];
        for d in self.root_filtered(reg) {
            if d.category.is_empty() {
                continue;
            }
            let mut level = &mut roots;
            let mut key = String::new();
            for seg in d.category.split('/') {
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str(seg);
                let i = match level.iter().position(|n| n.label == seg) {
                    Some(i) => i,
                    None => {
                        level.push(CatNode { key: key.clone(), label: seg.to_owned(), children: vec![] });
                        level.len() - 1
                    }
                };
                level = &mut level[i].children;
            }
        }
        match self.root {
            Some(r) if roots.len() > 1 => {
                fn prefix(n: CatNode, k: &str) -> CatNode {
                    CatNode { key: format!("{k}/{}", n.key), label: n.label, children: n.children.into_iter().map(|c| prefix(c, k)).collect() }
                }
                let k = r.id();
                vec![CatNode { key: k.into(), label: r.label().into(), children: roots.into_iter().map(|n| prefix(n, k)).collect() }]
            }
            _ => roots,
        }
    }
}

/// `parentCategory`.
pub fn parent_category(key: &str) -> String {
    key.rfind('/').filter(|i| *i > 0).map(|i| key[..i].to_owned()).unwrap_or_default()
}

/// `selectCategory` toggle: returns the new expanded category.
pub fn toggle_expanded(expanded: &str, id: &str) -> String {
    if expanded == id { parent_category(id) } else { id.to_owned() }
}

/// NodeSearchCategoryTreeNode `isExpanded`.
pub fn is_expanded(expanded: &str, key: &str) -> bool {
    expanded == key || expanded.starts_with(&format!("{key}/"))
}

#[cfg(test)]
mod tests {
    use super::super::tests::reg;
    use super::*;

    #[test]
    fn roots_tree_and_results() {
        let r = reg();
        assert_eq!(available_roots(&r), [Root::Comfy, Root::Custom]);
        let mut c = Content::default();
        assert_eq!(c.tree(&r).iter().map(|n| n.label.as_str()).collect::<Vec<_>>(), ["sampling", "loaders", "utils", "impact"]);
        c.toggle_root(Root::Custom);
        assert_eq!(c.displayed(&r).len(), 1);
        c.toggle_root(Root::Comfy);
        let t = c.tree(&r);
        assert_eq!((t.len(), t[0].key.as_str(), t[0].children[0].key.as_str()), (1, "comfy", "comfy/sampling"));
        c.category = "comfy/sampling".into();
        assert_eq!(c.displayed(&r).len(), 2);
        c.category = "comfy".into();
        assert_eq!(c.displayed(&r).len(), 5); // every visible core node
    }

    #[test]
    fn expand_toggle() {
        assert_eq!(toggle_expanded("a/b", "a/b"), "a");
        assert_eq!(toggle_expanded("a", "a/b"), "a/b");
        assert!(is_expanded("a/b", "a") && !is_expanded("ab", "a"));
    }
}
