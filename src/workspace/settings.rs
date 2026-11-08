// SPDX-License-Identifier: GPL-3.0-only
//! Settings dialog, laid out like the web UI's.
//!
//! `/api/settings` only stores values; names, categories, types and options come from the
//! frontend/extension JS (`crate::settings_defs`). Both are fetched off the main thread, so
//! the window opens at once on the User page and the APPLICATION SETTINGS list fills in
//! when the scan finishes. Edits are optimistic: the local value changes, then
//! `POST /api/settings/{key}` runs in a worker and the footer reports the outcome.

use super::Workspace;
use crate::api;
use crate::settings_defs::{self, category_label, plain, Catalog, SettingDef};
use adw::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const ACCOUNT_URL: &str = "https://platform.comfy.org";

thread_local! {
    /// The single live Settings window (weak: it clears when the window is destroyed).
    static OPEN: RefCell<Option<gtk::glib::WeakRef<adw::Window>>> = const { RefCell::new(None) };
}

#[derive(Clone, PartialEq)]
enum Page {
    User,
    Cat(String),
    Keybinding,
    Extension,
    About,
}

struct Dlg {
    win: adw::Window,
    nav: gtk::ListBox,
    body: gtk::Box,
    status: gtk::Label,
    search: gtk::SearchEntry,
    /// Backing store: edited here first, so the next render shows the new value.
    values: RefCell<BTreeMap<String, Value>>,
    /// Definitions merged with the stored keys; None while loading.
    cat: RefCell<Option<Rc<Catalog>>>,
    client: Option<api::Client>,
    /// Workspace, so a snap setting takes effect without a reconnect.
    ws: Workspace,
    /// One entry per nav row; `None` for captions.
    pages: RefCell<Vec<Option<Page>>>,
    current: RefCell<Page>,
    nodes: usize,
}

/// Close the settings window if it is open; `true` when there was one (gear toggles).
pub(super) fn close_open() -> bool {
    // Hide, don't destroy: the window is a singleton, and a destroyed window cannot be
    // presented again (it stays reachable through the signal handlers' `Dlg`).
    match OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())) {
        Some(win) if win.is_visible() => {
            win.set_visible(false);
            true
        }
        _ => false,
    }
}

pub(super) fn open(ws: &Workspace) {
    // One window only: a second Ctrl+, or gear click raises the open one.
    if let Some(win) = OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())) {
        win.present();
        return;
    }
    let client = ws.client.borrow().clone();
    install_css();

    let win = adw::Window::builder().title("Settings").default_width(1040).default_height(780).build();
    // A toplevel the GtkApplication does not own never gets pointer input on X11.
    if let Some(root) = ws.tabs.root().and_downcast::<gtk::Window>() {
        win.set_transient_for(Some(&root));
        if let Some(app) = root.application().and_downcast::<gtk::Application>() {
            app.add_window(&win);
        }
    }

    {
        let w = win.clone();
        win.connect_close_request(move |_| {
            w.set_visible(false);
            gtk::glib::Propagation::Stop
        });
    }

    // Escape / Ctrl+W always get you out, whatever the window manager does with the
    // title bar (on a plain X session there may not be a close button at all).
    {
        let keys = gtk::EventControllerKey::new();
        // Capture: the focused search entry would otherwise eat Escape (clear the query).
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = win.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let esc = key == gtk::gdk::Key::Escape;
            let ctrl_w = key == gtk::gdk::Key::w && mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if esc || ctrl_w {
                w.set_visible(false);
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        win.add_controller(keys);
    }

    // Sidebar: "⚙ Settings", search, navigation list.
    let side_title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    side_title.append(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    side_title.append(&gtk::Label::builder().label("Settings").css_classes(["heading"]).build());
    let side_hb = adw::HeaderBar::builder().title_widget(&side_title).show_end_title_buttons(false).build();
    let search = gtk::SearchEntry::builder().placeholder_text("Search Settings...").margin_top(6).margin_bottom(6).margin_start(10).margin_end(10).build();
    let nav = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
    let nav_scroller = gtk::ScrolledWindow::builder().child(&nav).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side_box.append(&search);
    side_box.append(&nav_scroller);
    let side = adw::ToolbarView::builder().content(&side_box).width_request(260).css_classes(["cg-settings-sidebar"]).build();
    side.add_top_bar(&side_hb);

    // Content: clamped page + footer status.
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(12);
    body.set_margin_bottom(24);
    body.set_margin_start(24);
    body.set_margin_end(24);
    let clamp = adw::Clamp::builder().maximum_size(720).tightening_threshold(560).child(&body).build();
    let scroller = gtk::ScrolledWindow::builder().child(&clamp).hexpand(true).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    let status = gtk::Label::builder().xalign(0.0).css_classes(["dim-label", "caption"]).margin_start(24).margin_end(24).margin_top(6).margin_bottom(6).build();
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.append(&scroller);
    pane.append(&status);
    let content = adw::ToolbarView::builder().content(&pane).hexpand(true).build();
    content.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.append(&side);
    root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    root.append(&content);
    win.set_content(Some(&root));

    let dlg = Rc::new(Dlg {
        win: win.clone(),
        nav: nav.clone(),
        body,
        status,
        search: search.clone(),
        values: RefCell::new(BTreeMap::new()),
        cat: RefCell::new(None),
        client: client.clone(),
        ws: ws.clone(),
        pages: RefCell::new(Vec::new()),
        current: RefCell::new(Page::User),
        nodes: ws.registry.borrow().len(),
    });

    fill_nav(&dlg);
    let d = dlg.clone();
    nav.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let page = d.pages.borrow().get(row.index() as usize).cloned().flatten();
        let Some(page) = page else { return };
        *d.current.borrow_mut() = page.clone();
        if !d.search.text().is_empty() {
            d.search.set_text("");
        }
        show(&d, &page);
    });
    let d = dlg.clone();
    search.connect_search_changed(move |e| {
        let q = e.text().trim().to_lowercase();
        if q.is_empty() {
            let cur = d.current.borrow().clone();
            show(&d, &cur);
        } else {
            matches(&d, &q);
        }
    });
    select(&dlg, &Page::User);
    win.present();
    OPEN.with(|o| *o.borrow_mut() = Some(win.downgrade()));

    // Definitions (cached after the first scan) + current values, off the main thread.
    let Some(c) = client else {
        dlg.status.set_text("Not connected to a backend.");
        return;
    };
    dlg.status.set_text("Loading settings…");
    gtk::glib::spawn_future_local(async move {
        let res = gtk::gio::spawn_blocking(move || {
            let cat = settings_defs::catalog(&c);
            let values = c.settings();
            let merged = settings_defs::merge(&cat, values.as_ref().unwrap_or(&BTreeMap::new()));
            (merged, values)
        })
        .await;
        let Ok((merged, values)) = res else { return };
        match values {
            Ok(v) => {
                dlg.values.replace(v);
                dlg.status.set_text("");
            }
            Err(e) => {
                dlg.status.set_text(&format!("Could not read settings: {e}"));
            }
        }
        dlg.cat.replace(Some(Rc::new(merged)));
        fill_nav(&dlg);
        let cur = dlg.current.borrow().clone();
        select(&dlg, &cur);
    });
}

fn install_css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_string(".cg-settings-sidebar { background-color: @sidebar_bg_color; color: @sidebar_fg_color; }");
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn cat_icon(c: &str) -> &'static str {
    match c {
        "Comfy" => "emblem-system-symbolic",
        "LiteGraph" => "network-wired-symbolic",
        "Appearance" => "applications-graphics-symbolic",
        "Mask Editor" => "document-edit-symbolic",
        "3D" => "package-x-generic-symbolic",
        _ => "application-x-addon-symbolic",
    }
}

fn fill_nav(d: &Rc<Dlg>) {
    d.nav.remove_all();
    d.pages.borrow_mut().clear();
    caption(d, "ACCOUNT");
    item(d, "User", "avatar-default-symbolic", Page::User);
    caption(d, "APPLICATION SETTINGS");
    let cat = d.cat.borrow().clone();
    match cat {
        Some(cat) => {
            for c in &cat.categories {
                item(d, category_label(c), cat_icon(c), Page::Cat(c.clone()));
            }
        }
        None => {
            let row = gtk::ListBoxRow::builder().activatable(false).selectable(false).build();
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            bx.set_margin_start(12);
            bx.set_margin_top(6);
            bx.set_margin_bottom(6);
            bx.append(&gtk::Spinner::builder().spinning(true).build());
            bx.append(&gtk::Label::builder().label("Loading…").css_classes(["dim-label"]).build());
            row.set_child(Some(&bx));
            d.pages.borrow_mut().push(None);
            d.nav.append(&row);
        }
    }
    caption(d, "SPECIAL SETTINGS");
    item(d, "Keybinding", "input-keyboard-symbolic", Page::Keybinding);
    item(d, "Extension", "application-x-addon-symbolic", Page::Extension);
    item(d, "About", "help-about-symbolic", Page::About);
}

fn select(d: &Rc<Dlg>, page: &Page) {
    let i = d.pages.borrow().iter().position(|p| p.as_ref() == Some(page));
    match i.and_then(|i| d.nav.row_at_index(i as i32)) {
        // The row-selected handler renders it (and it re-renders if it was already selected).
        Some(row) => {
            d.nav.unselect_all();
            d.nav.select_row(Some(&row));
        }
        None => show(d, page),
    }
}

fn caption(d: &Rc<Dlg>, text: &str) {
    let row = gtk::ListBoxRow::builder().activatable(false).selectable(false).build();
    let l = gtk::Label::builder().label(text).xalign(0.0).css_classes(["dim-label", "caption-heading"]).margin_top(14).margin_bottom(4).margin_start(8).build();
    row.set_child(Some(&l));
    d.pages.borrow_mut().push(None);
    d.nav.append(&row);
}

fn item(d: &Rc<Dlg>, label: &str, icon: &str, page: Page) {
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bx.set_margin_top(4);
    bx.set_margin_bottom(4);
    bx.set_margin_start(4);
    bx.append(&gtk::Image::from_icon_name(icon));
    bx.append(&gtk::Label::builder().label(label).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
    let row = gtk::ListBoxRow::builder().child(&bx).build();
    d.pages.borrow_mut().push(Some(page));
    d.nav.append(&row);
}

fn clear(bx: &gtk::Box) {
    while let Some(c) = bx.first_child() {
        bx.remove(&c);
    }
}

fn title(d: &Rc<Dlg>, text: &str) {
    d.body.append(&gtk::Label::builder().label(text).xalign(0.0).css_classes(["title-2"]).margin_top(12).build());
    d.body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
}

fn note(d: &Rc<Dlg>, text: &str) {
    d.body.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label"]).build());
}

/// A section: optional heading + a boxed list to put rows in.
fn group(d: &Rc<Dlg>, heading: &str) -> adw::PreferencesGroup {
    if !heading.is_empty() {
        d.body.append(&gtk::Label::builder().label(heading).xalign(0.0).css_classes(["heading"]).margin_top(8).build());
    }
    let g = adw::PreferencesGroup::new();
    d.body.append(&g);
    g
}

fn show(d: &Rc<Dlg>, page: &Page) {
    clear(&d.body);
    let cat = d.cat.borrow().clone();
    match page {
        Page::User => user(d),
        Page::Keybinding => keybindings(d),
        Page::About => about(d),
        Page::Extension => match cat {
            Some(cat) => extensions(d, &cat),
            None => loading(d, "Extension"),
        },
        Page::Cat(c) => {
            title(d, category_label(c));
            if c == "Appearance" {
                theme_row(&group(d, "Comfy-GNOME"));
            }
            let Some(cat) = cat else { return loading(d, "") };
            let defs: Vec<&SettingDef> = cat.defs.iter().filter(|s| s.category() == c).collect();
            let mut sections: Vec<&str> = Vec::new();
            for s in &defs {
                if !sections.contains(&s.section()) {
                    sections.push(s.section());
                }
            }
            for sec in sections {
                let g = group(d, sec);
                for s in defs.iter().filter(|s| s.section() == sec) {
                    g.add(&setting_row(d, s));
                }
            }
        }
    }
}

fn loading(d: &Rc<Dlg>, name: &str) {
    if !name.is_empty() {
        title(d, name);
    }
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bx.append(&gtk::Spinner::builder().spinning(true).build());
    bx.append(&gtk::Label::builder().label("Loading setting definitions from the backend…").css_classes(["dim-label"]).build());
    d.body.append(&bx);
}

fn matches(d: &Rc<Dlg>, q: &str) {
    clear(&d.body);
    title(d, "Matches");
    let Some(cat) = d.cat.borrow().clone() else { return loading(d, "") };
    let hit = |s: &SettingDef| [&s.name, &s.id, &s.tooltip].iter().any(|t| t.to_lowercase().contains(q));
    let hits: Vec<&SettingDef> = cat.defs.iter().filter(|s| hit(s)).take(200).collect();
    note(d, &format!("{} setting(s) for “{q}”", hits.len()));
    for c in &cat.categories {
        let in_cat: Vec<&&SettingDef> = hits.iter().filter(|s| s.category() == c).collect();
        if in_cat.is_empty() {
            continue;
        }
        let g = group(d, category_label(c));
        for s in in_cat {
            g.add(&setting_row(d, s));
        }
    }
}

fn value_of(d: &Rc<Dlg>, s: &SettingDef) -> Value {
    d.values.borrow().get(&s.id).cloned().unwrap_or_else(|| s.default.clone())
}

fn setting_row(d: &Rc<Dlg>, s: &SettingDef) -> gtk::Widget {
    let v = value_of(d, s);
    let sub = s.tooltip.trim();
    let key = s.id.clone();
    match s.kind.as_str() {
        "boolean" if !v.is_array() && !v.is_object() => {
            let r = adw::SwitchRow::builder().title(&s.name).subtitle(sub).use_markup(false).active(v.as_bool().unwrap_or(false)).tooltip_text(&key).build();
            let d = d.clone();
            r.connect_active_notify(move |r| commit(&d, &key, Value::Bool(r.is_active())));
            r.upcast()
        }
        "number" | "slider" if v.is_number() || v.is_null() => {
            let int = s.step.map_or(v.is_i64() || v.is_u64() || v.is_null(), |st| st.fract() == 0.0);
            let step = s.step.unwrap_or(if int { 1.0 } else { 0.01 });
            let r = adw::SpinRow::with_range(s.min.unwrap_or(-1e9), s.max.unwrap_or(1e9), step);
            r.set_title(&s.name);
            r.set_subtitle(sub);
            r.set_use_markup(false);
            r.set_tooltip_text(Some(&key));
            r.set_digits(if int { 0 } else { (-step.log10()).ceil().clamp(1.0, 4.0) as u32 });
            r.set_value(v.as_f64().unwrap_or(0.0));
            let d = d.clone();
            r.connect_value_notify(move |r| {
                let v = if int { Value::from(r.value().round() as i64) } else { Value::from(r.value()) };
                if d.values.borrow().get(&key) != Some(&v) {
                    commit(&d, &key, v);
                }
            });
            r.upcast()
        }
        "combo" if !s.options.is_empty() => {
            let mut labels: Vec<String> = s.options.iter().map(|(l, _)| l.clone()).collect();
            let mut values: Vec<Value> = s.options.iter().map(|(_, v)| v.clone()).collect();
            // Keep a stored value the definition doesn't list, so the row doesn't lie.
            let at = values.iter().position(|o| *o == v || plain(o) == plain(&v)).unwrap_or_else(|| {
                labels.push(plain(&v));
                values.push(v.clone());
                values.len() - 1
            });
            let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            let r = adw::ComboRow::builder().title(&s.name).subtitle(sub).use_markup(false).model(&model).selected(at as u32).tooltip_text(&key).build();
            let d = d.clone();
            r.connect_selected_notify(move |r| {
                if let Some(v) = values.get(r.selected() as usize) {
                    commit(&d, &key, v.clone());
                }
            });
            r.upcast()
        }
        "text" | "string" | "url" | "color" | "combo" | "number" | "slider" if !v.is_array() && !v.is_object() => {
            let r = adw::ActionRow::builder().title(&s.name).subtitle(sub).use_markup(false).tooltip_text(&key).build();
            let e = gtk::Entry::builder().text(plain(&v).trim_matches('"')).width_chars(22).valign(gtk::Align::Center).build();
            if v.is_null() {
                e.set_text("");
            }
            let numeric = v.is_number();
            let save = {
                let d = d.clone();
                move |e: &gtk::Entry| {
                    let t = e.text().to_string();
                    let v = if numeric { t.parse::<f64>().map(Value::from).unwrap_or(Value::String(t)) } else { Value::String(t) };
                    if d.values.borrow().get(&key) != Some(&v) {
                        commit(&d, &key, v);
                    }
                }
            };
            let s2 = save.clone();
            e.connect_activate(move |e| s2(e));
            let focus = gtk::EventControllerFocus::new();
            let e2 = e.clone();
            focus.connect_leave(move |_| save(&e2));
            e.add_controller(focus);
            r.add_suffix(&e);
            r.upcast()
        }
        _ => {
            // Custom widgets (a JS function), images, lists/objects: show, don't pretend to edit.
            let r = adw::ActionRow::builder().title(&s.name).subtitle(sub).use_markup(false).tooltip_text(&key).build();
            let text = if s.kind == "custom" && v.is_null() { "Configured in the web UI".to_owned() } else { plain(&v) };
            r.add_suffix(&gtk::Label::builder().label(text).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(32).css_classes(["dim-label"]).build());
            r.upcast()
        }
    }
}

fn commit(d: &Rc<Dlg>, key: &str, v: Value) {
    d.values.borrow_mut().insert(key.to_owned(), v.clone());
    if key == "Comfy.LinkRenderMode" {
        let mode = v.as_i64().unwrap_or(2) as i32;
        d.ws.set_link_mode(mode);
    }
    if key == "Comfy.Execution.PreviewMethod" {
        d.ws.set_preview_method(v.as_str().unwrap_or("default"));
    }
    if key == "Comfy.SnapToGrid.GridSize" || key == "pysssss.SnapToGrid" {
        let values = d.values.borrow();
        let grid = values.get("Comfy.SnapToGrid.GridSize").and_then(Value::as_f64).unwrap_or(0.0);
        let always = values.get("pysssss.SnapToGrid").and_then(Value::as_bool).unwrap_or(false);
        drop(values);
        d.ws.set_snap(always, grid);
    }
    let Some(c) = d.client.clone() else { return };
    let (k, status) = (key.to_owned(), d.status.clone());
    let key = k.clone();
    gtk::glib::spawn_future_local(async move {
        match gtk::gio::spawn_blocking(move || c.set_setting(&k, v)).await {
            Ok(Ok(())) => status.set_text(&format!("Saved {key}")),
            Ok(Err(e)) => status.set_text(&format!("Could not save {key}: {e}")),
            Err(_) => {}
        }
    });
}

fn user(d: &Rc<Dlg>) {
    title(d, "My Account Settings");
    d.body.append(&gtk::Label::builder().label("Log in to your account").xalign(0.0).margin_top(6).build());
    let btn = gtk::Button::builder().child(&adw::ButtonContent::builder().icon_name("avatar-default-symbolic").label("Sign In / Sign Up").build()).css_classes(["suggested-action"]).halign(gtk::Align::Start).build();
    let (win, status) = (d.win.clone(), d.status.clone());
    btn.connect_clicked(move |_| {
        let status = status.clone();
        gtk::UriLauncher::new(ACCOUNT_URL).launch(Some(&win), gtk::gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                status.set_text(&format!("Could not open {ACCOUNT_URL}: {e}"));
            }
        });
    });
    d.body.append(&btn);
    note(d, "Sign-in happens in your browser at platform.comfy.org. Comfy-GNOME talks to your local backend and keeps no account session.");
}

fn keybindings(d: &Rc<Dlg>) {
    title(d, "Keybinding");
    note(d, "This app's shortcuts are fixed; the web UI's Comfy.Keybinding settings don't apply here.");
    let sections: [(&str, &[(&str, &str)]); 3] = [
        ("Queue", &[("Ctrl+Enter", "Run"), ("Ctrl+Shift+Enter", "Run at front of queue (Shift+Run button too)"), ("Ctrl+Alt+Enter", "Interrupt")]),
        (
            "Canvas",
            &[
                (".", "Fit view"),
                ("Esc", "Leave subgraph"),
                ("Alt+C", "Collapse selected nodes"),
                ("Ctrl+B", "Bypass selected nodes"),
                ("Ctrl+M", "Mute selected nodes"),
                ("Ctrl+G", "Group selected nodes"),
                ("Shift+Click", "Add to selection"),
            ],
        ),
        (
            "Workflow & windows",
            &[("Ctrl+S", "Save"), ("Ctrl+,", "Settings"), ("Shift+Close tab", "Close without the unsaved-changes prompt"), ("← → PgUp PgDn", "Image lightbox: step through a job's images"), ("Esc", "Image lightbox: close")],
        ),
    ];
    for (sec, keys) in sections {
        let g = group(d, sec);
        for (k, what) in keys {
            let r = adw::ActionRow::builder().title(*what).use_markup(false).build();
            r.add_suffix(&gtk::Label::builder().label(*k).css_classes(["monospace", "dim-label"]).build());
            g.add(&r);
        }
    }
}

fn extensions(d: &Rc<Dlg>, cat: &Catalog) {
    const KEY: &str = "Comfy.Extension.Disabled";
    title(d, "Extension");
    note(d, "Disabled extensions are stored in Comfy.Extension.Disabled. The web UI applies changes after a reload; this app doesn't run extension JS.");
    let mut packs: Vec<&str> = Vec::new();
    for (p, _) in &cat.extensions {
        if !packs.contains(&p.as_str()) {
            packs.push(p);
        }
    }
    for p in packs {
        let g = group(d, p);
        for (_, name) in cat.extensions.iter().filter(|(x, _)| x == p) {
            let off = d.values.borrow().get(KEY).and_then(Value::as_array).is_some_and(|a| a.iter().any(|v| v.as_str() == Some(name)));
            let r = adw::SwitchRow::builder().title(name).use_markup(false).active(!off).build();
            let (d, name) = (d.clone(), name.clone());
            r.connect_active_notify(move |r| {
                let mut list: Vec<Value> = d.values.borrow().get(KEY).and_then(Value::as_array).cloned().unwrap_or_default();
                list.retain(|v| v.as_str() != Some(&name));
                if !r.is_active() {
                    list.push(Value::String(name.clone()));
                }
                commit(&d, KEY, Value::Array(list));
            });
            g.add(&r);
        }
    }
}

fn theme_row(g: &adw::PreferencesGroup) {
    let model = gtk::StringList::new(&["Follow system", "Light", "Dark"]);
    let sm = adw::StyleManager::default();
    let sel = match sm.color_scheme() {
        adw::ColorScheme::ForceLight => 1,
        adw::ColorScheme::ForceDark => 2,
        _ => 0,
    };
    let r = adw::ComboRow::builder().title("Theme").subtitle("Applied by this app only").model(&model).selected(sel).build();
    r.connect_selected_notify(|c| {
        adw::StyleManager::default().set_color_scheme(match c.selected() {
            1 => adw::ColorScheme::ForceLight,
            2 => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        })
    });
    g.add(&r);
}

fn about(d: &Rc<Dlg>) {
    title(d, "About");
    let backend = d.client.as_ref().map(|c| c.base_url().to_owned()).unwrap_or_else(|| "not connected".into());
    let ver = d.values.borrow().get("Comfy.InstalledVersion").and_then(Value::as_str).unwrap_or("?").to_owned();
    let pkgs = [("Comfy-GNOME", env!("CARGO_PKG_VERSION").to_owned()), ("ComfyUI backend", format!("{ver} — {backend}")), ("GTK", format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version())), ("Node types", d.nodes.to_string())];
    let g = group(d, "");
    for (k, v) in pkgs {
        let r = adw::ActionRow::builder().title(k).subtitle(v).use_markup(false).subtitle_selectable(true).css_classes(["property"]).build();
        g.add(&r);
    }
}
