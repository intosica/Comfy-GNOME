// SPDX-License-Identifier: GPL-3.0-only
//! Right-hand properties panel — the web frontend's "Toggle properties panel"
//! (`reference/Toggle Properties Panel/`).
//!
//! Parameters / Info / Settings for the selected node, plus the subgraph input editor
//! (SHOWN / HIDDEN inputs with promote & demote). Values are edited through the same
//! widget keys the node body uses (`graph_view::build_widgets`), so the panel and the
//! canvas never disagree about where a value lives: a promoted widget keeps its value on
//! the *instance*, an inner widget on the inner node.

use super::*;
use crate::canvas::widgets::{WKind, Widget};
use crate::graph_view;
use crate::prompt::{self, Role};
use crate::workflow::{self, Subgraph};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const SW: i32 = 34;
const SH: i32 = 20;
const GAP: i32 = 6;

#[derive(Clone)]
pub(super) struct Panel {
    pub root: gtk::Box,
    title: gtk::Label,
    pencil: gtk::Button,
    /// Panel-header toggle for the subgraph input editor (the web's `settings-2` button).
    sub_btn: gtk::ToggleButton,
    search: gtk::SearchEntry,
    stack: gtk::Stack,
    tab_info: gtk::ToggleButton,
    params: gtk::Box,
    info: gtk::Box,
    settings: gtk::Box,
    editor: gtk::Box,
    state: Rc<State>,
}

#[derive(Default)]
struct State {
    /// What `refresh` last rendered (see `key`); a tick re-renders when it changes.
    key: RefCell<String>,
    /// The pointer/focus is inside the panel: rebuilding would kill an in-progress edit.
    focus: Cell<bool>,
    /// Rows of the Parameters page, for the search filter.
    rows: RefCell<Vec<(gtk::Widget, String)>>,
    /// Tab shown before the subgraph editor took the stack.
    tab: RefCell<String>,
    /// Rename handler of the title pencil (re-bound per node).
    pencil: RefCell<Option<glib::SignalHandlerId>>,
}

/// Everything the panel depends on; cheap enough to compute per frame (nothing is rebuilt
/// unless it changed).
fn key(ws: &Workspace) -> String {
    let Some(doc) = ws.active_doc() else { return "none".into() };
    let d = doc.borrow();
    let sel = d.canvas.selected();
    let graph = graph_ref(&d);
    let node = sel.last().and_then(|id| graph.nodes.iter().find(|n| &n.id == id));
    let io = node.filter(|n| d.wf.subgraphs.contains_key(&n.ty)).map(|n| d.wf.subgraphs[&n.ty].inputs.len());
    format!(
        "{:p}|{}|{:?}|{}|{}|{io:?}|{}",
        Rc::as_ptr(&doc) as *const u8,
        d.path.join("/"),
        sel,
        ws.registry.borrow().nodes.len(),
        node.map(|n| n.inputs.len()).unwrap_or(0),
        node.map(|n| n.widgets_values.to_string()).unwrap_or_default(),
    )
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["prop-heading"]).margin_top(4).build()
}

/// A node's label for the editor: its title, else the registry's display name.
fn nice_name(n: &workflow::Node, reg: &crate::registry::Registry) -> String {
    n.title
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| reg.get(&n.ty).map(|d| d.display_name.clone()))
        .unwrap_or_else(|| n.ty.clone())
}

fn heading_row(text: &str, action: &gtk::Button) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(4);
    let h = heading(text);
    h.set_hexpand(true);
    row.append(&h);
    row.append(action);
    row.upcast()
}

fn round(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

// ---------------------------------------------------------------------------------------
// Construction (no `Workspace` yet: the callbacks are created in `refresh`).
// ---------------------------------------------------------------------------------------

pub(super) fn build() -> Panel {
    let title = gtk::Label::builder()
        .label("Properties")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["heading"])
        .build();
    let pencil = gtk::Button::builder()
        .icon_name("cg-document-edit-symbolic")
        .tooltip_text("Rename Node")
        .css_classes(["flat"])
        .sensitive(false)
        .build();
    let sub_btn = gtk::ToggleButton::builder()
        .icon_name("cg-emblem-system-symbolic")
        .tooltip_text("Edit Subgraph Inputs")
        .css_classes(["flat"])
        .visible(false)
        .build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.set_margin_start(10);
    head.set_margin_end(6);
    head.set_margin_top(6);
    head.append(&title);
    head.append(&pencil);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    head.append(&sub_btn);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search Inputs")
        .margin_start(10)
        .margin_end(10)
        .margin_bottom(4)
        .build();
    let params = gtk::Box::new(gtk::Orientation::Vertical, 8);
    params.set_margin_start(10);
    params.set_margin_end(10);
    params.set_margin_top(2);
    params.set_margin_bottom(14);
    let params_sc = gtk::ScrolledWindow::builder()
        .child(&params)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let params_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    params_page.append(&search);
    params_page.append(&params_sc);

    let page = |b: &gtk::Box| {
        let sc = gtk::ScrolledWindow::builder().child(b).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        sc.upcast::<gtk::Widget>()
    };
    let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
    info.set_margin_start(12);
    info.set_margin_end(12);
    info.set_margin_top(4);
    info.set_margin_bottom(14);
    let settings = gtk::Box::new(gtk::Orientation::Vertical, 8);
    settings.set_margin_start(12);
    settings.set_margin_end(12);
    settings.set_margin_top(4);
    settings.set_margin_bottom(14);
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 8);
    editor.set_margin_start(12);
    editor.set_margin_end(12);
    editor.set_margin_top(4);
    editor.set_margin_bottom(14);

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();
    stack.add_named(&params_page, Some("params"));
    stack.add_named(&page(&info), Some("info"));
    stack.add_named(&page(&settings), Some("settings"));
    stack.add_named(&page(&editor), Some("subgraph"));
    // Compact left-aligned tabs, not a stretched StackSwitcher: with the width the web
    // reserves for the panel the switcher's buttons fill the whole strip.
    let tab = |label: &str| gtk::ToggleButton::builder().label(label).css_classes(["flat", "prop-tab"]).build();
    let tab_params = tab("Parameters");
    let tab_info = tab("Info");
    let tab_settings = tab("Settings");
    tab_params.set_active(true);
    tab_info.set_group(Some(&tab_params));
    tab_settings.set_group(Some(&tab_params));
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    tabs.set_margin_start(8);
    tabs.set_halign(gtk::Align::Start);
    tabs.append(&tab_params);
    tabs.append(&tab_info);
    tabs.append(&tab_settings);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.add_css_class("prop-panel");
    root.append(&head);
    root.append(&tabs);
    root.append(&stack);

    let state = Rc::new(State::default());
    *state.tab.borrow_mut() = "params".into();

    // Search: hide rows that do not match (the rows are rebuilt by `refresh`).
    {
        let st = state.clone();
        search.connect_search_changed(move |e| {
            let q = e.text().to_lowercase();
            for (w, text) in st.rows.borrow().iter() {
                w.set_visible(q.is_empty() || text.to_lowercase().contains(&q));
            }
        });
    }
    // Focus guard: a rebuild while an entry has focus would drop the caret.
    {
        let st = state.clone();
        let f = gtk::EventControllerFocus::new();
        let st2 = st.clone();
        f.connect_enter(move |_| st2.focus.set(true));
        f.connect_leave(move |_| st.focus.set(false));
        root.add_controller(f);
    }
    // Tab clicks drive the stack; the stack also shows the subgraph editor, where the
    // tabs (like the web's) are unavailable.
    for (b, page) in [(&tab_params, "params"), (&tab_info, "info"), (&tab_settings, "settings")] {
        let stack = stack.clone();
        b.connect_clicked(move |_| stack.set_visible_child_name(page));
    }
    {
        let (t1, t2, t3) = (tab_params.clone(), tab_info.clone(), tab_settings.clone());
        stack.connect_visible_child_name_notify(move |s| {
            let cur = s.visible_child_name().unwrap_or_default();
            t1.set_active(cur == "params");
            t2.set_active(cur == "info");
            t3.set_active(cur == "settings");
        });
    }
    // Subgraph editor toggle: the tabs go dim (screen: "Unkown Button Action on Subgraph").
    {
        let (st, stack) = (state.clone(), stack.clone());
        let (t1, t2, t3) = (tab_params.clone(), tab_info.clone(), tab_settings.clone());
        sub_btn.connect_toggled(move |b| {
            if b.is_active() {
                let cur = stack.visible_child_name().map(|s| s.to_string()).unwrap_or_else(|| "params".into());
                if cur != "subgraph" {
                    *st.tab.borrow_mut() = cur;
                }
                stack.set_visible_child_name("subgraph");
                for t in [&t1, &t2, &t3] {
                    t.set_sensitive(false);
                }
            } else {
                stack.set_visible_child_name(&st.tab.borrow());
                for t in [&t1, &t2, &t3] {
                    t.set_sensitive(true);
                }
            }
        });
    }

    Panel { root, title, pencil, sub_btn, search, stack, tab_info, params, info, settings, editor, state }
}

// ---------------------------------------------------------------------------------------
// Filling the panel for the current selection.
// ---------------------------------------------------------------------------------------

impl Panel {
    /// Re-render when the selection / node / values changed since the last render.
    pub(super) fn sync(&self, ws: &Workspace) {
        if !self.root.is_mapped() {
            // Refresh when the panel is shown again.
            *self.state.key.borrow_mut() = String::new();
            return;
        }
        if self.state.focus.get() {
            return;
        }
        if key(ws) != *self.state.key.borrow() {
            self.refresh(ws);
        }
    }

    pub(super) fn refresh(&self, ws: &Workspace) {
        for b in [&self.params, &self.info, &self.settings, &self.editor] {
            clear(b);
        }
        self.state.rows.borrow_mut().clear();
        *self.state.key.borrow_mut() = key(ws);
        if let Some(h) = self.state.pencil.borrow_mut().take() {
            self.pencil.disconnect(h);
        }
        self.pencil.set_sensitive(false);
        self.sub_btn.set_visible(false);
        self.tab_info.set_visible(true);
        if self.sub_btn.is_active() {
            self.sub_btn.set_active(false);
        }
        let q = self.search.text().to_lowercase();

        let Some(doc) = ws.active_doc() else {
            if self.stack.visible_child_name().as_deref() == Some("subgraph") {
                self.stack.set_visible_child_name("params");
            }
            self.title.set_label("Properties");
            self.params.append(&dim("No workflow open"));
            self.info.append(&dim("No workflow open"));
            return;
        };
        let reg = ws.registry.borrow();
        let d = doc.borrow();
        let graph = graph_ref(&d);
        let sel = d.canvas.selected();
        let node = sel.last().and_then(|id| graph.nodes.iter().find(|n| &n.id == id)).cloned();
        let Some(node) = node else {
            if self.stack.visible_child_name().as_deref() == Some("subgraph") {
                self.stack.set_visible_child_name("params");
            }
            self.title.set_label("Properties");
            self.params.append(&dim("Select a node to see its parameters"));
            self.info.append(&dim("Select a node"));
            return;
        };
        let sg = d.wf.subgraphs.get(&node.ty).cloned();
        self.tab_info.set_visible(sg.is_none());
        if sg.is_some() && self.stack.visible_child_name().as_deref() == Some("info") {
            // No Info page for subgraph nodes (the web hides that tab).
            self.stack.set_visible_child_name("params");
        }
        let name = node
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| sg.as_ref().map(|s| s.name.clone()))
            .or_else(|| reg.get(&node.ty).map(|def| def.display_name.clone()))
            .unwrap_or_else(|| node.ty.clone());
        self.title.set_label(&name);
        self.title.set_tooltip_text(Some(&node.ty));

        // Rename (display title only, like the web's row-menu Rename).
        {
            let (ws, doc, id, cur) = (ws.clone(), doc.clone(), node.id.clone(), name.clone());
            let h = self.pencil.connect_clicked(move |_| {
                let (ws, doc, id) = (ws.clone(), doc.clone(), id.clone());
                let me = ws.clone();
                ws.name_dialog("Rename Node", "Rename", &cur, move |t| me.set_title(&doc, &id, t));
            });
            *self.state.pencil.borrow_mut() = Some(h);
            self.pencil.set_sensitive(true);
        }

        // Parameters: the same rows the canvas draws, split at `advanced`. A subgraph
        // instance additionally lists every interior widget that is not promoted
        // (ADVANCED INPUTS), editable in place.
        let mut images = Vec::new();
        let promoted = graph_view::build_widgets(&d.wf, graph, &node, &reg, &mut images);
        let interior = graph_view::subgraph_advanced_widgets(&d.wf, &node, &reg);
        let mut normal: Vec<gtk::Widget> = Vec::new();
        let mut normal_reset: Vec<Rc<dyn Fn()>> = Vec::new();
        let mut advanced: Vec<gtk::Widget> = Vec::new();
        let mut advanced_reset: Vec<Rc<dyn Fn()>> = Vec::new();
        for w in &promoted {
            let Some(row) = self.row(ws, &doc, &node, w, sg.as_ref(), &q) else { continue };
            let reset = self.reset_action(ws, &doc, &node, w, sg.as_ref());
            // A subgraph instance's promoted rows are INPUTS however the registry marks
            // them; a plain node keeps the registry's advanced split.
            if sg.is_none() && w.advanced {
                advanced.push(row);
                advanced_reset.extend(reset);
            } else {
                normal.push(row);
                normal_reset.extend(reset);
            }
        }
        for w in &interior {
            let Some(row) = self.row(ws, &doc, &node, w, sg.as_ref(), &q) else { continue };
            advanced.push(row);
            advanced_reset.extend(self.reset_action(ws, &doc, &node, w, sg.as_ref()));
        }
        if normal.is_empty() {
            normal.push(dim("No inputs").upcast());
        }
        self.params.append(&self.section("INPUTS", true, normal, normal_reset));
        if !advanced.is_empty() {
            self.params.append(&self.section("ADVANCED INPUTS", false, advanced, advanced_reset));
        }

        self.fill_info(&node, &reg, sg.as_ref());
        self.fill_settings(ws, &doc, &node, &sel);
        self.sub_btn.set_visible(sg.is_some());
        if sg.is_none() && self.stack.visible_child_name().as_deref() == Some("subgraph") {
            self.stack.set_visible_child_name("params");
        }
        if let Some(sg) = &sg {
            self.fill_editor(ws, &doc, &node, sg, &reg);
        }
    }

    fn section(
        &self,
        label: &str,
        expanded: bool,
        rows: Vec<gtk::Widget>,
        resets: Vec<Rc<dyn Fn()>>,
    ) -> gtk::Widget {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.set_margin_top(2);
        for r in rows {
            list.append(&r);
        }
        let exp = gtk::Expander::builder().expanded(expanded).css_classes(["prop-section"]).build();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let l = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build();
        head.append(&l);
        if !resets.is_empty() {
            let all = resets.clone();
            let b = gtk::Button::builder()
                .icon_name("view-refresh-symbolic")
                .tooltip_text("Reset All to Defaults")
                .css_classes(["flat", "prop-eye"])
                .build();
            b.connect_clicked(move |_| {
                for f in &all {
                    f();
                }
            });
            head.append(&b);
        }
        exp.set_label_widget(Some(&head));
        exp.set_child(Some(&list));
        exp.upcast()
    }

    /// One parameter row: name + caption, the editor, and the actions menu.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        ws: &Workspace,
        doc: &DocRef,
        node: &workflow::Node,
        w: &Widget,
        _sg: Option<&Subgraph>,
        query: &str,
    ) -> Option<gtk::Widget> {
        let ed = editor(w, commit(ws, doc, node, w))?;

        // Source node, as the web's right-aligned caption; the socket type when the row
        // is the node's own widget.
        let ty = w.input.and_then(|i| node.inputs.get(i)).map(|i| i.ty.clone()).unwrap_or_default();
        let caption = if w.source.is_empty() { ty } else { w.source.clone() };

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.append(
            &gtk::Label::builder()
                .label(&w.label)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["prop-name"])
                .build(),
        );
        if !caption.is_empty() {
            top.append(
                &gtk::Label::builder()
                    .label(&caption)
                    .xalign(1.0)
                    .hexpand(false)
                    .max_width_chars(18)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["prop-type"])
                    .build(),
            );
        }
        let rename = rename_target(ws, node, w);
        let promote = promote_target(ws, node, w);

        // Eye: promoted rows demote by their public input name.
        if w.key.starts_with("w:") {
            let name = w.input.and_then(|i| node.inputs.get(i)).map(|i| i.name.clone()).unwrap_or_default();
            let (ws, doc, sg_ty) = (ws.clone(), doc.clone(), node.ty.clone());
            let b = gtk::Button::builder()
                .icon_name("cg-lucide-eye-off-symbolic")
                .tooltip_text("Hide Input")
                .css_classes(["flat", "prop-eye"])
                .build();
            b.connect_clicked(move |_| {
                ws.demote_input(&doc, &sg_ty, &name);
                later(&ws);
            });
            top.append(&b);
        }

        // Per-row menu: Rename / Show Input / Reset to Default (screen: "Three Dots").
        if rename.is_some() || promote.is_some() || reset_value(w).is_some() {
            top.append(&self.actions_menu(ws, doc, node, w));
        }

        let row = gtk::Box::new(gtk::Orientation::Vertical, 3);
        row.append(&top);
        row.append(&ed);
        row.set_visible(query.is_empty() || w.label.to_lowercase().contains(query));
        self.state.rows.borrow_mut().push((row.clone().upcast(), w.label.clone()));
        Some(row.upcast())
    }

    fn actions_menu(
        &self,
        ws: &Workspace,
        doc: &DocRef,
        node: &workflow::Node,
        w: &Widget,
    ) -> gtk::MenuButton {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        pop.set_child(Some(&list));

        // Items are built when the menu opens, not when the row is built: a value edited
        // in place (or a rename) must not leave a stale "Reset to Default" behind.
        let (ws2, doc2, node2, snap) =
            (ws.clone(), doc.clone(), node.clone(), w.clone());
        let fill: Rc<dyn Fn()> = {
            let (list, pop) = (list.clone(), pop.clone());
            Rc::new(move || {
                clear(&list);
                let w = live_widget(&ws2, &doc2, &node2, &snap.key).unwrap_or_else(|| snap.clone());
                let rename = rename_target(&ws2, &node2, &w);
                let promote = promote_target(&ws2, &node2, &w);
                if let Some(t) = rename.clone() {
                    let (ws, doc, cur) = (ws2.clone(), doc2.clone(), w.label.clone());
                    let p = pop.clone();
                    list.append(&menu_item(
                        "Rename",
                        true,
                        Box::new(move || {
                            p.popdown();
                            let (ws, doc, t) = (ws.clone(), doc.clone(), t.clone());
                            let me = ws.clone();
                            ws.name_dialog("Rename Input", "Rename", &cur, move |text| {
                                {
                                    let mut d = doc.borrow_mut();
                                    let graph = match &t.sg {
                                        Some(ty) => d.wf.subgraphs.get_mut(ty).map(|s| &mut s.graph),
                                        None => Some(graph_mut(&mut d)),
                                    };
                                    if let Some(g) = graph {
                                        for n in g.nodes.iter_mut().filter(|n| n.id == t.node) {
                                            if let Some(i) = n.inputs.get_mut(t.slot) {
                                                i.label = Some(text.clone()).filter(|x| !x.is_empty());
                                            }
                                        }
                                    }
                                }
                                me.edited(&doc);
                                later(&me);
                            });
                        }),
                    ));
                }
                if let Some((inner, name)) = promote.clone() {
                    let (ws, doc, sg_id, seed) =
                        (ws2.clone(), doc2.clone(), node2.ty.clone(), w.value.clone());
                    let p = pop.clone();
                    list.append(&menu_item(
                        "Show Input",
                        true,
                        Box::new(move || {
                            p.popdown();
                            ws.promote_widget(&doc, &sg_id, &inner, &name, seed.clone());
                            later(&ws);
                        }),
                    ));
                }
                match reset_value(&w) {
                    Some(v) => {
                        let c = commit(&ws2, &doc2, &node2, &w);
                        let (ws3, p) = (ws2.clone(), pop.clone());
                        list.append(&menu_item(
                            "Reset to Default",
                            true,
                            Box::new(move || {
                                p.popdown();
                                c(v.clone());
                                later(&ws3);
                            }),
                        ));
                    }
                    None if rename.is_some() || promote.is_some() => {
                        list.append(&menu_item("Reset to Default", false, Box::new(|| {})));
                    }
                    None => {}
                }
            })
        };
        {
            let fill = fill.clone();
            pop.connect_visible_notify(move |p| {
                if p.is_visible() {
                    fill();
                }
            });
        }

        gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Widget Actions")
            .popover(&pop)
            .css_classes(["flat", "prop-eye"])
            .build()
    }

    fn reset_action(
        &self,
        ws: &Workspace,
        doc: &DocRef,
        node: &workflow::Node,
        w: &Widget,
        _sg: Option<&Subgraph>,
    ) -> Option<Rc<dyn Fn()>> {
        let v = reset_value(w)?;
        let c = commit(ws, doc, node, w);
        let ws2 = ws.clone();
        Some(Rc::new(move || {
            c(v.clone());
            later(&ws2);
        }))
    }

    fn fill_info(&self, node: &workflow::Node, reg: &crate::registry::Registry, sg: Option<&Subgraph>) {
        self.info.append(&gtk::Label::builder().label(&node.ty).xalign(0.0).wrap(true).css_classes(["prop-type"]).build());
        if let Some(sg) = sg {
            self.info.append(&dim(&format!("Subgraph “{}” — {} inputs, {} outputs", sg.name, sg.inputs.len(), sg.outputs.len())));
            self.info.append(&dim("Use the panel-header button to choose which parameters are shown on the node."));
            return;
        }
        let Some(def) = reg.get(&node.ty) else {
            self.info.append(&dim("Unknown node type"));
            return;
        };
        if !def.display_name.is_empty() {
            self.info.append(&gtk::Label::builder().label(&def.display_name).xalign(0.0).wrap(true).css_classes(["prop-name"]).build());
        }
        if !def.description.is_empty() {
            self.info.append(&dim(&def.description));
        }
        let table = |label: &str, rows: Vec<(String, String)>| {
            if rows.is_empty() {
                return;
            }
            self.info.append(&heading(label));
            let grid = gtk::Grid::builder().row_spacing(3).column_spacing(12).build();
            for (r, (n, t)) in rows.iter().enumerate() {
                grid.attach(&gtk::Label::builder().label(n).xalign(0.0).css_classes(["prop-name"]).build(), 0, r as i32, 1, 1);
                grid.attach(&gtk::Label::builder().label(t).xalign(0.0).css_classes(["prop-type"]).build(), 1, r as i32, 1, 1);
            }
            self.info.append(&grid);
        };
        table("INPUTS", def.inputs.iter().map(|i| (i.name.clone(), i.type_name().to_owned())).collect());
        table("OUTPUTS", def.outputs.iter().map(|(t, n)| (n.clone(), t.clone())).collect());
    }

    fn fill_settings(&self, ws: &Workspace, doc: &DocRef, node: &workflow::Node, sel: &[String]) {
        let ids: Vec<String> = sel.iter().filter(|id| id.as_str() != "-10" && id.as_str() != "-20").cloned().collect();
        let ids = if ids.is_empty() { vec![node.id.clone()] } else { ids };

        self.settings.append(&heading("NODE STATE"));
        let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        seg.add_css_class("linked");
        let mut group: Option<gtk::ToggleButton> = None;
        for (label, mode) in [("Normal", 0), ("Bypass", workflow::MODE_BYPASS), ("Mute", workflow::MODE_NEVER)] {
            let b = gtk::ToggleButton::builder().label(label).hexpand(true).build();
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(node.mode == mode);
            let (ws, doc, ids) = (ws.clone(), doc.clone(), ids.clone());
            b.connect_clicked(move |_| set_mode(&ws, &doc, &ids, mode));
            seg.append(&b);
        }
        self.settings.append(&seg);

        self.settings.append(&heading("NODE COLOR"));
        self.settings.append(&swatches(ws, doc, &ids, node));

        self.settings.append(&heading("PINNED"));
        let pin = gtk::Switch::builder()
            .active(node.pinned)
            .halign(gtk::Align::Start)
            .tooltip_text("Pinned Nodes Cannot Be Dragged")
            .build();
        {
            let (ws, doc, ids) = (ws.clone(), doc.clone(), ids.clone());
            pin.connect_state_set(move |_, on| {
                {
                    let mut d = doc.borrow_mut();
                    for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                        n.pinned = on;
                    }
                }
                ws.set_dirty(&doc, true);
                ws.render_doc(&doc);
                glib::Propagation::Proceed
            });
        }
        self.settings.append(&pin);
    }

    /// SHOWN / HIDDEN inputs of the selected subgraph (the panel-header toggle).
    fn fill_editor(&self, ws: &Workspace, doc: &DocRef, node: &workflow::Node, sg: &Subgraph, reg: &crate::registry::Registry) {
        let sg_id = node.ty.clone();
        let node_id = node.id.clone();

        let show_all = gtk::Button::builder().label("Show all").css_classes(["flat", "prop-eye"]).halign(gtk::Align::End).build();
        {
            let (ws, doc, node_id) = (ws.clone(), doc.clone(), node_id.clone());
            show_all.connect_clicked(move |_| {
                ws.show_all_inputs(&doc, &node_id);
                later(&ws);
            });
        }
        self.editor.append(&heading_row("SHOWN ON NODE", &show_all));
        let mut shown = 0;
        for inp in node.inputs.iter().filter(|i| i.widget.is_some()) {
            let inner = sg.inputs.iter().find(|i| i.name == inp.name).and_then(|io| {
                let l = sg.graph.links.values().find(|l| io.link_ids.contains(&l.id))?;
                Some(nice_name(sg.graph.node(&l.target_id)?, reg))
            });
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
            t.append(&gtk::Label::builder().label(&inp.name).xalign(0.0).css_classes(["prop-name"]).build());
            t.append(&dim(&inner.unwrap_or_default()));
            row.append(&t);
            let b = gtk::Button::builder()
                .icon_name("cg-lucide-eye-off-symbolic")
                .tooltip_text("Hide Input")
                .css_classes(["flat", "prop-eye"])
                .halign(gtk::Align::End)
                .hexpand(true)
                .build();
            {
                let (ws, doc, sg_id, name) = (ws.clone(), doc.clone(), sg_id.clone(), inp.name.clone());
                b.connect_clicked(move |_| {
                    ws.demote_input(&doc, &sg_id, &name);
                    later(&ws);
                });
            }
            row.append(&b);
            self.editor.append(&row);
            shown += 1;
        }
        if shown == 0 {
            self.editor.append(&dim("Nothing promoted"));
        }

        let hide_all = gtk::Button::builder().label("Hide all").css_classes(["flat", "prop-eye"]).halign(gtk::Align::End).build();
        {
            let (ws, doc, node_id) = (ws.clone(), doc.clone(), node_id);
            hide_all.connect_clicked(move |_| {
                ws.hide_all_inputs(&doc, &node_id);
                later(&ws);
            });
        }
        self.editor.append(&heading_row("HIDDEN / NESTED PARAMETERS", &hide_all));
        let mut hidden = 0;
        for inner in &sg.graph.nodes {
            let Some(def) = reg.get(&inner.ty) else { continue };
            for s in prompt::widget_slots(inner, def).into_iter().filter(|s| s.role == Role::Value) {
                let Some(slot) = inner.inputs.iter().position(|i| i.widget.as_deref() == Some(s.name.as_str())) else { continue };
                if inner.inputs[slot].link.is_some() {
                    continue;
                }
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
                t.append(&gtk::Label::builder().label(&s.name).xalign(0.0).css_classes(["prop-name"]).build());
                t.append(&dim(&nice_name(inner, reg)));
                row.append(&t);
                let b = gtk::Button::builder()
                    .icon_name("cg-lucide-eye-symbolic")
                    .tooltip_text("Show Input")
                    .css_classes(["flat", "prop-eye"])
                    .halign(gtk::Align::End)
                    .hexpand(true)
                    .build();
                {
                    let (ws, doc, sg_id, inner_id, name, seed) =
                        (ws.clone(), doc.clone(), sg_id.clone(), inner.id.clone(), s.name.clone(), s.value.clone());
                    b.connect_clicked(move |_| {
                        ws.promote_widget(&doc, &sg_id, &inner_id, &name, seed.clone());
                        later(&ws);
                    });
                }
                row.append(&b);
                self.editor.append(&row);
                hidden += 1;
            }
        }
        if hidden == 0 {
            self.editor.append(&dim("Every parameter is on the node"));
        }
        self.editor.append(&dim("Promoted inputs live on the subgraph, so every instance shows them."));
    }
}

// ---------------------------------------------------------------------------------------
// Editors (GTK widgets for the canvas widget kinds).
// ---------------------------------------------------------------------------------------

fn editor(w: &Widget, commit: Rc<dyn Fn(Value)>) -> Option<gtk::Widget> {
    match &w.kind {
        WKind::Bool => {
            let s = gtk::Switch::builder().active(w.value.as_bool().unwrap_or(false)).halign(gtk::Align::Start).build();
            let c = commit.clone();
            s.connect_state_set(move |_, on| {
                c(Value::Bool(on));
                glib::Propagation::Proceed
            });
            Some(s.upcast())
        }
        WKind::Combo(opts) => {
            // The saved file may name a model the server no longer has: show it anyway
            // instead of silently displaying (and then writing) the first option.
            let cur = w.value.as_str().map(str::to_owned).unwrap_or_else(|| w.value.to_string());
            let mut items = opts.clone();
            if !items.iter().any(|o| *o == cur) && !cur.is_empty() {
                items.push(cur.clone());
            }
            if items.is_empty() {
                return Some(dim(&display(&w.value)).upcast());
            }
            let strings: Vec<&str> = items.iter().map(String::as_str).collect();
            let dd = gtk::DropDown::from_strings(&strings);
            if let Some(i) = items.iter().position(|o| *o == cur) {
                dd.set_selected(i as u32);
            }
            let (c, items) = (commit.clone(), items);
            dd.connect_selected_notify(move |d| {
                if let Some(o) = items.get(d.selected() as usize) {
                    c(Value::from(o.as_str()));
                }
            });
            Some(dd.upcast())
        }
        WKind::Int { min, max, step } => {
            let sp = gtk::SpinButton::with_range(*min, (*max).max(*min + 1000.0), (*step).max(1.0));
            sp.set_digits(0);
            sp.set_value(w.value.as_f64().unwrap_or(0.0));
            sp.set_hexpand(true);
            let c = commit.clone();
            sp.connect_value_changed(move |s| c(Value::from(s.value() as i64)));
            Some(sp.upcast())
        }
        WKind::Float { min, max, step, prec } => {
            let sp = gtk::SpinButton::with_range(*min, (*max).max(*min + 1.0), (*step).max(0.001));
            sp.set_digits(*prec as u32);
            sp.set_value(w.value.as_f64().unwrap_or(0.0));
            sp.set_hexpand(true);
            let c = commit.clone();
            sp.connect_value_changed(move |s| c(Value::from(s.value())));
            Some(sp.upcast())
        }
        WKind::Text { multiline: false } => {
            let e = gtk::Entry::builder().text(display(&w.value)).hexpand(true).build();
            let (c, e2) = (commit.clone(), e.clone());
            e.connect_activate(move |_| c(Value::from(e2.text().to_string())));
            let (c, e3) = (commit, e.clone());
            let f = gtk::EventControllerFocus::new();
            f.connect_leave(move |_| c(Value::from(e3.text().to_string())));
            e.add_controller(f);
            Some(e.upcast())
        }
        WKind::Text { multiline: true } => {
            let tv = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).top_margin(4).bottom_margin(4).left_margin(6).right_margin(6).build();
            tv.buffer().set_text(&display(&w.value));
            let sc = gtk::ScrolledWindow::builder().child(&tv).height_request(120).build();
            sc.add_css_class("prop-text");
            let (c, tv2) = (commit, tv.clone());
            let f = gtk::EventControllerFocus::new();
            f.connect_leave(move |_| {
                let b = tv2.buffer();
                c(Value::from(b.text(&b.start_iter(), &b.end_iter(), false).to_string()));
            });
            tv.add_controller(f);
            Some(sc.upcast())
        }
        // Composite rows (LoRA stacks, tag clouds, upload buttons) only have canvas editors.
        WKind::Button | WKind::Lora(_) | WKind::Cloud(_) => None,
        WKind::Seed | WKind::ReadOnly | WKind::Label => Some(dim(&display(&w.value)).upcast()),
    }
}

// ---------------------------------------------------------------------------------------
// Small pieces.
// ---------------------------------------------------------------------------------------

/// Value written by the row's editor and by Reset to Default.
fn commit(ws: &Workspace, doc: &DocRef, node: &workflow::Node, w: &Widget) -> Rc<dyn Fn(Value)> {
    let (ws, doc, id, key, structural) = (ws.clone(), doc.clone(), node.id.clone(), w.key.clone(), w.structural);
    Rc::new(move |v: Value| {
        ws.on_widget(&doc, &id, &key, v, structural);
        if !structural {
            // The canvas paints widget values from its own scene: re-render it.
            ws.render_doc(&doc);
            ws.set_dirty(&doc, true);
        }
    })
}

/// The value Reset to Default writes, when it differs from the current one.
/// Where a row keeps its user-facing label.
#[derive(Clone)]
struct SlotRef {
    /// Set when the slot lives on an interior node of this subgraph type.
    sg: Option<String>,
    node: String,
    slot: usize,
}

fn rename_target(ws: &Workspace, node: &workflow::Node, w: &Widget) -> Option<SlotRef> {
    if let Some(i) = w.input {
        return Some(SlotRef { sg: None, node: node.id.clone(), slot: i });
    }
    let (inner_id, _) = w.key.strip_prefix("p:")?.split_once(':')?;
    let sg = ws.active_doc()?.borrow().wf.subgraphs.get(&node.ty)?.clone();
    let inner = sg.graph.node(inner_id)?;
    let slot = inner.inputs.iter().position(|i| slot_for(i, w))?;
    Some(SlotRef { sg: Some(node.ty.clone()), node: inner_id.to_owned(), slot })
}

/// A freshly derived row for `key`, so menus decide on the current value/label.
fn live_widget(ws: &Workspace, doc: &DocRef, node: &workflow::Node, key: &str) -> Option<Widget> {
    let d = doc.borrow();
    let graph = graph_ref(&d);
    let n = graph.nodes.iter().find(|n| n.id == node.id).unwrap_or(node);
    let reg = ws.registry.borrow();
    let mut rows = crate::graph_view::build_widgets(&d.wf, graph, n, &reg, &mut Vec::new());
    if d.wf.subgraphs.contains_key(&n.ty) {
        rows.extend(crate::graph_view::subgraph_advanced_widgets(&d.wf, n, &reg));
    }
    rows.into_iter().find(|r| r.key == key)
}

fn menu_item(label: &str, sensitive: bool, f: Box<dyn Fn()>) -> gtk::Button {
    let b = gtk::Button::builder().label(label).halign(gtk::Align::Fill).css_classes(["flat"]).sensitive(sensitive).build();
    if sensitive {
        b.connect_clicked(move |_| f());
    }
    b
}

fn reset_value(w: &Widget) -> Option<Value> {
    if w.default.is_null() || w.default == w.value {
        None
    } else {
        Some(w.default.clone())
    }
}

/// Subgraph row → the input slot a rename can write (`label`).
fn slot_for(i: &workflow::Input, w: &Widget) -> bool {
    i.widget.as_deref() == Some(w.label.as_str()) || i.label.as_deref() == Some(w.label.as_str())
}

/// A promo table target for an ADVANCED INPUTS row: promote that interior widget.
fn promote_target(ws: &Workspace, node: &workflow::Node, w: &Widget) -> Option<(String, String)> {
    let sg = ws.active_doc()?.borrow().wf.subgraphs.get(&node.ty)?.clone();
    let (inner_id, _) = w.key.strip_prefix("p:")?.split_once(':')?;
    let inner = sg.graph.node(inner_id)?;
    if let Some(i) = inner.inputs.iter().find(|i| slot_for(i, w)) {
        // Already linked to the subgraph boundary: nothing left to show.
        if i.link.is_some() {
            return None;
        }
        return Some((inner_id.to_owned(), i.widget.clone().unwrap_or_else(|| w.label.clone())));
    }
    // Saved graphs omit widget sockets; promotion creates one, so the row is still showable.
    Some((inner_id.to_owned(), w.label.clone()))
}

/// The panel's own actions run while its focus guard is up: rebuild it once the click
/// that caused them has been handled.
fn later(ws: &Workspace) {
    let ws = ws.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || ws.panel.refresh(&ws));
}

fn set_mode(ws: &Workspace, doc: &DocRef, ids: &[String], mode: i64) {
    {
        let mut d = doc.borrow_mut();
        for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
            n.mode = mode;
        }
    }
    ws.edited(doc);
}

/// Colour grid: the 8 node colours plus "no colour", drawn in one area (click = pick).
fn swatches(ws: &Workspace, doc: &DocRef, ids: &[String], node: &workflow::Node) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(5 * (SW + GAP))
        .content_height(2 * (SH + GAP))
        .halign(gtk::Align::Start)
        .build();
    let current = node.color.as_deref().and_then(crate::canvas::parse_color);
    let draw_current = Cell::new(current);
    {
        let dc = draw_current.clone();
        area.set_draw_func(move |_, cr, _, _| {
            for i in 0..9 {
                let (col, row) = ((i % 5) as f64, (i / 5) as f64);
                let (x, y) = (col * (SW + GAP) as f64, row * (SH + GAP) as f64);
                let c = if i < NODE_COLORS.len() { NODE_COLORS[i].1 } else { 0 };
                if c == 0 {
                    cr.set_source_rgba(0.55, 0.55, 0.55, 0.25);
                } else {
                    let ch = |sh: u32| ((c >> sh) & 0xff) as f64 / 255.0;
                    cr.set_source_rgba(ch(16), ch(8), ch(0), 1.0);
                }
                round(cr, x, y, SW as f64, SH as f64, 9.0);
                cr.fill().ok();
                if c == 0 {
                    cr.set_source_rgba(0.8, 0.8, 0.8, 0.7);
                    cr.set_line_width(1.5);
                    cr.move_to(x + 10.0, y + SH as f64 - 5.0);
                    cr.line_to(x + SW as f64 - 10.0, y + 5.0);
                    cr.stroke().ok();
                }
                if dc.get() == Some(c) && c != 0 {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
                    cr.set_line_width(2.0);
                    round(cr, x - 2.0, y - 2.0, SW as f64 + 4.0, SH as f64 + 4.0, 11.0);
                    cr.stroke().ok();
                }
            }
        });
    }
    let click = gtk::GestureClick::new();
    {
        let (ws, doc, ids, dc) = (ws.clone(), doc.clone(), ids.to_vec(), draw_current);
        let area2 = area.clone();
        click.connect_pressed(move |_, _, x, y| {
            let (col, row) = (x as i32 / (SW + GAP), y as i32 / (SH + GAP));
            if !(0..5).contains(&col) || !(0..2).contains(&row) {
                return;
            }
            let idx = (row * 5 + col) as usize;
            if idx > NODE_COLORS.len() {
                return;
            }
            let c = NODE_COLORS.get(idx).map(|(_, rgb)| *rgb);
            dc.set(c);
            ws.set_node_color(&doc, &ids, c);
            area2.queue_draw();
        });
    }
    area.add_controller(click);
    area.set_cursor_from_name(Some("pointer"));
    area
}
