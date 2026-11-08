// SPDX-License-Identifier: GPL-3.0-only
//! Node Library sidebar panel (`reference/Node Library.png`): searchable category tree over
//! `/object_info`. Clicking a node adds it at the centre of the visible canvas.
//!
//! ponytail: one expander per top-level category, children built on first expand. Bookmarks,
//! blueprints and per-extension sections of the web panel are not mirrored — the tree already
//! reaches every node, and `Add Node…` (double-click on empty canvas) still exists.

use super::*;

/// Roughly how many search hits the flat result list shows before it stops.
const MAX_HITS: usize = 200;

/// Owned row data so nothing borrows the registry once the widgets outlive the call.
#[derive(Clone)]
struct Row {
    name: String,
    label: String,
    category: String,
    description: String,
}

fn rows(ws: &Workspace, q: &str) -> Vec<Row> {
    let reg = ws.registry.borrow();
    let needle = q.trim().to_lowercase();
    let mut out: Vec<Row> = reg
        .by_category()
        .into_values()
        .flatten()
        .filter(|d| needle.is_empty() || format!("{} {} {}", d.display_name, d.name, d.category).to_lowercase().contains(&needle))
        .map(|d| Row {
            name: d.name.clone(),
            label: d.display_name.clone(),
            category: d.category.clone(),
            description: d.description.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

pub(super) fn page(ws: &Workspace) -> gtk::Widget {
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search nodes")
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .margin_start(8)
        .margin_end(8)
        .margin_bottom(4)
        .css_classes(["dim-label", "caption"])
        .build();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.set_margin_bottom(8);
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).child(&list).build();
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.append(&search);
    page.append(&status);
    page.append(&scroller);

    let fill: Rc<dyn Fn(&str)> = {
        let (ws, list, status) = (ws.clone(), list.clone(), status.clone());
        Rc::new(move |q: &str| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let all = rows(&ws, q);
            if all.is_empty() && ws.registry.borrow().len() == 0 {
                status.set_text("Not connected — no node types yet.");
                return;
            }
            if q.trim().is_empty() {
                // Group by the first category segment, like the web panel's COMFY NODES section.
                let mut groups: std::collections::BTreeMap<String, Vec<Row>> = Default::default();
                for r in all.iter() {
                    let top = r.category.split('/').next().unwrap_or("other").to_owned();
                    groups.entry(top).or_default().push(r.clone());
                }
                status.set_text(&format!("{} nodes · {} categories", all.len(), groups.len()));
                for (group, defs) in groups {
                    let boxed = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    let exp = gtk::Expander::builder()
                        .label(&group)
                        .margin_start(6)
                        .margin_end(6)
                        .css_classes(["node-category"])
                        .child(&boxed)
                        .build();
                    let (ws2, boxed2) = (ws.clone(), boxed.clone());
                    let filled = Rc::new(std::cell::Cell::new(false));
                    exp.connect_expanded_notify(move |e| {
                        if !e.is_expanded() || filled.replace(true) {
                            return;
                        }
                        fill_nodes(&ws2, &boxed2, &defs);
                    });
                    list.append(&exp);
                }
            } else {
                let total = all.len();
                let shown = &all[..total.min(MAX_HITS)];
                status.set_text(&if total > MAX_HITS { format!("{MAX_HITS} of {total} matches") } else { format!("{total} matches") });
                fill_nodes(&ws, &list, shown);
            }
        })
    };

    fill("");
    {
        let fill = fill.clone();
        search.connect_search_changed(move |e| fill(&e.text()));
    }
    {
        // Rows appear when the registry fills up (connect / reload), like the workflow list.
        let (fill, search) = (fill.clone(), search.clone());
        page.connect_map(move |_| fill(&search.text()));
    }
    page.upcast()
}

/// One flat button per node; the click adds the node at the centre of the viewport.
fn fill_nodes(ws: &Workspace, parent: &gtk::Box, defs: &[Row]) {
    for d in defs {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_start(8);
        row.set_margin_end(4);
        row.append(&gtk::Label::builder().label(&d.label).xalign(0.0).build());
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
        row.append(&gtk::Label::builder().label(d.category.replace('/', " › ")).xalign(1.0).css_classes(["dim-label", "caption"]).build());
        let btn = gtk::Button::builder().css_classes(["flat", "node-row"]).child(&row).tooltip_text(d.description.clone()).build();
        let (ws, ty) = (ws.clone(), d.name.clone());
        btn.connect_clicked(move |_| add_node(&ws, &ty));
        parent.append(&btn);
    }
}

fn add_node(ws: &Workspace, ty: &str) {
    let Some(doc) = ws.doc_or_blank("nodes") else { return };
    let (zoom, off) = doc.borrow().canvas.view();
    let area = doc.borrow().canvas.area.clone();
    // A brand new tab has no allocated area yet; park the node near the origin instead.
    let pos = if area.width() > 1 { ((area.width() as f64 / 2.0 - off.0) / zoom, (area.height() as f64 / 2.0 - off.1) / zoom) } else { (120.0, 120.0) };
    ws.create_node(&doc, ty, pos);
}
