// SPDX-License-Identifier: GPL-3.0-only
//! Templates browser: `/templates/index.json` (categories + metadata), thumbnails from
//! `/templates/<name>-1.<mediaSubtype>`, opened as an unsaved tab. Custom-node templates come
//! from `/api/workflow_templates` (`{module: [name,…]}`) and live under `/api/workflow_templates/…`.

use super::*;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Default)]
struct Item {
    name: String,
    title: String,
    desc: String,
    subtype: String,
    tags: Vec<String>,
    models: Vec<String>,
    usage: f64,
    app: bool,
    /// `default` for the bundled index, else the custom node module.
    module: String,
}

impl Item {
    fn base(&self) -> String {
        if self.module == "default" {
            format!("/templates/{}", self.name)
        } else {
            format!("/api/workflow_templates/{}/{}", self.module, self.name)
        }
    }
    fn thumb(&self) -> String {
        format!("{}-1.{}", self.base(), if self.subtype.is_empty() { "webp" } else { &self.subtype })
    }
    fn json(&self) -> String {
        format!("{}.json", self.base())
    }
    fn haystack(&self) -> String {
        format!("{} {} {} {}", self.title, self.desc, self.tags.join(" "), self.models.join(" ")).to_lowercase()
    }
}

struct Cat {
    title: String,
    kind: String,
    items: Vec<Item>,
}

fn item(v: &Value, module: &str) -> Item {
    let strs = |k: &str| -> Vec<String> { v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect() };
    Item {
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        title: v["title"].as_str().unwrap_or_default().to_owned(),
        desc: v["description"].as_str().unwrap_or_default().to_owned(),
        subtype: v["mediaSubtype"].as_str().unwrap_or("webp").to_owned(),
        tags: strs("tags"),
        models: strs("models"),
        usage: v["usage"].as_f64().unwrap_or(0.0),
        app: v["isApp"].as_bool().unwrap_or(false),
        module: module.to_owned(),
    }
}

fn parse_index(v: &Value) -> Vec<Cat> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let module = c["moduleName"].as_str().unwrap_or("default");
            Cat {
                title: c["title"].as_str().unwrap_or_default().to_owned(),
                kind: c["type"].as_str().unwrap_or_default().to_owned(),
                items: c["templates"].as_array().into_iter().flatten().map(|t| item(t, module)).collect(),
            }
        })
        .collect()
}

#[derive(Clone)]
enum Page {
    All,
    Popular,
    Cat(usize),
    Module(String),
}

struct Browser {
    ws: Workspace,
    client: api::Client,
    win: gtk::Window,
    cats: std::cell::RefCell<Vec<Cat>>,
    modules: std::cell::RefCell<HashMap<String, Vec<String>>>,
    grid: gtk::Grid,
    entry: gtk::SearchEntry,
    status: gtk::Label,
    /// Items of the page currently shown; the grid is rebuilt from these on filter changes.
    items: std::cell::RefCell<Vec<Item>>,
    pages: std::cell::RefCell<Vec<Page>>,
    /// 0 = all types, 1 = node graphs, 2 = apps.
    kind: std::cell::Cell<u8>,
    gen: std::rc::Rc<std::cell::Cell<u32>>,
    cache: std::rc::Rc<std::cell::RefCell<HashMap<String, Vec<u8>>>>,
}

impl Browser {
    fn items_for(&self, page: &Page) -> Vec<Item> {
        match page {
            Page::All => self.cats.borrow().iter().flat_map(|c| c.items.iter().cloned()).collect(),
            Page::Popular => {
                let mut v: Vec<Item> = self.cats.borrow().iter().flat_map(|c| c.items.iter().cloned()).collect();
                v.sort_by(|a, b| b.usage.total_cmp(&a.usage));
                v
            }
            Page::Cat(i) => self.cats.borrow()[*i].items.clone(),
            Page::Module(m) => self
                .modules
                .borrow()
                .get(m)
                .into_iter()
                .flatten()
                .map(|n| Item { name: n.clone(), title: n.clone(), module: m.clone(), ..Default::default() })
                .collect(),
        }
    }

}

/// Fetch thumbnails one at a time (cached) so a 600-card page does not stampede the server.
fn load_thumbs(
    client: api::Client,
    cache: std::rc::Rc<std::cell::RefCell<HashMap<String, Vec<u8>>>>,
    gen: std::rc::Rc<std::cell::Cell<u32>>,
    my: u32,
    todo: Vec<(String, gtk::Image)>,
) {
    if todo.is_empty() {
        return;
    }
    glib::spawn_future_local(async move {
        for (url, pic) in todo {
            if gen.get() != my {
                return;
            }
            // Cache lookup must not hold a borrow across the insert below.
            let hit = cache.borrow().get(&url).cloned();
            let bytes = match hit {
                Some(b) => Some(b),
                None => {
                    let (c, u) = (client.clone(), url.clone());
                    match gio::spawn_blocking(move || c.view_query(&u)).await {
                        Ok(Ok(b)) => {
                            cache.borrow_mut().insert(url, b.clone());
                            Some(b)
                        }
                        _ => None,
                    }
                }
            };
            if let Some(b) = bytes {
                if gen.get() != my {
                    return;
                }
                if let Ok(tex) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from(&b)) {
                    pic.set_paintable(Some(&tex));
                }
            }
        }
    });
}

impl Workspace {
    /// Open the template browser (a modal window, like the web UI's Templates dialog).
    pub(super) fn open_templates(&self) {
        let Some(client) = self.client.borrow().clone() else {
            return self.append_log("[templates] not connected");
        };
        let win = adw::Window::builder().title("Templates").default_width(1180).default_height(820).build();
        // A toplevel that is not owned by the GtkApplication never gets pointer input on X11.
        if let Some(app) = self.root.clone().downcast::<gtk::Window>().ok().and_then(|w| w.application()).and_downcast::<gtk::Application>() {
            app.add_window(&win);
        }
        if let Some(root) = self.tabs.root().and_downcast::<gtk::Window>() {
            win.set_transient_for(Some(&root));
            // Modal would be nicer, but a modal toplevel without a window manager (Xvfb tests)
            // never receives pointer input; the dialog is still transient for the main window.
            let _ = &win;
        }
        let hb = adw::HeaderBar::new();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&hb);
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        let nav = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
        nav.set_halign(gtk::Align::Start);
        let nav_scroller = gtk::ScrolledWindow::builder()
            .child(&nav)
            .width_request(230)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let entry = gtk::SearchEntry::builder().placeholder_text("Search …").hexpand(true).build();
        let types = gtk::Box::builder().css_classes(["linked"]).build();
        let mut group: Option<gtk::ToggleButton> = None;
        for (i, label) in ["All", "Node graph", "Apps"].iter().enumerate() {
            let b = gtk::ToggleButton::with_label(label);
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(i == 0);
            types.append(&b);
        }
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.set_margin_top(8);
        top.set_margin_bottom(8);
        top.set_margin_start(12);
        top.set_margin_end(12);
        top.append(&entry);
        top.append(&types);

        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .row_spacing(12)
            .column_spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        let grid_scroller = gtk::ScrolledWindow::builder().child(&grid).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let status = gtk::Label::builder().xalign(0.0).margin_start(14).margin_bottom(6).css_classes(["dim-label", "caption"]).build();
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.append(&top);
        right.append(&grid_scroller);
        right.append(&status);
        body.append(&nav_scroller);
        body.append(&right);
        view.set_content(Some(&body));
        win.set_content(Some(&view));

        let br = std::rc::Rc::new(Browser {
            ws: self.clone(),
            client: client.clone(),
            win: win.clone().upcast(),
            cats: Default::default(),
            modules: Default::default(),
            grid,
            entry: entry.clone(),
            status,
            items: Default::default(),
            pages: Default::default(),
            kind: Default::default(),
            gen: Default::default(),
            cache: Default::default(),
        });
        br.status.set_text("Loading templates…");

        {
            let br = br.clone();
            let nav = nav.clone();
            glib::spawn_future_local(async move {
                let c = br.client.clone();
                let index = gio::spawn_blocking(move || c.get("/templates/index.json")).await;
                let c = br.client.clone();
                let modules = gio::spawn_blocking(move || c.get("/api/workflow_templates")).await;
                if let Ok(Ok(v)) = index {
                    *br.cats.borrow_mut() = parse_index(&v);
                }
                if let Ok(Ok(Value::Object(m))) = modules {
                    for (k, v) in m {
                        let names: Vec<String> = v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
                        if !names.is_empty() {
                            br.modules.borrow_mut().insert(k, names);
                        }
                    }
                }
                if br.cats.borrow().is_empty() {
                    br.status.set_text("Could not load /templates/index.json");
                    return;
                }
                build_nav(&br, &nav);
                nav.select_row(nav.row_at_index(0).as_ref());
            });
        }

        // Search + type filter.
        {
            let br = br.clone();
            entry.connect_search_changed(move |_| render(&br));
        }
        {
            let br = br.clone();
            let mut kids = Vec::new();
            let mut c = types.first_child();
            while let Some(w) = c {
                if let Some(t) = w.downcast_ref::<gtk::ToggleButton>() {
                    kids.push(t.clone());
                }
                c = w.next_sibling();
            }
            for (i, t) in kids.into_iter().enumerate() {
                let br = br.clone();
                t.connect_toggled(move |t| {
                    if t.is_active() {
                        br.kind.set(i as u8);
                        render(&br);
                    }
                });
            }
        }

        win.set_default_size(1180, 820);
        win.present();
    }
}


/// Build one template card; clicking it opens the template as an unsaved tab.
fn card(br: &std::rc::Rc<Browser>, it: &Item) -> (gtk::Button, gtk::Image) {
    // `Image` with an explicit pixel size reports that size as its natural width — a `Picture`
    // reports the texture's own width, which would collapse the grid to one card per row.
    let pic = gtk::Image::builder().pixel_size(132).css_classes(["card"]).build();
    let title = gtk::Label::builder()
        .label(&it.title)
        .xalign(0.0)
        .wrap(true)
        .lines(2)
        .max_width_chars(24)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("heading");
    let desc = gtk::Label::builder()
        .label(&it.desc)
        .xalign(0.0)
        .wrap(true)
        .lines(2)
        .max_width_chars(26)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    let kind = if it.app { "App" } else { "Node graph" };
    let foot = gtk::Label::builder()
        .label(format!("{kind}  ·  {}", it.tags.join(", ")))
        .xalign(0.0)
        .max_width_chars(26)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 4);
    bx.append(&pic);
    bx.append(&title);
    bx.append(&desc);
    bx.append(&foot);
    let card = gtk::Button::builder().child(&bx).width_request(250).css_classes(["flat"]).build();
    let (b2, t2) = (br.clone(), it.clone());
    card.connect_clicked(move |_| open(&b2, &t2));
    (card, pic)
}

/// Switch pages: remember the items and rebuild the grid.
fn show(br: &std::rc::Rc<Browser>, page: Page) {
    *br.items.borrow_mut() = br.items_for(&page);
    render(br);
}

/// Rebuild the card grid from `items`, filtered by the search box and the type tabs.
/// A `Grid` rather than a `FlowBox`: FlowBox swallowed clicks on the cards.
fn render(br: &std::rc::Rc<Browser>) {
    let my = br.gen.get() + 1;
    br.gen.set(my);
    while let Some(c) = br.grid.first_child() {
        br.grid.remove(&c);
    }
    let q = br.entry.text().to_lowercase();
    let kind = br.kind.get();
    let mut todo = Vec::new();
    let mut shown = 0i32;
    {
        let items = br.items.borrow();
        for it in items.iter() {
            let kind_ok = kind == 0 || (kind == 1 && !it.app) || (kind == 2 && it.app);
            if !kind_ok || !(q.is_empty() || it.haystack().contains(&q)) {
                continue;
            }
            let (card, pic) = card(br, it);
            if it.module == "default" {
                todo.push((it.thumb(), pic));
            }
            br.grid.attach(&card, shown % 3, shown / 3, 1, 1);
            shown += 1;
        }
    }
    br.status.set_text(&format!("{shown} templates"));
    load_thumbs(br.client.clone(), br.cache.clone(), br.gen.clone(), my, todo);
}

/// Fetch the template JSON and open it in an unsaved tab.
fn open(br: &std::rc::Rc<Browser>, it: &Item) {
    let (ws, client, url, title) = (br.ws.clone(), br.client.clone(), it.json(), it.title.clone());
    let win = br.win.clone();
    glib::spawn_future_local(async move {
        let c = client.clone();
        let Ok(Ok(bytes)) = gio::spawn_blocking(move || c.view_query(&url)).await else {
            return ws.append_log(&format!("[templates] {title}: fetch failed"));
        };
        let wf = serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| e.to_string())
            .and_then(workflow::Workflow::from_json);
        match wf {
            Ok(w) => {
                ws.open_temp(&title, w);
                win.close();
            }
            Err(e) => ws.append_log(&format!("[templates] {title}: {e}")),
        }
    });
}

fn build_nav(br: &std::rc::Rc<Browser>, nav: &gtk::ListBox) {
    let mut pages = br.pages.borrow_mut();
    pages.clear();
    let mut add = |label: &str, icon: Option<&str>, page: Page, header: bool| {
        let row = gtk::ListBoxRow::new();
        let bx = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bx.set_margin_top(6);
        bx.set_margin_bottom(6);
        bx.set_margin_start(10);
        bx.set_margin_end(10);
        if let Some(i) = icon {
            bx.append(&gtk::Image::from_icon_name(i));
        }
        let lb = gtk::Label::builder().label(label).xalign(0.0).build();
        if header {
            lb.add_css_class("heading");
            lb.add_css_class("caption");
        }
        bx.append(&lb);
        row.set_child(Some(&bx));
        row.set_activatable(!header);
        row.set_selectable(!header);
        nav.append(&row);
        pages.push(page);
    };
    add("All Templates", Some("view-grid-symbolic"), Page::All, false);
    add("Popular", Some("starred-symbolic"), Page::Popular, false);
    for (i, c) in br.cats.borrow().iter().enumerate() {
        let icon = match c.kind.as_str() {
            "image" => "image-x-generic-symbolic",
            "video" => "video-x-generic-symbolic",
            "audio" => "audio-x-generic-symbolic",
            "3d" => "applications-graphics-symbolic",
            "llm" => "utilities-terminal-symbolic",
            _ => "view-app-grid-symbolic",
        };
        add(&c.title, Some(icon), Page::Cat(i), false);
    }
    if !br.modules.borrow().is_empty() {
        add("Extensions", None, Page::All, true);
        let mut mods: Vec<String> = br.modules.borrow().keys().cloned().collect();
        mods.sort();
        for m in mods {
            add(&m.clone(), Some("folder-templates-symbolic"), Page::Module(m), false);
        }
    }
    let br2 = br.clone();
    nav.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let page = br2.pages.borrow().get(row.index() as usize).cloned();
        if let Some(p) = page {
            show(&br2, p);
        }
    });
}
