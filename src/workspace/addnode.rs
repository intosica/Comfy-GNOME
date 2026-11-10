// SPDX-License-Identifier: GPL-3.0-only
//! "Add Node" search dialog = the frontend's NodeSearchBox v2 (`reference/Node Browser.png`).
//! Opened by a double-click on empty canvas, the context menu, or by releasing a dragged link.
//!
//! This file is only widgets. Every decision (what is listed, in which order, which chips and
//! categories exist) comes from `crate::nodesearch`, a function-by-function port of
//! `src/components/searchbox/v2/*` and `src/services/nodeSearchService.ts` — when the frontend
//! changes, re-port there and this file rarely needs to follow. Frontend counterparts:
//!   input row        NodeSearchInput.vue          filter bar   NodeSearchFilterBar.vue
//!   type popover     NodeSearchTypeFilterPopover  sidebar      NodeSearchCategorySidebar.vue
//!   result row       NodeSearchListItem.vue       link release NodeSearchBoxPopover.vue
//!
//! ponytail: no virtual list (results are capped at `MAX_ROWS`); chevrons are always shown
//! instead of on sidebar hover; no favourites chip (native node bookmarks don't exist).

use super::*;
use crate::nodesearch::content::{available_roots, is_expanded, toggle_expanded, CatNode, Content, DEFAULT_CATEGORY};
use crate::nodesearch::source::{node_source, SourceType};
use crate::nodesearch::{filter_options, visible, Filter, FilterKind};

/// (from node id, slot, dragged from an output, slot type)
type Link = (String, usize, bool, String);

/// Rows built for a category/“most relevant” listing (search is already capped at 64).
const MAX_ROWS: usize = 300;
/// `MAX_VISIBLE_DOTS` of NodeSearchFilterBar.vue.
const MAX_DOTS: usize = 4;
const ROW_H: i32 = 56;

struct St {
    c: Content,
    expanded: String,
    selected: usize,
}

struct Ui {
    ws: Workspace,
    doc: DocRef,
    pos: (f64, f64),
    link: Option<Link>,
    dlg: Rc<dyn Fn()>,
    st: RefCell<St>,
    entry: gtk::Text,
    applied: gtk::Box,
    roots: gtk::Box,
    tree: gtk::Box,
    preset: gtk::Button,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    shown: RefCell<Vec<String>>,
    type_updates: RefCell<Vec<Box<dyn Fn()>>>,
}

fn icon(name: &str) -> gtk::Image {
    let i = gtk::Image::from_icon_name(&format!("cg-lucide-{name}-symbolic"));
    i.set_pixel_size(14);
    i
}

/// Link-type colour dot (the frontend colours dots with `getLinkTypeColor`).
fn dot(ty: &str) -> gtk::DrawingArea {
    let c = canvas::type_color(ty);
    let d = gtk::DrawingArea::builder().content_width(8).content_height(8).valign(gtk::Align::Center).build();
    d.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgb(((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0);
        cr.arc(w as f64 / 2.0, h as f64 / 2.0, 3.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    d
}

fn clear(b: &impl IsA<gtk::Widget>) {
    while let Some(c) = b.first_child() {
        c.unparent();
    }
}

fn chip(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("ns-chip");
    b.set_focus_on_click(false);
    b
}

/// `highlightQuery`: bold the query words in a name.
fn highlight(name: &str, query: &str) -> String {
    let esc = |s: &str| gtk::glib::markup_escape_text(s).to_string();
    let lower = name.to_lowercase();
    let mut marks: Vec<(usize, usize)> = vec![];
    if lower.len() == name.len() {
        for w in query.to_lowercase().split_whitespace() {
            if let Some(i) = lower.find(w) {
                marks.push((i, i + w.len()));
            }
        }
    }
    marks.sort();
    let (mut out, mut at) = (String::new(), 0);
    for (s, e) in marks {
        if s < at {
            continue;
        }
        out += &esc(&name[at..s]);
        out += &format!("<b>{}</b>", esc(&name[s..e]));
        at = e;
    }
    out + &esc(&name[at..])
}

impl Ui {
    fn refresh(self: &Rc<Self>) {
        self.refresh_bar();
        self.refresh_tree();
        self.refresh_results();
    }

    /// Filter bar + the applied-filter chips of the input row.
    fn refresh_bar(self: &Rc<Self>) {
        let me = self;
        clear(&self.roots);
        let (active, filters) = {
            let s = self.st.borrow();
            (s.c.root, s.c.filters.clone())
        };
        for r in available_roots(&self.ws.registry.borrow()) {
            let b = chip(r.label());
            if active == Some(r) {
                b.add_css_class("active");
            }
            let me = me.clone();
            b.connect_clicked(move |_| {
                {
                    let mut s = me.st.borrow_mut();
                    s.c.toggle_root(r);
                    s.expanded.clear();
                    s.selected = 0;
                }
                me.refresh();
            });
            self.roots.append(&b);
        }
        clear(&self.applied);
        for f in filters {
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            bx.append(&gtk::Label::new(Some(&format!("{}:", f.kind.label()))));
            bx.append(&dot(&f.value));
            bx.append(&gtk::Label::new(Some(&f.value)));
            bx.append(&icon("x"));
            let b = gtk::Button::builder().child(&bx).build();
            b.add_css_class("ns-chip");
            b.add_css_class("ns-applied");
            b.set_focus_on_click(false);
            let me = me.clone();
            b.connect_clicked(move |_| {
                me.st.borrow_mut().c.filters.retain(|x| *x != f);
                me.st.borrow_mut().selected = 0;
                me.refresh();
                me.entry.grab_focus();
            });
            self.applied.append(&b);
        }
        for u in self.type_updates.borrow().iter() {
            u();
        }
    }

    /// Category sidebar: "Most relevant" preset + tree.
    fn refresh_tree(self: &Rc<Self>) {
        clear(&self.tree);
        let (tree, nested, root) = {
            let mut s = self.st.borrow_mut();
            let reg = self.ws.registry.borrow();
            let tree = s.c.tree(&reg);
            // Exactly one root with children opens by itself.
            if s.expanded.is_empty() && tree.len() == 1 && !tree[0].children.is_empty() {
                s.expanded = tree[0].key.clone();
            }
            (tree, s.c.any_nested(&reg), s.c.root)
        };
        self.preset.set_visible(root.is_none());
        if root.is_none() {
            self.preset.remove_css_class("sel");
            if self.st.borrow().c.category == DEFAULT_CATEGORY {
                self.preset.add_css_class("sel");
            }
        }
        for n in &tree {
            self.add_cat(n, 0, nested);
        }
    }

    fn add_cat(self: &Rc<Self>, n: &CatNode, depth: i32, nested: bool) {
        let (selected, expanded) = {
            let s = self.st.borrow();
            (s.c.category == n.key, is_expanded(&s.expanded, &n.key))
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.set_margin_start(depth * 14);
        let lb = gtk::Label::builder().label(&n.label).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        row.append(&lb);
        if nested && !n.children.is_empty() {
            let ch = icon(if expanded { "chevron-down" } else { "chevron-right" });
            ch.add_css_class("dim-label");
            row.append(&ch);
        }
        let b = gtk::Button::builder().child(&row).build();
        b.add_css_class("ns-cat");
        b.set_focus_on_click(false);
        if selected {
            b.add_css_class("sel");
        } else if expanded && !n.children.is_empty() {
            b.add_css_class("exp");
        }
        let (me, key) = (self.clone(), n.key.clone());
        b.connect_clicked(move |_| {
            {
                let mut s = me.st.borrow_mut();
                s.expanded = toggle_expanded(&s.expanded, &key);
                s.c.category = key.clone();
                s.selected = 0;
            }
            me.refresh_tree();
            me.refresh_results();
        });
        self.tree.append(&b);
        if expanded {
            for c in &n.children {
                self.add_cat(c, depth + 1, nested);
            }
        }
    }

    /// Result list (`displayedResults`).
    fn refresh_results(self: &Rc<Self>) {
        self.list.remove_all();
        let query = self.st.borrow().c.query.clone();
        let mut names = vec![];
        {
            let s = self.st.borrow();
            let reg = self.ws.registry.borrow();
            for d in s.c.displayed(&reg).into_iter().take(MAX_ROWS) {
                names.push(d.name.clone());
                self.list.append(&result_row(d, &query));
            }
        }
        if names.is_empty() {
            let l = gtk::Label::builder().label("No Results").margin_top(60).build();
            l.add_css_class("dim-label");
            let r = gtk::ListBoxRow::builder().child(&l).selectable(false).activatable(false).build();
            self.list.append(&r);
        }
        *self.shown.borrow_mut() = names;
        let sel = self.st.borrow().selected;
        self.select(sel);
    }

    fn select(&self, i: usize) {
        let n = self.shown.borrow().len();
        if n == 0 {
            return;
        }
        let i = i.min(n - 1);
        self.st.borrow_mut().selected = i;
        if let Some(r) = self.list.row_at_index(i as i32) {
            self.list.select_row(Some(&r));
            // Keep the row inside the viewport (rows have a fixed height).
            let adj = self.scroll.vadjustment();
            let (top, bot) = (i as f64 * ROW_H as f64, (i + 1) as f64 * ROW_H as f64);
            if top < adj.value() {
                adj.set_value(top);
            } else if bot > adj.value() + adj.page_size() {
                adj.set_value(bot - adj.page_size());
            }
        }
    }

    /// `addNode`: create at the drop position, then wire the dragged link to it.
    fn add(&self, i: usize) {
        let Some(name) = self.shown.borrow().get(i).cloned() else { return };
        if let Some(new_id) = self.ws.create_node(&self.doc, &name, self.pos) {
            if let Some(req) = &self.link {
                self.ws.connect_new(&self.doc, &new_id, req);
            }
        }
        (self.dlg)();
    }
}

fn result_row(d: &registry::NodeDef, query: &str) -> gtk::ListBoxRow {
    let name = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
    name.set_markup(&highlight(&d.display_name, query));
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&name);
    // NodeSearchListItem.vue: "Comfy" for core nodes, the extension's short name otherwise,
    // coins for partner nodes.
    let src = node_source(&d.python_module);
    let pill_text = match src.kind {
        SourceType::Core => Some("Comfy".to_owned()),
        SourceType::Unknown => None,
        _ => Some(src.display_text),
    };
    if d.api_node {
        let c = icon("coins");
        c.add_css_class("dim-label");
        top.append(&c);
    }
    if let Some(t) = pill_text {
        let p = gtk::Label::new(Some(&t));
        p.add_css_class("ns-pill");
        top.append(&p);
    }
    let cat = gtk::Label::builder().label(d.category.replace('/', " / ")).xalign(0.0).build();
    cat.add_css_class("dim-label");
    cat.add_css_class("caption");
    // Descriptions can hold newlines (CLIP recipes…); the row shows only the first line's worth.
    let desc = gtk::Label::builder()
        .label(d.description.split_whitespace().collect::<Vec<_>>().join(" "))
        .xalign(0.0)
        .hexpand(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.append(&cat);
    if !d.description.is_empty() {
        bottom.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bottom.append(&desc);
    }
    let col = gtk::Box::new(gtk::Orientation::Vertical, 3);
    col.set_margin_start(12);
    col.set_margin_end(12);
    col.set_valign(gtk::Align::Center);
    col.append(&top);
    col.append(&bottom);
    let row = gtk::ListBoxRow::builder().child(&col).height_request(ROW_H).build();
    row.add_css_class("ns-row");
    row
}

/// One "Input ⌄" / "Output ⌄" chip with its type popover (NodeSearchTypeFilterPopover.vue).
fn type_filter(ui: &Rc<Ui>, kind: FilterKind) -> gtk::MenuButton {
    let options: Vec<String> = {
        let reg = ui.ws.registry.borrow();
        let mut o = filter_options(kind, &visible(&reg));
        o.sort();
        o
    };
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let label = gtk::Label::new(Some(kind.label()));
    let dots = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    content.append(&label);
    content.append(&dots);
    content.append(&icon("chevron-down"));
    let btn = gtk::MenuButton::builder().child(&content).always_show_arrow(false).build();
    btn.add_css_class("ns-chip");
    btn.set_focus_on_click(false);

    let pop = gtk::Popover::new();
    pop.set_has_arrow(false);
    pop.add_css_class("ns-pop");
    let search = gtk::SearchEntry::builder().placeholder_text("Search").build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let count = gtk::Label::builder().xalign(0.0).hexpand(true).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    let clear_all = gtk::Button::with_label("Clear all");
    clear_all.add_css_class("flat");
    clear_all.add_css_class("caption");
    head.append(&count);
    head.append(&clear_all);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let sc = gtk::ScrolledWindow::builder().child(&rows).min_content_height(220).max_content_height(260).propagate_natural_height(true).build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.set_width_request(256);
    body.append(&search);
    body.append(&head);
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    body.append(&sc);
    pop.set_child(Some(&body));
    btn.set_popover(Some(&pop));

    let selected = {
        let ui = ui.clone();
        move || -> Vec<String> { ui.st.borrow().c.filters.iter().filter(|f| f.kind == kind).map(|f| f.value.clone()).collect() }
    };
    let fill: Rc<dyn Fn()> = {
        let (ui, rows, search, options, selected) = (ui.clone(), rows.clone(), search.clone(), options.clone(), selected.clone());
        Rc::new(move || {
            clear(&rows);
            let q = search.text().to_string();
            // `FuseSearch` over the option strings, first 64 hits; no query lists everything.
            let hits: Vec<&String> = if q.trim().is_empty() {
                options.iter().collect()
            } else {
                crate::nodesearch::fuse::search_strings(&options, &q, 0.3).into_iter().take(64).collect()
            };
            let sel = selected();
            for o in hits {
                let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let ck = icon("check");
                ck.set_opacity(if sel.contains(o) { 1.0 } else { 0.0 });
                r.append(&ck);
                r.append(&gtk::Label::builder().label(o).xalign(0.0).hexpand(true).build());
                r.append(&dot(o));
                let b = gtk::Button::builder().child(&r).build();
                b.add_css_class("flat");
                b.add_css_class("ns-opt");
                b.set_focus_on_click(false);
                let (ui, o) = (ui.clone(), o.clone());
                b.connect_clicked(move |_| {
                    {
                        let mut s = ui.st.borrow_mut();
                        let f = Filter { kind, value: o.clone() };
                        if let Some(i) = s.c.filters.iter().position(|x| *x == f) {
                            s.c.filters.remove(i);
                        } else {
                            s.c.filters.push(f);
                        }
                        s.selected = 0;
                    }
                    ui.refresh();
                });
                rows.append(&b);
            }
            if rows.first_child().is_none() {
                let l = gtk::Label::builder().label("No Results").margin_top(12).margin_bottom(12).build();
                l.add_css_class("dim-label");
                rows.append(&l);
            }
        })
    };
    {
        let fill = fill.clone();
        search.connect_search_changed(move |_| fill());
    }
    {
        let ui = ui.clone();
        clear_all.connect_clicked(move |_| {
            ui.st.borrow_mut().c.filters.retain(|f| f.kind != kind);
            ui.refresh();
        });
    }
    // Called after every state change: dots, highlighted border, popover contents.
    let update = {
        let (btn, dots, count, clear_all) = (btn.clone(), dots, count, clear_all);
        move || {
            let sel = selected();
            clear(&dots);
            for v in sel.iter().take(MAX_DOTS) {
                dots.append(&dot(v));
            }
            if sel.len() > MAX_DOTS {
                dots.append(&gtk::Label::new(Some(&format!("+{}", sel.len() - MAX_DOTS))));
            }
            if sel.is_empty() {
                btn.remove_css_class("has-sel");
            } else {
                btn.add_css_class("has-sel");
            }
            count.set_label(&format!("{} items selected", sel.len()));
            clear_all.set_visible(!sel.is_empty());
            fill();
        }
    };
    ui.type_updates.borrow_mut().push(Box::new(update));
    btn
}

impl Workspace {
    pub(super) fn add_node_dialog(&self, doc: &DocRef, pos: (f64, f64), link: Option<Link>) {
        // An in-window card over a dark scrim (not a modal window): a click on the scrim, the
        // canvas around the card, closes it like the web's NodeSearchBoxPopover.
        let Some(host) = self.root.ancestor(gtk::Overlay::static_type()).and_downcast::<gtk::Overlay>() else { return };
        let scrim = gtk::Box::builder().css_classes(["cg-scrim-light"]).hexpand(true).vexpand(true).build();
        let card = gtk::Box::builder().css_classes(["addnode", "background"]).halign(gtk::Align::Center).valign(gtk::Align::Center)
            .width_request(880).height_request(660).overflow(gtk::Overflow::Hidden).build();
        scrim.append(&card);
        let dlg: Rc<dyn Fn()> = {
            let (host, scrim, canvas) = (host.clone(), scrim.clone(), doc.borrow().canvas.area.clone());
            Rc::new(move || {
                if scrim.parent().is_some() {
                    host.remove_overlay(&scrim);
                    canvas.grab_focus();
                }
            })
        };
        let click = gtk::GestureClick::new();
        {
            let (dlg, card) = (dlg.clone(), card.clone());
            click.connect_pressed(move |g, _, x, y| {
                let Some(w) = g.widget() else { return };
                if !w.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|p| p == card || p.is_ancestor(&card)) {
                    dlg();
                }
            });
        }
        scrim.add_controller(click);

        // Input row: applied filter chips + text box.
        let applied = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let entry = gtk::Text::builder().placeholder_text("Add a node…").hexpand(true).build();
        entry.add_css_class("ns-input");
        let input_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        input_row.set_margin_top(12);
        input_row.set_margin_bottom(8);
        input_row.set_margin_start(14);
        input_row.set_margin_end(14);
        input_row.append(&applied);
        input_row.append(&entry);

        // Filter bar: sidebar toggle | root chips | type chips.
        let toggle = gtk::Button::builder().child(&icon("panel-left")).build();
        toggle.add_css_class("ns-chip");
        toggle.set_focus_on_click(false);
        let roots = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let types = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.set_margin_start(14);
        bar.set_margin_end(14);
        bar.set_margin_bottom(10);
        bar.append(&toggle);
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(&roots);
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(&types);

        // Sidebar.
        let preset = gtk::Button::with_label("Most relevant");
        preset.add_css_class("ns-cat");
        preset.set_focus_on_click(false);
        if let Some(l) = preset.child().and_downcast::<gtk::Label>() {
            l.set_xalign(0.0);
        }
        let tree = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let side = gtk::Box::new(gtk::Orientation::Vertical, 4);
        side.set_width_request(208);
        side.set_hexpand(false);
        side.set_margin_start(10);
        side.set_margin_end(8);
        side.set_margin_bottom(8);
        side.append(&preset);
        side.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let tree_sc = gtk::ScrolledWindow::builder().child(&tree).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        side.append(&tree_sc);

        // Results.
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("ns-list");
        let scroll = gtk::ScrolledWindow::builder().child(&list).hexpand(true).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&side);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&scroll);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&input_row);
        root.append(&bar);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&body);
        root.set_hexpand(true);
        card.append(&root);

        let mut c = Content::default();
        // NodeSearchBoxPopover.showNewSearchBox: a dragged-out link pre-applies a type filter.
        // Dragged from an output → the new node needs an input of that type, and vice versa.
        // ponytail: "*"/untyped links apply no filter (the frontend would list only wildcard nodes).
        if let Some((_, _, from_output, ty)) = &link {
            if !ty.is_empty() && ty != "*" {
                c.filters.push(Filter { kind: if *from_output { FilterKind::Input } else { FilterKind::Output }, value: ty.clone() });
            }
        }
        let ui = Rc::new(Ui {
            ws: self.clone(),
            doc: doc.clone(),
            pos,
            link,
            dlg: dlg.clone(),
            st: RefCell::new(St { c, expanded: String::new(), selected: 0 }),
            entry: entry.clone(),
            applied,
            roots,
            tree,
            preset: preset.clone(),
            list: list.clone(),
            scroll,
            shown: RefCell::default(),
            type_updates: RefCell::default(),
        });
        types.append(&type_filter(&ui, FilterKind::Input));
        types.append(&type_filter(&ui, FilterKind::Output));

        {
            let side = side.clone();
            toggle.connect_clicked(move |_| side.set_visible(!side.is_visible()));
        }
        {
            let ui = ui.clone();
            preset.connect_clicked(move |_| {
                {
                    let mut s = ui.st.borrow_mut();
                    s.c.category = DEFAULT_CATEGORY.into();
                    s.expanded.clear();
                    s.selected = 0;
                }
                ui.refresh_tree();
                ui.refresh_results();
            });
        }
        {
            let ui = ui.clone();
            entry.connect_changed(move |e| {
                {
                    let mut s = ui.st.borrow_mut();
                    s.c.query = e.text().to_string();
                    s.selected = 0;
                }
                ui.refresh_results();
            });
        }
        {
            let ui = ui.clone();
            entry.connect_activate(move |_| {
                let i = ui.st.borrow().selected;
                ui.add(i);
            });
        }
        {
            let ui = ui.clone();
            list.connect_row_activated(move |_, r| ui.add(r.index() as usize));
        }
        // Up/Down move the highlight while typing; Esc closes (NodeSearchContent.vue keys).
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let (ui, dlg) = (ui.clone(), dlg.clone());
            keys.connect_key_pressed(move |_, key, _, _| {
                let i = ui.st.borrow().selected;
                match key {
                    gtk::gdk::Key::Down => ui.select(i + 1),
                    gtk::gdk::Key::Up => ui.select(i.saturating_sub(1)),
                    gtk::gdk::Key::Escape => dlg(),
                    _ => return gtk::glib::Propagation::Proceed,
                }
                gtk::glib::Propagation::Stop
            });
        }
        scrim.add_controller(keys);

        ui.refresh();
        host.add_overlay(&scrim);
        entry.grab_focus();
    }
}
