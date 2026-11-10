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

#[derive(Clone)]
pub(super) struct Panel {
    pub root: gtk::Box,
    title: gtk::Label,
    pencil: gtk::Button,
    /// Panel-header toggle for the subgraph input editor (the web's `settings-2` button).
    sub_btn: gtk::ToggleButton,
    /// Panel-header close button (the web's `panel-right`); wired by the workspace.
    pub close: gtk::Button,
    search: gtk::SearchEntry,
    stack: gtk::Stack,
    tab_info: gtk::ToggleButton,
    params: gtk::Box,
    info: gtk::Box,
    settings: gtk::Box,
    editor: gtk::Box,
    /// The pages' scrollers (params, info, settings, subgraph), kept across refreshes.
    scrolls: Vec<gtk::ScrolledWindow>,
    state: Rc<State>,
}

#[derive(Default)]
struct State {
    /// What `refresh` last rendered (see `key`); a tick re-renders when it changes.
    key: RefCell<String>,
    /// Which node that was (see `ident`).
    ident: RefCell<String>,
    /// Section label → expanded, for the node currently shown.
    open: RefCell<std::collections::HashMap<String, bool>>,
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
/// unless it changed). Covers what the canvas, menus and other panels can change behind
/// the panel's back: title, state, colour, pin, sockets/labels, values, interior values.
fn key(ws: &Workspace) -> String {
    use std::hash::{Hash, Hasher};
    let Some(doc) = ws.active_doc() else { return "none".into() };
    let d = doc.borrow();
    let sel = d.canvas.selected();
    let graph = graph_ref(&d);
    let node = sel.last().and_then(|id| graph.nodes.iter().find(|n| &n.id == id));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(n) = node {
        (&n.title, n.mode, n.pinned, &n.color, &n.bgcolor).hash(&mut h);
        n.properties.to_string().hash(&mut h);
        n.widgets_values.to_string().hash(&mut h);
        for i in &n.inputs {
            (&i.name, &i.label, &i.widget, i.link.is_some()).hash(&mut h);
        }
        if let Some(sg) = d.wf.subgraphs.get(&n.ty) {
            (&sg.name, sg.inputs.len()).hash(&mut h);
            for inner in &sg.graph.nodes {
                inner.widgets_values.to_string().hash(&mut h);
                for i in &inner.inputs {
                    (&i.label, &i.widget, i.link.is_some()).hash(&mut h);
                }
            }
        }
    }
    format!(
        "{}|{:x}|{}",
        ident(&doc, &d, &sel),
        h.finish(),
        ws.registry.borrow().nodes.len(),
    )
}

/// Which node the panel shows (doc, graph level, selection), regardless of its values:
/// a refresh of the same node keeps scroll positions and folded sections.
fn ident(doc: &DocRef, d: &Doc, sel: &[String]) -> String {
    format!("{:p}|{}|{:?}", Rc::as_ptr(doc) as *const u8, d.path.join("/"), sel)
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

/// Small label above a settings control ("Node state", "Node color"…).
fn field(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["dim-label", "caption"]).margin_top(6).build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["prop-heading"]).margin_top(4).build()
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
        .tooltip_text("Edit Subgraph")
        .css_classes(["flat"])
        .visible(false)
        .build();
    let close = gtk::Button::builder()
        .icon_name("cg-lucide-panel-right-symbolic")
        .tooltip_text("Toggle Properties Panel")
        .css_classes(["flat"])
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
    head.append(&close);
    // The panel sits at the window's right edge, where the main header bar hides its
    // window buttons (libadwaita does that for split views): show them here instead.
    head.append(&gtk::WindowControls::new(gtk::PackType::End));

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
        gtk::ScrolledWindow::builder().child(b).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build()
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
    let scrolls = vec![params_sc.clone(), page(&info), page(&settings), page(&editor)];
    stack.add_named(&scrolls[1], Some("info"));
    stack.add_named(&scrolls[2], Some("settings"));
    stack.add_named(&scrolls[3], Some("subgraph"));
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
    root.append(&gtk::WindowHandle::builder().child(&head).build());
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

    Panel { root, title, pencil, sub_btn, close, search, stack, tab_info, params, info, settings, editor, scrolls, state }
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
        // Same node re-rendered (value edited, input shown/hidden…): keep where the user
        // was. Another node: start from the top with the default folds.
        let ident = ws.active_doc().map(|doc| {
            let d = doc.borrow();
            ident(&doc, &d, &d.canvas.selected())
        });
        let ident = ident.unwrap_or_default();
        let same = *self.state.ident.borrow() == ident;
        if !same {
            self.state.open.borrow_mut().clear();
        }
        *self.state.ident.borrow_mut() = ident;
        let scroll: Vec<f64> = self.scrolls.iter().map(|s| s.vadjustment().value()).collect();
        self.fill(ws);
        if same {
            for (sc, v) in self.scrolls.iter().zip(scroll) {
                let adj = sc.vadjustment();
                // After the new rows are laid out (the upper bound is stale until then).
                glib::idle_add_local_once(move || adj.set_value(v.min(adj.upper() - adj.page_size())));
            }
        }
    }

    /// The Info tab exists only for a single plain node (the web hides it for subgraph
    /// instances and for the workflow overview); leaving it falls back to Parameters.
    fn info_tab(&self, on: bool) {
        self.tab_info.set_visible(on);
        if !on {
            if self.stack.visible_child_name().as_deref() == Some("info") {
                self.stack.set_visible_child_name("params");
            }
            if *self.state.tab.borrow() == "info" {
                *self.state.tab.borrow_mut() = "params".into();
            }
        }
    }

    fn fill(&self, ws: &Workspace) {
        for b in [&self.params, &self.info, &self.settings, &self.editor] {
            clear(b);
        }
        self.state.rows.borrow_mut().clear();
        *self.state.key.borrow_mut() = key(ws);
        if let Some(h) = self.state.pencil.borrow_mut().take() {
            self.pencil.disconnect(h);
        }
        self.pencil.set_sensitive(false);
        let q = self.search.text().to_lowercase();

        let Some(doc) = ws.active_doc() else {
            self.close_editor();
            self.title.set_label("Properties");
            self.params.append(&dim("No workflow open"));
            self.info_tab(false);
            return;
        };
        let reg = ws.registry.borrow();
        let d = doc.borrow();
        let graph = graph_ref(&d);
        let sel = d.canvas.selected();
        let node = sel.last().and_then(|id| graph.nodes.iter().find(|n| &n.id == id)).cloned();
        let Some(node) = node else {
            self.close_editor();
            self.title.set_label("Properties");
            self.params.append(&dim("Select a node to see its parameters"));
            self.info_tab(false);
            return;
        };
        let sg = d.wf.subgraphs.get(&node.ty).cloned();
        self.info_tab(sg.is_none());
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
        // The editor stays open while the selection is a subgraph instance (the web keeps
        // its "subgraph" tab while `isSingleSubgraphNode`).
        self.sub_btn.set_visible(sg.is_some());
        if sg.is_some() {
            let shown: Vec<Widget> = promoted.into_iter().filter(|w| w.key.starts_with("w:") || w.key.starts_with("p:")).collect();
            self.fill_editor(ws, &doc, &node, &shown, &interior);
        } else {
            self.close_editor();
        }
    }

    /// Open the subgraph editor ("Edit Subgraph Widgets"); a no-op unless a subgraph is shown.
    pub(super) fn open_editor(&self) {
        if self.sub_btn.is_visible() {
            self.sub_btn.set_active(true);
        }
    }

    fn close_editor(&self) {
        self.sub_btn.set_visible(false);
        if self.sub_btn.is_active() {
            self.sub_btn.set_active(false);
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
        let expanded = self.state.open.borrow().get(label).copied().unwrap_or(expanded);
        let exp = gtk::Expander::builder().expanded(expanded).css_classes(["prop-section"]).build();
        {
            let (st, label) = (self.state.clone(), label.to_owned());
            exp.connect_expanded_notify(move |e| {
                st.open.borrow_mut().insert(label.clone(), e.is_expanded());
            });
        }
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

        // Source node, as the web's right-aligned caption (subgraph rows only; a plain
        // node's rows have none).
        let caption = w.source.clone();

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
        // Per-row menu: Rename / Show input / Reset to default (screen: "Three Dots").
        // No eye here: like the web, hiding a promoted input is the subgraph editor's job.
        if menu_worthy(w) {
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
                if let Some(t) = rename_target(&ws2, &node2, &w) {
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
                                rename(&me, &doc, &t, text);
                                later(&me);
                            });
                        }),
                    ));
                }
                if let Some((inner, name)) = promote_target(&ws2, &node2, &w) {
                    let (ws, doc, sg_id, seed, host) = (ws2.clone(), doc2.clone(), node2.ty.clone(), w.value.clone(), node2.id.clone());
                    let p = pop.clone();
                    list.append(&menu_item(
                        "Show input",
                        true,
                        Box::new(move || {
                            p.popdown();
                            ws.promote_widget(&doc, &host, &sg_id, &inner, &name, seed.clone());
                            later(&ws);
                        }),
                    ));
                }
                // Like the web: shown when the widget has a default, greyed out at it.
                if !w.default.is_null() {
                    let c = commit(&ws2, &doc2, &node2, &w);
                    let (ws3, p, v) = (ws2.clone(), pop.clone(), w.default.clone());
                    list.append(&menu_item(
                        "Reset to default",
                        w.value != w.default,
                        Box::new(move || {
                            p.popdown();
                            c(v.clone());
                            later(&ws3);
                        }),
                    ));
                }
                if list.first_child().is_none() {
                    list.append(&menu_item("No actions", false, Box::new(|| {})));
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

    /// Node documentation as the web's Info tab falls back to it without docs: name,
    /// description, then Inputs / Outputs tables from the node definition.
    fn fill_info(&self, node: &workflow::Node, reg: &crate::registry::Registry, sg: Option<&Subgraph>) {
        if sg.is_some() {
            return; // tab hidden for subgraph instances
        }
        let Some(def) = reg.get(&node.ty) else {
            self.info.append(&gtk::Label::builder().label(&node.ty).xalign(0.0).wrap(true).css_classes(["title-4"]).build());
            self.info.append(&dim("Unknown node type: the server does not provide it"));
            return;
        };
        let name = if def.display_name.is_empty() { &node.ty } else { &def.display_name };
        self.info.append(&gtk::Label::builder().label(name).xalign(0.0).wrap(true).css_classes(["title-3"]).build());
        if !def.description.is_empty() {
            self.info.append(&gtk::Label::builder().label(&def.description).xalign(0.0).wrap(true).build());
        }
        let cell = |text: &str, classes: &[&str]| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .yalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .max_width_chars(28)
                .css_classes(classes.to_vec())
                .build()
        };
        let table = |label: &str, head: &[&str], rows: Vec<Vec<String>>| {
            if rows.is_empty() {
                return;
            }
            let h = gtk::Label::builder().label(label).xalign(0.0).css_classes(["heading"]).margin_top(12).build();
            self.info.append(&h);
            let grid = gtk::Grid::builder().row_spacing(6).column_spacing(12).build();
            for (c, t) in head.iter().enumerate() {
                grid.attach(&cell(t, &["caption-heading"]), c as i32, 0, 1, 1);
            }
            for (r, row) in rows.iter().enumerate() {
                for (c, t) in row.iter().enumerate() {
                    let classes: &[&str] = match c {
                        0 => &["monospace", "accent"],
                        _ => &["caption"],
                    };
                    grid.attach(&cell(t, classes), c as i32, r as i32 + 1, 1, 1);
                }
            }
            self.info.append(&grid);
        };
        // JSON's own number text: an INT max of 2^64-1 must not print as a float.
        let range = |o: &Value| match (&o["min"], &o["max"]) {
            (a @ Value::Number(_), b @ Value::Number(_)) => format!("{a} – {b}"),
            _ => "N/A".into(),
        };
        table(
            "Inputs",
            &["Parameter", "Description", "Data Type", "Required", "Range"],
            def.inputs
                .iter()
                .map(|i| {
                    vec![
                        i.name.clone(),
                        i.opts["tooltip"].as_str().unwrap_or("").to_owned(),
                        i.type_name().to_owned(),
                        if i.optional { "No" } else { "Yes" }.into(),
                        range(&i.opts),
                    ]
                })
                .collect(),
        );
        table("Outputs", &["Output Name", "Data Type"], def.outputs.iter().map(|(t, n)| vec![n.clone(), t.clone()]).collect());
    }

    fn fill_settings(&self, ws: &Workspace, doc: &DocRef, node: &workflow::Node, sel: &[String]) {
        let ids: Vec<String> = sel.iter().filter(|id| id.as_str() != "-10" && id.as_str() != "-20").cloned().collect();
        let ids = if ids.is_empty() { vec![node.id.clone()] } else { ids };

        self.settings.append(&field("Node state"));
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
            b.connect_toggled(move |b| {
                if b.is_active() {
                    set_mode(&ws, &doc, &ids, mode);
                }
            });
            seg.append(&b);
        }
        self.settings.append(&seg);

        self.settings.append(&field("Node color"));
        self.settings.append(&swatches(ws, doc, &ids, node));

        let pin_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let pin_label = field("Pinned");
        pin_label.set_hexpand(true);
        pin_row.append(&pin_label);
        let pin = gtk::Switch::builder()
            .active(node.pinned)
            .valign(gtk::Align::Center)
            .tooltip_text("Pinned nodes cannot be dragged or resized")
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
        pin_row.append(&pin);
        self.settings.append(&pin_row);
    }

    /// SHOWN / HIDDEN inputs of the selected subgraph (the panel-header toggle): the same
    /// rows as Parameters' INPUTS (promoted, modern or legacy proxy) and ADVANCED INPUTS.
    fn fill_editor(&self, ws: &Workspace, doc: &DocRef, node: &workflow::Node, shown: &[Widget], hidden: &[Widget]) {
        let item = |w: &Widget, icon: &str, tip: &str, act: Box<dyn Fn()>| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
            t.set_hexpand(true);
            if !w.source.is_empty() {
                t.append(&dim(&w.source));
            }
            t.append(&gtk::Label::builder().label(&w.label).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["prop-name"]).build());
            row.append(&t);
            let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).css_classes(["flat", "prop-eye"]).valign(gtk::Align::Center).build();
            b.connect_clicked(move |_| act());
            row.append(&b);
            row
        };
        let host = node.id.clone();

        let hide_all = gtk::Button::builder().label("Hide all").css_classes(["flat", "prop-eye"]).sensitive(!shown.is_empty()).build();
        {
            let (ws, doc, host) = (ws.clone(), doc.clone(), host.clone());
            hide_all.connect_clicked(move |_| {
                ws.hide_all_inputs(&doc, &host);
                later(&ws);
            });
        }
        self.editor.append(&heading_row("SHOWN ON NODE", &hide_all));
        for w in shown {
            let (ws, doc, host, sg_ty) = (ws.clone(), doc.clone(), host.clone(), node.ty.clone());
            let act: Box<dyn Fn()> = match w.input.and_then(|i| node.inputs.get(i)) {
                // Modern: a subgraph input linked to the widget.
                Some(inp) if w.key.starts_with("w:") => {
                    let name = inp.name.clone();
                    Box::new(move || {
                        ws.demote_input(&doc, &sg_ty, &name);
                        later(&ws);
                    })
                }
                // Legacy: an entry of the instance's `properties.proxyWidgets`.
                _ => {
                    let key = w.key.clone();
                    Box::new(move || {
                        let target = {
                            let d = doc.borrow();
                            graph_view::interior_slot(&d.wf, &sg_ty, &key, &ws.registry.borrow())
                        };
                        if let Some((inner, name)) = target {
                            ws.demote_proxy(&doc, &host, &inner, &name);
                            later(&ws);
                        }
                    })
                }
            };
            self.editor.append(&item(w, "cg-lucide-eye-symbolic", "Hide input", act));
        }
        if shown.is_empty() {
            self.editor.append(&dim("No inputs shown on the node"));
        }

        let show_all = gtk::Button::builder().label("Show all").css_classes(["flat", "prop-eye"]).sensitive(!hidden.is_empty()).build();
        {
            let (ws, doc, host) = (ws.clone(), doc.clone(), host.clone());
            show_all.connect_clicked(move |_| {
                ws.show_all_inputs(&doc, &host);
                later(&ws);
            });
        }
        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_margin_top(6);
        self.editor.append(&sep);
        self.editor.append(&heading_row("HIDDEN / NESTED PARAMETERS", &show_all));
        for w in hidden {
            let Some((inner, name)) = promote_target(ws, node, w) else { continue };
            let (ws, doc, sg_ty, seed, host) = (ws.clone(), doc.clone(), node.ty.clone(), w.value.clone(), host.clone());
            let act: Box<dyn Fn()> = Box::new(move || {
                ws.promote_widget(&doc, &host, &sg_ty, &inner, &name, seed.clone());
                later(&ws);
            });
            self.editor.append(&item(w, "cg-lucide-eye-off-symbolic", "Show input", act));
        }
        if hidden.is_empty() {
            self.editor.append(&dim("Every parameter is shown on the node"));
        }
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

/// Value written by the row's editor and by Reset to Default. Unchanged values are not
/// written (a text field commits on every focus-out; that must not dirty the tab).
fn commit(ws: &Workspace, doc: &DocRef, node: &workflow::Node, w: &Widget) -> Rc<dyn Fn(Value)> {
    let (ws, doc, id, key, structural) = (ws.clone(), doc.clone(), node.id.clone(), w.key.clone(), w.structural);
    let last = RefCell::new(w.value.clone());
    Rc::new(move |v: Value| {
        if *last.borrow() == v {
            return;
        }
        *last.borrow_mut() = v.clone();
        ws.on_widget(&doc, &id, &key, v, structural);
        if !structural {
            // The canvas paints widget values from its own scene: re-render it.
            ws.render_doc(&doc);
            ws.set_dirty(&doc, true);
        }
    })
}

/// Where a row keeps its user-facing label: an input slot (`label`), created on demand
/// for widgets the saved file gave no socket (the web gives every widget one).
#[derive(Clone)]
struct SlotRef {
    /// Set when the slot lives on an interior node of this subgraph type.
    sg: Option<String>,
    node: String,
    /// Existing input slot, else the widget whose socket the rename creates.
    slot: Option<usize>,
    widget: String,
}

fn rename_target(ws: &Workspace, node: &workflow::Node, w: &Widget) -> Option<SlotRef> {
    if let Some(i) = w.input {
        return Some(SlotRef { sg: None, node: node.id.clone(), slot: Some(i), widget: String::new() });
    }
    let doc = ws.active_doc()?;
    let d = doc.borrow();
    let reg = ws.registry.borrow();
    if w.key.starts_with("p:") {
        let (inner, widget) = graph_view::interior_slot(&d.wf, &node.ty, &w.key, &reg)?;
        return Some(SlotRef { sg: Some(node.ty.clone()), node: inner, slot: None, widget });
    }
    // A plain node's own widget (`<idx>` key): value widgets only, not control_after_generate.
    let idx: usize = w.key.parse().ok()?;
    let s = prompt::widget_slots(node, reg.get(&node.ty)?).into_iter().find(|s| s.idx == idx && s.role == Role::Value)?;
    Some(SlotRef { sg: None, node: node.id.clone(), slot: None, widget: s.name })
}

fn rename(ws: &Workspace, doc: &DocRef, t: &SlotRef, text: String) {
    let label = Some(text).filter(|x| !x.trim().is_empty());
    {
        let reg = ws.registry.borrow();
        let mut d = doc.borrow_mut();
        let graph = match &t.sg {
            Some(ty) => d.wf.subgraphs.get_mut(ty).map(|s| &mut s.graph),
            None => Some(graph_mut(&mut d)),
        };
        let Some(n) = graph.and_then(|g| g.nodes.iter_mut().find(|n| n.id == t.node)) else { return };
        let at = t.slot.or_else(|| n.inputs.iter().position(|i| i.widget.as_deref() == Some(t.widget.as_str())));
        match at {
            Some(i) => {
                if let Some(i) = n.inputs.get_mut(i) {
                    i.label = label;
                }
            }
            None if label.is_some() => {
                let ty = reg
                    .get(&n.ty)
                    .and_then(|def| def.inputs.iter().find(|i| i.name == t.widget))
                    .map(|i| i.type_name().to_owned())
                    .unwrap_or_default();
                n.inputs.push(workflow::Input {
                    name: t.widget.clone(),
                    ty,
                    link: None,
                    widget: Some(t.widget.clone()),
                    label,
                });
            }
            None => return,
        }
    }
    ws.edited(doc);
}

/// Rows that get the ⋮ menu (display-only rows have nothing to act on).
fn menu_worthy(w: &Widget) -> bool {
    !matches!(w.kind, WKind::ReadOnly | WKind::Label)
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

/// LiteGraph's `node_colors` (name, title `color`, body `bgcolor`), as 1.60's Settings tab
/// offers them; written verbatim so the web frontend recognises the choice again.
const LG_COLORS: [(&str, &str, &str); 9] = [
    ("Red", "#322", "#533"),
    ("Brown", "#332922", "#593930"),
    ("Green", "#232", "#353"),
    ("Blue", "#223", "#335"),
    ("Pale blue", "#2a363b", "#3f5159"),
    ("Cyan", "#233", "#355"),
    ("Purple", "#323", "#535"),
    ("Yellow", "#432", "#653"),
    ("Black", "#222", "#000"),
];

/// `adjustColor(c, { lightness })`: HSL lightness + `dl`, as the web draws the dots.
fn lighten(c: u32, dl: f64) -> (f64, f64, f64) {
    let ch = |sh: u32| ((c >> sh) & 0xff) as f64 / 255.0;
    let (r, g, b) = (ch(16), ch(8), ch(0));
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
        let h = if mx == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s)
    };
    let l = (l + dl).clamp(0.0, 1.0);
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

/// Colour grid: "no colour" + LiteGraph's nine, 5×2 round dots (click = pick). The
/// current one sits on a muted tile, as on the web.
fn swatches(ws: &Workspace, doc: &DocRef, ids: &[String], node: &workflow::Node) -> gtk::DrawingArea {
    const CELL: f64 = 32.0;
    let area = gtk::DrawingArea::builder().content_height(2 * CELL as i32 + 8).hexpand(true).build();
    area.add_css_class("prop-swatches");
    // 0 = no colour; 1..=9 = LG_COLORS; None = a colour this palette does not have.
    let current = match (&node.color, &node.bgcolor) {
        (None, None) => Some(0),
        (c, bg) => LG_COLORS
            .iter()
            .position(|(_, lc, lbg)| c.as_deref() == Some(*lc) && bg.as_deref() == Some(*lbg))
            .map(|i| i + 1),
    };
    let fg = area.color();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // Panel tile behind the grid (bg-secondary).
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.06);
        round(cr, 0.0, 0.0, w, h, 8.0);
        cr.fill().ok();
        let cw = w / 5.0;
        for i in 0..10usize {
            let (cx, cy) = ((i % 5) as f64 * cw + cw / 2.0, 4.0 + (i / 5) as f64 * CELL + CELL / 2.0);
            let bg = if i == 0 { 0x353535 } else { crate::canvas::parse_color(LG_COLORS[i - 1].2).unwrap_or(0) };
            if current == Some(i) {
                cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.12);
                round(cr, cx - CELL / 2.0, cy - CELL / 2.0, CELL, CELL, 7.0);
                cr.fill().ok();
            }
            let (r, g, b) = lighten(bg, 0.3);
            cr.set_source_rgb(r, g, b);
            cr.arc(cx, cy, 8.0, 0.0, std::f64::consts::TAU);
            cr.fill_preserve().ok();
            let ring = if current == Some(i) { lighten(bg, 0.5) } else { (0.5, 0.5, 0.5) };
            cr.set_source_rgba(ring.0, ring.1, ring.2, if current == Some(i) { 1.0 } else { 0.1 });
            cr.set_line_width(2.0);
            cr.stroke().ok();
        }
    });
    let click = gtk::GestureClick::new();
    {
        let (ws, doc, ids) = (ws.clone(), doc.clone(), ids.to_vec());
        let area2 = area.clone();
        click.connect_pressed(move |_, _, x, y| {
            let col = (x / (area2.width() as f64 / 5.0)) as i32;
            let row = ((y - 4.0) / CELL) as i32;
            if !(0..5).contains(&col) || !(0..2).contains(&row) || y < 4.0 {
                return;
            }
            let i = (row * 5 + col) as usize;
            let pick = (i > 0).then(|| LG_COLORS[i - 1]);
            {
                let mut d = doc.borrow_mut();
                for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                    n.color = pick.map(|(_, c, _)| c.to_owned());
                    n.bgcolor = pick.map(|(_, _, bg)| bg.to_owned());
                }
            }
            ws.edited(&doc);
        });
    }
    area.add_controller(click);
    area.set_cursor_from_name(Some("pointer"));
    {
        // Name of the dot under the pointer, like the web's tooltips.
        let area2 = area.clone();
        area.set_has_tooltip(true);
        area.connect_query_tooltip(move |_, x, y, _, tip| {
            let col = (x as f64 / (area2.width() as f64 / 5.0)) as usize;
            let row = ((y as f64 - 4.0) / CELL).max(0.0) as usize;
            let i = row * 5 + col;
            if i >= 10 {
                return false;
            }
            tip.set_text(Some(if i == 0 { "No color" } else { LG_COLORS[i - 1].0 }));
            true
        });
    }
    area
}
