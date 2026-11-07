// SPDX-License-Identifier: GPL-3.0-only
//! Main workspace shell: header bar, sidebar (workflows/queue), canvas, log panel.

use crate::{api, backend, canvas, graph_view, prompt, registry, workflow};
use adw::{gio, glib, gtk, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const MAX_LOG_LINES: i32 = 5000;

#[derive(Clone)]
pub struct Workspace {
    pub root: gtk::Widget,
    title: adw::WindowTitle,
    log: gtk::TextView,
    proc: Rc<RefCell<Option<backend::Backend>>>,
    pub client: Rc<RefCell<Option<api::Client>>>,
    pub registry: Rc<RefCell<registry::Registry>>,
    tabs: adw::TabView,
    docs: Rc<RefCell<Vec<DocRef>>>,
    wf_list: gtk::ListBox,
    /// Sidebar row index → workflow file path (rows never reorder, only hide).
    files: Rc<RefCell<Vec<String>>>,
    crumb: gtk::Label,
    back: gtk::Button,
    queue_list: gtk::ListBox,
    /// Serialised last `/queue` snapshot — skip rebuilding identical lists.
    queue_key: Rc<RefCell<String>>,
    monitor: gtk::Label,
    monitor_detail: gtk::Label,
    monitor_btn: gtk::MenuButton,
    interrupt_btn: gtk::Button,
    preview: gtk::Picture,
    preview_caption: gtk::Label,
    bottom: adw::ViewStack,
}

type DocRef = Rc<RefCell<Doc>>;

/// One open workflow file = one tab, with its own canvas and subgraph path.
struct Doc {
    file: String,
    wf: workflow::Workflow,
    /// Subgraph definitions drilled into, outermost first.
    path: Vec<String>,
    canvas: canvas::Canvas,
    page: adw::TabPage,
    /// Edited since the last save (shown as a dot on the tab).
    dirty: bool,
}

/// Title text of a workflow row. libadwaita < 1.6 makes `AdwActionRow` a `GtkListBoxRow`
/// itself, so the title can live on the row or on its child — handle both.
fn row_title(row: &gtk::ListBoxRow) -> String {
    if let Some(r) = row.downcast_ref::<adw::ActionRow>() {
        return r.title().to_string();
    }
    row.child().and_then(|c| c.downcast::<adw::ActionRow>().ok()).map(|r| r.title().to_string()).unwrap_or_default()
}

/// The graph the tab is currently showing (root or the drilled-into subgraph).
fn graph_ref(d: &Doc) -> &workflow::Graph {
    d.path.last().and_then(|p| d.wf.subgraphs.get(p)).map(|s| &s.graph).unwrap_or(&d.wf.root)
}

fn graph_mut(d: &mut Doc) -> &mut workflow::Graph {
    match d.path.last().cloned() {
        Some(p) => &mut d.wf.subgraphs.get_mut(&p).expect("drilled subgraph vanished").graph,
        None => &mut d.wf.root,
    }
}

/// One row of a context menu: a button, a separator, or a row of colour swatches.
enum Entry {
    Item { label: String, danger: bool, f: Box<dyn Fn()> },
    Line,
    Swatches(Vec<(u32, Box<dyn Fn()>)>),
}

fn item(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: false, f: Box::new(f) }
}

fn danger(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: true, f: Box::new(f) }
}

/// Plain closure action, reachable from menus as `wb.<name>`.
fn placeholder(icon: &str, title: &str, desc: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(desc)
        .css_classes(["compact"])
        .build()
}

impl Workspace {
    pub fn new() -> Self {
        // Sidebar ----------------------------------------------------------------
        let side_stack = adw::ViewStack::new();
        // Single, not None: GTK disables click activation entirely in NONE mode.
        let wf_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        let wf_scroller = gtk::ScrolledWindow::builder().child(&wf_list).vexpand(true).build();
        let search = gtk::SearchEntry::builder().placeholder_text("Search workflows").hexpand(true).build();
        let reload = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Reload list").build();
        let wf_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        wf_bar.set_margin_top(6);
        wf_bar.set_margin_bottom(6);
        wf_bar.set_margin_start(6);
        wf_bar.set_margin_end(6);
        wf_bar.append(&search);
        wf_bar.append(&reload);
        let wf_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wf_page.append(&wf_bar);
        wf_page.append(&wf_scroller);
        // ponytail: client-side filter only; server-side search once lists get huge.
        {
            let list = wf_list.clone();
            search.connect_search_changed(move |e| {
                let q = e.text().to_lowercase();
                list.set_filter_func(move |row| {
                    q.is_empty() || row_title(row).to_lowercase().contains(&q)
                });
            });
        }
        side_stack.add_titled_with_icon(&wf_page, Some("workflows"), "Workflows", "folder-symbolic");
        let queue_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        let q_scroller = gtk::ScrolledWindow::builder().child(&queue_list).vexpand(true).build();
        let interrupt = gtk::Button::builder().label("Interrupt").sensitive(false).build();
        interrupt.add_css_class("destructive-action");
        interrupt.set_tooltip_text(Some("Stop the prompt that is running now"));
        let q_reload = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Reload queue").build();
        let q_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        q_bar.set_margin_top(6);
        q_bar.set_margin_bottom(6);
        q_bar.set_margin_start(6);
        q_bar.set_margin_end(6);
        q_bar.append(&interrupt);
        let q_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        q_spacer.set_hexpand(true);
        q_bar.append(&q_spacer);
        q_bar.append(&q_reload);
        let queue_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        queue_page.append(&q_bar);
        queue_page.append(&q_scroller);
        side_stack.add_titled_with_icon(&queue_page, Some("queue"), "Queue", "view-list-symbolic");
        let switcher = adw::ViewSwitcher::builder()
            .stack(&side_stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        let side_tb = adw::ToolbarView::new();
        side_tb.add_top_bar(&adw::HeaderBar::builder().title_widget(&switcher).show_end_title_buttons(false).build());
        side_tb.set_content(Some(&side_stack));

        // Canvas + log -----------------------------------------------------------
        let tabs = adw::TabView::new();
        tabs.set_vexpand(true);
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tabs));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);
        let canvas_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        canvas_pane.append(&tab_bar);
        canvas_pane.append(&tabs);

        let log = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(6).bottom_margin(6).left_margin(8).right_margin(8)
            .build();
        let log_scroller = gtk::ScrolledWindow::builder().child(&log).height_request(80).build();

        let preview = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let preview_caption = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["dim-label", "caption"]).build();
        let preview_page = gtk::Box::new(gtk::Orientation::Vertical, 4);
        preview_page.set_margin_top(6);
        preview_page.set_margin_bottom(6);
        preview_page.append(&preview);
        preview_page.append(&preview_caption);

        let bottom = adw::ViewStack::new();
        bottom.add_titled(&log_scroller, Some("log"), "Log");
        bottom.add_titled(&preview_page, Some("preview"), "Preview");
        let bottom_switcher = adw::ViewSwitcher::builder()
            .stack(&bottom)
            .policy(adw::ViewSwitcherPolicy::Narrow)
            .halign(gtk::Align::Start)
            .margin_start(6)
            .build();
        let bottom_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom_page.append(&bottom_switcher);
        bottom_page.append(&bottom);

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&canvas_pane)
            .end_child(&bottom_page)
            .resize_end_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        paned.set_position(520);

        // Header -----------------------------------------------------------------
        let title = adw::WindowTitle::new("Comfy-GNOME", "Not connected");
        let header = adw::HeaderBar::builder().title_widget(&title).show_start_title_buttons(false).build();
        let toggle = gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").active(true).tooltip_text("Toggle Sidebar").build();
        let reset = gtk::Button::builder().icon_name("zoom-original-symbolic").tooltip_text("Reset View").build();
        let fit = gtk::Button::builder().icon_name("zoom-fit-best-symbolic").tooltip_text("Fit All Nodes").build();
        let run = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Queue Prompt (Ctrl+Enter)")
            .css_classes(["suggested-action"])
            .build();
        header.pack_start(&toggle);
        let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Leave subgraph").visible(false).build();
        let crumb = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["dim-label"]).build();
        header.pack_start(&back);
        header.pack_start(&crumb);
        // Fixed width: the readout changes every second and would otherwise shove the
        // toolbar buttons around under the pointer.
        let monitor = gtk::Label::builder().width_chars(17).xalign(0.0).build();
        let mon_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        mon_box.append(&gtk::Image::from_icon_name("utilities-system-monitor-symbolic"));
        mon_box.append(&monitor);
        let mon_btn = gtk::MenuButton::builder().child(&mon_box).always_show_arrow(false).tooltip_text("Backend system monitor (DaSiWa nodes)").build();
        let mon_pop = gtk::Popover::new();
        let mon_detail = gtk::Label::builder().xalign(0.0).wrap(true).build();
        mon_pop.set_child(Some(&mon_detail));
        mon_btn.set_popover(Some(&mon_pop));
        mon_btn.set_visible(false);
        header.pack_end(&run);
        let save = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Save Workflow (Ctrl+S)").build();
        header.pack_end(&save);
        header.pack_end(&fit);
        header.pack_end(&reset);
        header.pack_end(&mon_btn);

        let content_tb = adw::ToolbarView::new();
        content_tb.add_top_bar(&header);
        content_tb.set_content(Some(&paned));

        let split = adw::OverlaySplitView::builder()
            .sidebar(&side_tb)
            .content(&content_tb)
            .min_sidebar_width(220.0)
            .build();
        toggle.bind_property("active", &split, "show-sidebar").bidirectional().sync_create().build();

        let ws = Self {
            root: split.upcast(),
            title,
            log,
            proc: Rc::default(),
            client: Rc::default(),
            registry: Rc::default(),
            tabs,
            docs: Rc::default(),
            wf_list,
            files: Rc::default(),
            crumb,
            back,
            queue_list,
            queue_key: Rc::default(),
            monitor,
            monitor_detail: mon_detail.clone(),
            monitor_btn: mon_btn.clone(),
            interrupt_btn: interrupt.clone(),
            preview,
            preview_caption,
            bottom,
        };
        {
            let me = ws.clone();
            run.connect_clicked(move |_| me.queue_prompt());
            let me = ws.clone();
            save.connect_clicked(move |_| me.save_active());
            let me = ws.clone();
            fit.connect_clicked(move |_| {
                if let Some(c) = me.active_canvas() {
                    c.fit_view();
                }
            });
            let me = ws.clone();
            reset.connect_clicked(move |_| {
                if let Some(c) = me.active_canvas() {
                    c.reset_view();
                }
            });
            let me = ws.clone();
            q_reload.connect_clicked(move |_| me.refresh_queue());
            let me = ws.clone();
            reload.connect_clicked(move |_| me.refresh_workflows());
            let me = ws.clone();
            interrupt.connect_clicked(move |_| me.interrupt());
            let me = ws.clone();
            let keys = gtk::ShortcutController::new();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>Return"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.queue_prompt();
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>s"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.save_active();
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>w"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.close_active();
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("Delete"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.delete_nodes(d, ids));
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>b"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.bypass(d, ids));
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>m"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.mute(d, ids));
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>d"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.duplicate_nodes(d, ids));
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>a"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(d) = me.active_doc() {
                        me.select_all(&d);
                    }
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>g"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.add_group(d, ids, None));
                    gtk::glib::Propagation::Stop
                })),
            ));
            ws.root.add_controller(keys);
        }
        ws.setup_navigation();
        ws
    }

    /// Convert the open workflow to an API prompt and queue it.
    fn queue_prompt(&self) {
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[run] not connected");
            return;
        };
        let Some(doc) = self.active_doc() else {
            self.append_log("[run] no workflow open");
            return;
        };
        let reg = self.registry.borrow();
        if reg.nodes.is_empty() {
            drop(reg);
            self.append_log("[run] node definitions not loaded yet");
            return;
        }
        let (api_prompt, warnings, raw) = {
            let d = doc.borrow();
            let (p, w) = prompt::build(&d.wf, &reg);
            (p, w, d.wf.raw.clone())
        };
        drop(reg);
        let n = api_prompt.as_object().map_or(0, |o| o.len());
        for w in warnings {
            self.append_log(&format!("[run] {w}"));
        }
        if n == 0 {
            self.append_log("[run] prompt is empty (all nodes muted, bypassed or unknown)");
            return;
        }
        self.append_log(&format!("[run] queueing {n} nodes…"));
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.queue_prompt(api_prompt, Some(raw))).await {
                Ok(Ok(id)) => me.append_log(&format!("[run] queued prompt {id}")),
                Ok(Err(e)) => me.append_log(&format!("[run] rejected: {e}")),
                Err(_) => me.append_log("[run] worker panicked"),
            }
        });
    }

    fn setup_navigation(&self) {
        let me = self.clone();
        self.queue_list.connect_map(move |_| me.refresh_queue());

        // row-selected, not row-activated: libadwaita rows don't fire activation reliably,
        // and this also gives keyboard navigation.
        let me = self.clone();
        self.wf_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            // Row widget name = file path (index shifts when the list is filtered).
            let name = row.widget_name().to_string();
            let file = if name.is_empty() { me.files.borrow().get(row.index() as usize).cloned() } else { Some(name) };
            if let Some(f) = file {
                me.load_workflow(f);
            }
        });

        let me = self.clone();
        self.back.connect_clicked(move |_| {
            if let Some(doc) = me.active_doc() {
                doc.borrow_mut().path.pop();
                me.render_doc(&doc);
            }
        });

        // Tab switching updates the breadcrumb / back button.
        let me = self.clone();
        self.tabs.connect_selected_page_notify(move |_| {
            if let Some(doc) = me.active_doc() {
                me.show_trail(&doc);
            }
        });

        // Drop the doc when its tab is closed (button on the tab, Ctrl+W, …).
        let docs = self.docs.clone();
        self.tabs.connect_page_detached(move |_, page, _| {
            docs.borrow_mut().retain(|d| &d.borrow().page != page);
        });

        let me = self.clone();
        self.wf_list.connect_map(move |_| me.refresh_workflows());
    }

    fn active_doc(&self) -> Option<DocRef> {
        let page = self.tabs.selected_page()?;
        self.docs.borrow().iter().find(|d| d.borrow().page == page).cloned()
    }

    fn active_canvas(&self) -> Option<canvas::Canvas> {
        self.active_doc().map(|d| d.borrow().canvas.clone())
    }

    fn close_active(&self) {
        if let Some(page) = self.tabs.selected_page() {
            self.tabs.close_page(&page);
        }
    }

    /// Push the canvas wiring back into the workflow model before it is used
    /// (save, run, switching views).
    fn sync_doc(&self, doc: &DocRef) {
        let (canvas, path) = {
            let d = doc.borrow();
            (d.canvas.clone(), d.path.clone())
        };
        let (nodes, links, groups) = canvas.scene();
        let (zoom, off) = canvas.view();
        let mut d = doc.borrow_mut();
        match path.last() {
            None => {
                graph_view::sync(&mut d.wf.root, &nodes, &links, &groups);
                // Persist the viewport like the web frontend (`extra.ds`).
                let ds = serde_json::json!({"scale": zoom, "offset": [off.0, off.1]});
                if let Some(extra) = d.wf.raw.get_mut("extra").and_then(|e| e.as_object_mut()) {
                    extra.insert("ds".into(), ds);
                }
            }
            Some(p) => {
                if let Some(sg) = d.wf.subgraphs.get_mut(p) {
                    graph_view::sync(&mut sg.graph, &nodes, &links, &groups);
                    graph_view::sync_io(sg);
                }
            }
        }
    }

    fn set_dirty(&self, doc: &DocRef, dirty: bool) {
        let mut d = doc.borrow_mut();
        if d.dirty == dirty {
            return;
        }
        d.dirty = dirty;
        let name = d.file.rsplit('/').next().unwrap_or(&d.file).trim_end_matches(".json").to_string();
        let title = if dirty { format!("{name} \u{2022}") } else { name };
        d.page.set_title(&title);
    }

    fn save_active(&self) {
        let Some(doc) = self.active_doc() else {
            self.append_log("[save] no workflow open");
            return;
        };
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[save] not connected");
            return;
        };
        self.sync_doc(&doc);
        let (file, body) = {
            let d = doc.borrow();
            (d.file.clone(), d.wf.to_json().to_string())
        };
        let me = self.clone();
        self.append_log(&format!("[save] {file}"));
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.save_workflow(&file, body.as_bytes())).await {
                Ok(Ok(())) => {
                    me.append_log("[save] saved");
                    me.set_dirty(&doc, false);
                }
                Ok(Err(e)) => me.append_log(&format!("[save] {e}")),
                Err(_) => me.append_log("[save] worker panicked"),
            }
        });
    }

    /// `GET /queue` → rebuild the sidebar list (cheap; called on every WS status event).
    fn refresh_queue(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.queue()).await {
                Ok(Ok(q)) => me.set_queue(q),
                Ok(Err(e)) => me.append_log(&format!("[queue] {e}")),
                Err(_) => me.append_log("[queue] worker panicked"),
            }
        });
    }

    fn set_queue(&self, q: api::Queue) {
        let key = format!("{q:?}");
        if *self.queue_key.borrow() == key {
            return;
        }
        *self.queue_key.borrow_mut() = key;
        while let Some(row) = self.queue_list.row_at_index(0) {
            self.queue_list.remove(&row);
        }
        self.interrupt_btn.set_sensitive(!q.running.is_empty());
        let client = self.client.borrow().clone();
        for (running, item) in std::iter::once(true).zip(&q.running).chain(std::iter::once(false).zip(&q.pending)) {
            let row = adw::ActionRow::builder()
                .title(format!("{} #{}", if running { "Running" } else { "Pending" }, item.number))
                .subtitle(format!(
                    "{} · {} nodes · {}",
                    &item.prompt_id[..item.prompt_id.len().min(8)],
                    item.nodes,
                    item.classes
                ))
                .build();
            let btn = gtk::Button::builder()
                .icon_name(if running { "media-playback-stop-symbolic" } else { "edit-delete-symbolic" })
                .tooltip_text(if running { "Interrupt" } else { "Remove from queue" })
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let (me, id, running, c) = (self.clone(), item.prompt_id.clone(), running, client.clone());
            btn.connect_clicked(move |_| {
                let Some(c) = c.clone() else { return };
                let (id, me) = (id.clone(), me.clone());
                me.append_log(if running { "[queue] interrupt" } else { "[queue] remove" });
                glib::spawn_future_local(async move {
                    let res = gio::spawn_blocking(move || {
                        if running { c.interrupt() } else { c.delete_queued(&[id]) }
                    })
                    .await;
                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => me.append_log(&format!("[queue] {e}")),
                        Err(_) => {}
                    }
                });
            });
            row.add_suffix(&btn);
            self.queue_list.append(&row);
        }
        if q.running.is_empty() && q.pending.is_empty() {
            let row = adw::ActionRow::builder().title("Queue is empty").css_classes(["dim-label"]).build();
            self.queue_list.append(&row);
        }
    }

    /// DaSiWa system monitor snapshot → header readout (no-op when that pack is absent).
    fn update_monitor(&self, data: &serde_json::Value) {
        let Some(cpu) = data["cpu_percent"].as_f64() else { return };
        let ram = &data["ram"];
        let gb = |b: f64| b / 1024.0 / 1024.0 / 1024.0;
        self.monitor.set_text(&match ram["percent"].as_f64() {
            Some(p) => format!("CPU {cpu:.0}% · RAM {p:.0}%"),
            None => format!("CPU {cpu:.0}%"),
        });
        let mut detail = format!(
            "CPU {cpu:.1}% of {} threads\nRAM {:.1}/{:.1} GB",
            data["cpu_count"].as_u64().unwrap_or(0),
            gb(ram["used"].as_f64().unwrap_or(0.0)),
            gb(ram["total"].as_f64().unwrap_or(0.0))
        );
        if let Some(swap) = data["swap"]["percent"].as_f64() {
            detail += &format!(" · swap {swap:.0}%");
        }
        for (i, g) in data["gpus"].as_array().into_iter().flatten().enumerate() {
            detail += &format!(
                "\nGPU{i} {}: {:.0}% · {:.1}/{:.1} GB",
                g["name"].as_str().unwrap_or("?"),
                g["utilization"].as_f64().unwrap_or(0.0),
                gb(g["memory_used"].as_f64().unwrap_or(0.0)),
                gb(g["memory_total"].as_f64().unwrap_or(0.0))
            );
            if let Some(t) = g["temperature"].as_f64() {
                detail += &format!(" · {t:.0}°C");
            }
        }
        self.monitor_detail.set_text(&detail);
        self.monitor_btn.set_visible(true);
    }

    /// Decode image bytes (PNG/JPEG) and put them in the Preview tab.
    fn show_image(&self, bytes: Vec<u8>, caption: &str) {
        let bytes = glib::Bytes::from_owned(bytes);
        match gtk::gdk::Texture::from_bytes(&bytes) {
            Ok(tex) => {
                self.preview.set_paintable(Some(&tex));
                self.preview_caption.set_text(caption);
                self.bottom.set_visible_child_name("preview");
            }
            Err(e) => self.append_log(&format!("[preview] {caption}: {e}")),
        }
    }

    /// Interrupt whatever is running now.
    fn interrupt(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        self.append_log("[queue] interrupt");
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.interrupt()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => me.append_log(&format!("[queue] {e}")),
                Err(_) => {}
            }
        });
    }

    /// Fetch the user's workflow list (silent when not connected).
    fn refresh_workflows(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.workflows()).await {
                Ok(Ok(files)) => me.set_workflow_list(files),
                Ok(Err(e)) => me.append_log(&format!("[workflows] {e}")),
                Err(_) => me.append_log("[workflows] worker panicked"),
            }
        });
    }

    fn set_workflow_list(&self, mut files: Vec<String>) {
        files.sort_by_key(|f| f.to_lowercase());
        let mut list = self.files.borrow_mut();
        list.clone_from(&files);
        drop(list);
        while let Some(row) = self.wf_list.row_at_index(0) {
            self.wf_list.remove(&row);
        }
        for f in &files {
            let row = match f.rsplit_once('/') {
                // AdwActionRow titles are Pango markup: escape '&' in workflow names.
                Some((dir, name)) => adw::ActionRow::builder()
                    .title(glib::markup_escape_text(name.trim_end_matches(".json")))
                    .subtitle(glib::markup_escape_text(dir))
                    .build(),
                None => adw::ActionRow::builder().title(glib::markup_escape_text(f.trim_end_matches(".json"))).build(),
            };
            row.set_tooltip_text(Some(f));
            row.set_widget_name(f);
            self.wf_list.append(&row);
        }
        self.append_log(&format!("{} workflows", files.len()));
    }

    fn load_workflow(&self, file: String) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let f = file.clone();
            let text = gio::spawn_blocking(move || client.workflow_json(&f)).await;
            let value = match text {
                Ok(Ok(t)) => match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(e) => return me.append_log(&format!("[workflow] {file}: {e}")),
                },
                Ok(Err(e)) => return me.append_log(&format!("[workflow] {file}: {e}")),
                Err(_) => return me.append_log("[workflow] worker panicked"),
            };
            let wf = match workflow::Workflow::from_json(value) {
                Ok(w) => w,
                Err(e) => return me.append_log(&format!("[workflow] {file}: {e}")),
            };
            // Log the saved viewport: test harnesses need it to turn node positions
            // in the file into screen coordinates.
            let ds = &wf.raw["extra"]["ds"];
            me.append_log(&format!(
                "Opened {file}: {} nodes, {} links, {} subgraphs [ds {} {}]",
                wf.root.nodes.len(),
                wf.root.links.len(),
                wf.subgraphs.len(),
                ds["scale"],
                ds["offset"]
            ));
            me.open_workflow(file, wf);
        });
    }

    /// Show a workflow in its own tab (or switch to the tab that already has it).
    fn open_workflow(&self, file: String, wf: workflow::Workflow) {
        if let Some(doc) = self.docs.borrow().iter().find(|d| d.borrow().file == file).cloned() {
            let page = doc.borrow().page.clone();
            self.tabs.set_selected_page(&page);
            return;
        }
        let canvas = canvas::Canvas::new();
        let page = self.tabs.append(&canvas.area);
        page.set_title(file.rsplit('/').next().unwrap_or(&file).trim_end_matches(".json"));
        page.set_tooltip(&file);
        let doc: DocRef = Rc::new(RefCell::new(Doc {
            file,
            wf,
            path: Vec::new(),
            canvas: canvas.clone(),
            page: page.clone(),
            dirty: false,
        }));
        self.docs.borrow_mut().push(doc.clone());
        {
            // Double-click a subgraph instance: drill in.
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_activate(move |id| {
                if let Some(d) = weak.upgrade() {
                    me.open_node(&d, id);
                }
            });
            // Right click: node / group / canvas menu.
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_context(move |x, y, hit| {
                if let Some(d) = weak.upgrade() {
                    me.context_menu(&d, x, y, hit);
                }
            });
            // Any model change marks the tab dirty (and keeps the model current).
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_change(move || {
                if let Some(d) = weak.upgrade() {
                    me.set_dirty(&d, true);
                }
            });
        }
        self.render_doc(&doc);
        self.tabs.set_selected_page(&page);
    }

    /// Double-click: drill into a subgraph instance.
    fn open_node(&self, doc: &DocRef, id: &str) {
        let ty = {
            let d = doc.borrow_mut();
            let graph = d.path.last().and_then(|p| d.wf.subgraphs.get(p)).map(|s| &s.graph).unwrap_or(&d.wf.root);
            match graph.node(id) {
                Some(n) if d.wf.subgraphs.contains_key(&n.ty) => n.ty.clone(),
                _ => return,
            }
        };
        doc.borrow_mut().path.push(ty);
        self.render_doc(doc);
    }

    fn show_trail(&self, doc: &DocRef) {
        let d = doc.borrow();
        let mut trail = vec![d.file.rsplit('/').next().unwrap_or_default().to_owned()];
        trail.extend(d.path.iter().map(|p| d.wf.subgraphs[p].name.clone()));
        self.crumb.set_text(&trail.join(" › "));
        self.back.set_visible(!d.path.is_empty());
    }

    fn render_doc(&self, doc: &DocRef) {
        let (canvas, nodes, links, groups, saved_view) = {
            let d = doc.borrow();
            let def = d.path.last().and_then(|p| d.wf.subgraphs.get(p));
            let graph = def.map(|s| &s.graph).unwrap_or(&d.wf.root);
            let (nodes, links, groups) = graph_view::scene(&d.wf, graph, def, &self.registry.borrow());
            // Root graph: restore the viewport the web frontend saved (`extra.ds`).
            let saved_view = d
                .path
                .is_empty()
                .then(|| d.wf.raw["extra"]["ds"].as_object())
                .flatten()
                .and_then(|ds| {
                    let scale = ds.get("scale")?.as_f64()?;
                    let off = ds.get("offset")?.as_array()?;
                    Some((scale, (off.first()?.as_f64()?, off.get(1)?.as_f64()?)))
                });
            (d.canvas.clone(), nodes, links, groups, saved_view)
        };
        self.show_trail(doc);
        canvas.set_graph(nodes, links, groups);
        if let Some((zoom, off)) = saved_view {
            canvas.set_view(zoom, off);
        }
    }

    /// Re-run `scene()` for every tab (after `/object_info` arrives) without
    /// losing its viewport.
    fn redraw_all(&self) {
        for doc in self.docs.borrow().iter() {
            let view = doc.borrow().canvas.view();
            self.render_doc(doc);
            doc.borrow().canvas.set_view(view.0, view.1);
        }
    }

    /// Run `f` with the active tab and its current selection (no-op when empty).
    fn with_selection(&self, f: impl Fn(&Self, &DocRef, &[String])) {
        let Some(doc) = self.active_doc() else { return };
        let mut ids = doc.borrow().canvas.selected();
        // Subgraph boundary pseudo-nodes ("Inputs"/"Outputs") are view-only: they are not
        // in the model, so editing them must be a no-op rather than a broken menu.
        {
            let d = doc.borrow();
            let g = graph_ref(&d);
            ids.retain(|id| g.nodes.iter().any(|n| &n.id == id));
        }
        if ids.is_empty() {
            self.append_log("nothing selected");
            return;
        }
        f(self, &doc, &ids);
    }

    /// Re-render from the model, keeping the viewport where it is.
    fn refresh_doc(&self, doc: &DocRef) {
        let view = doc.borrow().canvas.view();
        self.render_doc(doc);
        doc.borrow().canvas.set_view(view.0, view.1);
        // Keep keyboard shortcuts (Ctrl+S, Delete…) alive after a dialog or menu closes.
        doc.borrow().canvas.area.grab_focus();
    }

    fn edited(&self, doc: &DocRef) {
        self.refresh_doc(doc);
        self.set_dirty(doc, true);
    }

    fn is_bypassed(&self, doc: &DocRef, ids: &[String]) -> bool {
        let d = doc.borrow();
        graph_ref(&d).nodes.iter().any(|n| ids.contains(&n.id) && n.mode == workflow::MODE_BYPASS)
    }

    /// Toggle bypass (mode 4): the node is skipped when the prompt is built and its
    /// input is passed through to a matching output.
    fn bypass(&self, doc: &DocRef, ids: &[String]) {
        let off = self.is_bypassed(doc, ids);
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.mode = if off { 0 } else { workflow::MODE_BYPASS };
            }
        }
        self.edited(doc);
    }

    /// Toggle mute (mode 2): the node does not execute and its outputs go dark.
    fn mute(&self, doc: &DocRef, ids: &[String]) {
        let off = {
            let d = doc.borrow();
            graph_ref(&d).nodes.iter().any(|n| ids.contains(&n.id) && n.mode == workflow::MODE_NEVER)
        };
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.mode = if off { 0 } else { workflow::MODE_NEVER };
            }
        }
        self.edited(doc);
    }

    fn collapse(&self, doc: &DocRef, ids: &[String]) {
        let off = {
            let d = doc.borrow();
            graph_ref(&d).nodes.iter().any(|n| ids.contains(&n.id) && n.collapsed)
        };
        {
            let mut d = doc.borrow_mut();
            for n in graph_mut(&mut d).nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
                n.collapsed = !off;
            }
        }
        self.edited(doc);
    }

    fn set_title(&self, doc: &DocRef, id: &str, title: String) {
        {
            let mut d = doc.borrow_mut();
            if let Some(n) = graph_mut(&mut d).nodes.iter_mut().find(|n| n.id == id) {
                n.title = if title.trim().is_empty() { None } else { Some(title) };
            }
        }
        self.edited(doc);
    }

    fn delete_nodes(&self, doc: &DocRef, ids: &[String]) {
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            g.nodes.retain(|n| !ids.contains(&n.id));
            g.links.retain(|_, l| !ids.contains(&l.origin_id) && !ids.contains(&l.target_id));
        }
        doc.borrow().canvas.select(&[]);
        self.edited(doc);
    }

    fn duplicate_nodes(&self, doc: &DocRef, ids: &[String]) {
        let mut fresh = Vec::new();
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let originals: Vec<workflow::Node> = g.nodes.iter().filter(|n| ids.contains(&n.id)).cloned().collect();
            for mut n in originals {
                n.id = g.next_id();
                n.pos = (n.pos.0 + 30.0, n.pos.1 + 30.0);
                for i in &mut n.inputs {
                    i.link = None;
                }
                for o in &mut n.outputs {
                    o.links.clear();
                }
                fresh.push(n.id.clone());
                g.nodes.push(n);
            }
        }
        self.refresh_doc(doc);
        doc.borrow().canvas.select(&fresh);
        self.set_dirty(doc, true);
    }

    /// Group the selection (`at` = fixed rect for the canvas menu, else fit the nodes).
    fn add_group(&self, doc: &DocRef, ids: &[String], at: Option<(f64, f64)>) {
        let rect = {
            let d = doc.borrow();
            let sel: Vec<&workflow::Node> = graph_ref(&d).nodes.iter().filter(|n| ids.contains(&n.id)).collect();
            if let Some(p) = at {
                [p.0, p.1, 420.0, 260.0]
            } else if sel.is_empty() {
                [0.0, 0.0, 420.0, 260.0]
            } else {
                let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for n in sel {
                    b[0] = b[0].min(n.pos.0);
                    b[1] = b[1].min(n.pos.1 - 30.0);
                    b[2] = b[2].max(n.pos.0 + n.size.0);
                    b[3] = b[3].max(n.pos.1 + n.size.1);
                }
                [b[0] - 20.0, b[1] - 45.0, b[2] - b[0] + 40.0, b[3] - b[1] + 70.0]
            }
        };
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let title = format!("Group {}", g.groups.len() + 1);
            g.groups.push(workflow::Group { title, bounding: rect, color: Some("#3f789e".into()) });
        }
        self.edited(doc);
    }

    fn rename_group(&self, doc: &DocRef, gi: usize, title: String) {
        {
            let mut d = doc.borrow_mut();
            if let Some(g) = graph_mut(&mut d).groups.get_mut(gi) {
                g.title = title;
            }
        }
        self.edited(doc);
    }

    fn set_group_color(&self, doc: &DocRef, gi: usize, color: &str) {
        {
            let mut d = doc.borrow_mut();
            if let Some(g) = graph_mut(&mut d).groups.get_mut(gi) {
                g.color = Some(color.to_string());
            }
        }
        self.edited(doc);
    }

    fn delete_group(&self, doc: &DocRef, gi: usize) {
        {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            if gi < g.groups.len() {
                g.groups.remove(gi);
            }
        }
        self.edited(doc);
    }

    fn fit_rect(&self, canvas: &canvas::Canvas, r: [f64; 4]) {
        let (w, h) = (canvas.area.width() as f64, canvas.area.height() as f64);
        if w < 10.0 || h < 10.0 || r[2] <= 0.0 || r[3] <= 0.0 {
            return;
        }
        let m = 40.0;
        let zoom = ((w - 2.0 * m) / r[2]).min((h - 2.0 * m) / r[3]).clamp(0.1, 1.0);
        canvas.set_view(zoom, (w / 2.0 - (r[0] + r[2] / 2.0) * zoom, h / 2.0 - (r[1] + r[3] / 2.0) * zoom));
    }

    fn fit_group(&self, doc: &DocRef, gi: usize) {
        let (canvas, rect) = {
            let d = doc.borrow();
            (d.canvas.clone(), graph_ref(&d).groups.get(gi).map(|g| g.bounding))
        };
        if let Some(r) = rect {
            self.fit_rect(&canvas, r);
        }
    }

    fn select_all(&self, doc: &DocRef) {
        let ids: Vec<String> = {
            let d = doc.borrow();
            graph_ref(&d).nodes.iter().map(|n| n.id.clone()).collect()
        };
        doc.borrow().canvas.select(&ids);
        self.append_log(&format!("selected {} nodes", ids.len()));
    }

    /// Create a node from `/object_info` defaults at a canvas position.
    fn create_node(&self, doc: &DocRef, name: &str, pos: (f64, f64)) {
        let Some(def) = self.registry.borrow().get(name).cloned() else {
            self.append_log(&format!("unknown node type {name}"));
            return;
        };
        let id = {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let id = g.next_id();
            g.nodes.push(workflow::Node::from_def(&def, id.clone(), pos));
            id
        };
        self.refresh_doc(doc);
        doc.borrow().canvas.select(&[id]);
        self.set_dirty(doc, true);
        self.append_log(&format!("added {} at ({:.0}, {:.0})", def.display_name, pos.0, pos.1));
    }

    /// Open a popover menu at widget coordinates. Plain buttons, not GMenuModel:
    /// menu-item action resolution through widget action groups proved unreliable here.
    fn show_menu(&self, area: &gtk::DrawingArea, x: f64, y: f64, entries: Vec<Entry>) {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for e in entries {
            match e {
                Entry::Item { label, danger, f } => {
                    let lb = gtk::Label::builder().label(&label).xalign(0.0).build();
                    if danger {
                        lb.add_css_class("error");
                    }
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    row.set_margin_start(10);
                    row.set_margin_end(18);
                    row.append(&lb);
                    let b = gtk::Button::builder().child(&row).build();
                    b.add_css_class("flat");
                    b.set_halign(gtk::Align::Fill);
                    let p = pop.downgrade();
                    b.connect_clicked(move |_| {
                        if let Some(p) = p.upgrade() {
                            p.popdown();
                        }
                        f();
                    });
                    bx.append(&b);
                }
                Entry::Line => {
                    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                    sep.set_margin_top(3);
                    sep.set_margin_bottom(3);
                    bx.append(&sep);
                }
                Entry::Swatches(list) => {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    row.set_margin_start(10);
                    row.set_margin_top(4);
                    row.set_margin_bottom(4);
                    for (hex, f) in list {
                        let da = gtk::DrawingArea::new();
                        da.set_content_width(20);
                        da.set_content_height(20);
                        da.set_draw_func(move |_, cr, w, h| {
                            cr.set_source_rgb(
                                ((hex >> 16) & 0xff) as f64 / 255.0,
                                ((hex >> 8) & 0xff) as f64 / 255.0,
                                (hex & 0xff) as f64 / 255.0,
                            );
                            cr.rectangle(0.0, 0.0, w as f64, h as f64);
                            cr.fill().ok();
                        });
                        let b = gtk::Button::builder().child(&da).build();
                        b.add_css_class("flat");
                        let p = pop.downgrade();
                        b.connect_clicked(move |_| {
                            if let Some(p) = p.upgrade() {
                                p.popdown();
                            }
                            f();
                        });
                        row.append(&b);
                    }
                    bx.append(&row);
                }
            }
        }
        bx.set_margin_top(6);
        bx.set_margin_bottom(6);
        bx.set_margin_start(6);
        bx.set_margin_end(6);
        pop.set_child(Some(&bx));
        pop.set_parent(area);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let p = pop.clone();
        pop.connect_closed(move |_| p.unparent());
        pop.popup();
    }

    /// Context menu for whatever the right click landed on.
    fn context_menu(&self, doc: &DocRef, x: f64, y: f64, hit: canvas::Hit) {
        // Pseudo-nodes have no model counterpart: fall back to the canvas menu.
        let hit = match &hit {
            canvas::Hit::Node(id) if !graph_ref(&doc.borrow()).nodes.iter().any(|n| &n.id == id) => canvas::Hit::Empty,
            _ => hit,
        };
        let area = doc.borrow().canvas.area.clone();
        let ids = doc.borrow().canvas.selected();
        let mut entries: Vec<Entry> = Vec::new();
        match hit {
            canvas::Hit::Node(id) => {
                let (mode, collapsed, title, is_sub) = {
                    let d = doc.borrow();
                    match graph_ref(&d).node(&id) {
                        Some(n) => (n.mode, n.collapsed, n.title.clone().unwrap_or_default(), d.wf.subgraphs.contains_key(&n.ty)),
                        None => return,
                    }
                };
                let bypassed = mode == workflow::MODE_BYPASS;
                let muted = mode == workflow::MODE_NEVER;
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(item(
                        if bypassed { "Un-Bypass Node" } else { "Bypass Node (Ctrl+B)" },
                        move || me.bypass(&d, &i),
                    ));
                }
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(item(
                        if muted { "Un-Mute Node" } else { "Mute Node (Ctrl+M)" },
                        move || me.mute(&d, &i),
                    ));
                }
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(item(if collapsed { "Expand Node" } else { "Collapse Node" }, move || me.collapse(&d, &i)));
                }
                entries.push(Entry::Line);
                {
                    let (me, d) = (self.clone(), doc.clone());
                    let (node, initial) = (id.clone(), title.clone());
                    entries.push(item("Edit Title…", move || {
                        let (me2, d2, node2, initial2) = (me.clone(), d.clone(), node.clone(), initial.clone());
                        me.text_dialog("Node Title", &initial2, move |t| me2.set_title(&d2, &node2, t));
                    }));
                }
                if is_sub {
                    let (me, d) = (self.clone(), doc.clone());
                    let node = id.clone();
                    entries.push(item("Open Subgraph", move || me.open_node(&d, &node)));
                }
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(item("Duplicate Node", move || me.duplicate_nodes(&d, &i)));
                }
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(danger("Delete Node", move || me.delete_nodes(&d, &i)));
                }
            }
            canvas::Hit::Group(gi) => {
                let title = {
                    let d = doc.borrow();
                    graph_ref(&d).groups.get(gi).map(|g| g.title.clone()).unwrap_or_default()
                };
                {
                    let (me, d) = (self.clone(), doc.clone());
                    let initial = title.clone();
                    entries.push(item("Rename Group…", move || {
                        let (me2, d2, initial2) = (me.clone(), d.clone(), initial.clone());
                        me.text_dialog("Group Title", &initial2, move |t| me2.rename_group(&d2, gi, t));
                    }));
                }
                entries.push(Entry::Swatches(
                    [
                        ("#3f789e", 0x3f789e),
                        ("#ff6b6b", 0xff6b6b),
                        ("#59b259", 0x59b259),
                        ("#b39ddb", 0xb39ddb),
                        ("#ffd500", 0xffd500),
                        ("#59d0ff", 0x59d0ff),
                        ("#ff8fb1", 0xff8fb1),
                        ("#ffa931", 0xffa931),
                    ]
                    .into_iter()
                    .map(|(hex, rgb)| {
                        let (me, d) = (self.clone(), doc.clone());
                        (rgb, Box::new(move || me.set_group_color(&d, gi, hex)) as Box<dyn Fn()>)
                    })
                    .collect(),
                ));
                {
                    let (me, d) = (self.clone(), doc.clone());
                    entries.push(item("Fit to Group", move || me.fit_group(&d, gi)));
                }
                {
                    let (me, d) = (self.clone(), doc.clone());
                    entries.push(danger("Delete Group", move || me.delete_group(&d, gi)));
                }
            }
            canvas::Hit::Empty => {
                let (zoom, off) = doc.borrow().canvas.view();
                let pos = ((x - off.0) / zoom, (y - off.1) / zoom);
                {
                    let (me, d) = (self.clone(), doc.clone());
                    entries.push(item("Add Node…", move || me.add_node_dialog(&d, pos)));
                }
                {
                    let (me, d, i) = (self.clone(), doc.clone(), ids.clone());
                    entries.push(item("Add Group", move || me.add_group(&d, &i, Some(pos))));
                }
                entries.push(Entry::Line);
                {
                    let (me, d) = (self.clone(), doc.clone());
                    entries.push(item("Select All (Ctrl+A)", move || me.select_all(&d)));
                }
                {
                    let d = doc.clone();
                    entries.push(item("Fit View", move || d.borrow().canvas.fit_view()));
                }
                {
                    let d = doc.clone();
                    entries.push(item("Reset View", move || d.borrow().canvas.reset_view()));
                }
            }
        }
        self.show_menu(&area, x, y, entries);
    }

    /// One-button-per-result node search over `/object_info`.
    fn add_node_dialog(&self, doc: &DocRef, pos: (f64, f64)) {
        let Some(win) = self.root.root().and_downcast::<gtk::Window>() else { return };
        let dlg = gtk::Window::builder()
            .title("Add Node")
            .transient_for(&win)
            .modal(true)
            .default_width(460)
            .default_height(560)
            .build();
        let hb = gtk::HeaderBar::new();
        dlg.set_titlebar(Some(&hb));
        let search = gtk::SearchEntry::builder().placeholder_text("Search nodes…").margin_top(12).margin_start(12).margin_end(12).build();
        let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results.add_css_class("boxed-list");
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&results)
            .vexpand(true)
            .margin_top(6)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bx.append(&search);
        bx.append(&scrolled);
        dlg.set_child(Some(&bx));

        let fill: Rc<dyn Fn(&str)> = {
            let me = self.clone();
            let doc = doc.clone();
            let dlg = dlg.clone();
            let results = results.clone();
            Rc::new(move |q: &str| {
                while let Some(c) = results.first_child() {
                    results.remove(&c);
                }
                let q = q.to_lowercase();
                // 4775 rows would be slow to build; show the first matches only.
                let mut shown = 0;
                for def in me.registry.borrow().nodes.values() {
                    if shown >= 60 {
                        break;
                    }
                    if !q.is_empty() && !format!("{} {}", def.display_name, def.category).to_lowercase().contains(&q) {
                        continue;
                    }
                    shown += 1;
                    let name = gtk::Label::builder().label(&def.display_name).xalign(0.0).build();
                    let cat = gtk::Label::builder().label(&def.category).xalign(0.0).build();
                    cat.add_css_class("dim-label");
                    cat.add_css_class("caption");
                    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    col.append(&name);
                    col.append(&cat);
                    let b = gtk::Button::builder().child(&col).build();
                    b.add_css_class("flat");
                    b.set_halign(gtk::Align::Fill);
                    let me2 = me.clone();
                    let doc2 = doc.clone();
                    let dlg2 = dlg.clone();
                    let ty = def.name.clone();
                    b.connect_clicked(move |_| {
                        me2.create_node(&doc2, &ty, pos);
                        dlg2.close();
                    });
                    results.append(&b);
                }
                if shown == 0 {
                    let l = gtk::Label::builder().label("No matching nodes").margin_top(18).build();
                    l.add_css_class("dim-label");
                    results.append(&l);
                }
            })
        };
        {
            let fill = fill.clone();
            search.connect_search_changed(move |e| fill(&e.text()));
        }
        fill("");
        dlg.present();
    }

    fn text_dialog(&self, title: &str, initial: &str, on_ok: impl Fn(String) + 'static) {
        let Some(win) = self.root.root().and_downcast::<gtk::Window>() else { return };
        let dlg = gtk::Window::builder()
            .title(title)
            .transient_for(&win)
            .modal(true)
            .default_width(380)
            .build();
        let hb = gtk::HeaderBar::new();
        dlg.set_titlebar(Some(&hb));
        let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("suggested-action");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::End);
        row.append(&cancel);
        row.append(&ok);
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 12);
        bx.append(&entry);
        bx.append(&row);
        bx.set_margin_top(18);
        bx.set_margin_bottom(18);
        bx.set_margin_start(18);
        bx.set_margin_end(18);
        dlg.set_child(Some(&bx));
        dlg.set_default_widget(Some(&ok));
        let d = dlg.clone();
        cancel.connect_clicked(move |_| d.close());
        let d = dlg.clone();
        let on_ok = Rc::new(on_ok);
        ok.connect_clicked(move |_| {
            on_ok(entry.text().to_string());
            d.close();
        });
        dlg.present();
    }

    /// Stop a backend we launched (Drop kills its process group). Call on window close:
    /// child widgets never get "destroy" in GTK4, and PDEATHSIG alone can't make a script
    /// blocked on a foreground command run its cleanup trap.
    pub fn stop_backend(&self) {
        self.proc.borrow_mut().take();
    }

    pub fn append_log(&self, line: &str) {
        let buf = self.log.buffer();
        buf.insert(&mut buf.end_iter(), &strip_ansi(line.trim_end_matches('\n')));
        buf.insert(&mut buf.end_iter(), "\n");
        let extra = buf.line_count() - MAX_LOG_LINES;
        if extra > 0 {
            let mut start = buf.start_iter();
            let mut cut = buf.iter_at_line(extra).unwrap_or(buf.start_iter());
            buf.delete(&mut start, &mut cut);
        }
        let mark = buf.create_mark(None, &buf.end_iter(), false);
        self.log.scroll_mark_onscreen(&mark);
        buf.delete_mark(&mark);
    }

    pub fn start(&self, cfg: backend::LaunchConfig) {
        if let Some(what) = cfg.script.as_ref().or(cfg.comfy_dir.as_ref()) {
            self.append_log(&format!("$ {} {}", what.display(), cfg.args));
            let me = self.clone();
            match backend::spawn(&cfg, move |l| me.append_log(l)) {
                Ok(p) => *self.proc.borrow_mut() = Some(p),
                Err(e) => self.append_log(&format!("[launch failed] {e}")),
            }
        }
        self.append_log(&format!("Connecting to {} …", cfg.base_url()));
        self.title.set_subtitle(&format!("Connecting to {}…", cfg.base_url()));
        self.poll_backend(cfg, 0);
    }

    /// Probe until the backend answers (a freshly launched ComfyUI takes a while).
    fn poll_backend(&self, cfg: backend::LaunchConfig, attempt: u32) {
        let me = self.clone();
        glib::spawn_future_local(async move {
            let (h, p) = (cfg.host.clone(), cfg.port);
            let res = gio::spawn_blocking(move || backend::probe(&h, p)).await.unwrap_or_else(|_| Err("probe panicked".into()));
            match res {
                Ok(status) => {
                    me.append_log(&format!("Connected: {status}"));
                    me.title.set_subtitle(&format!("Connected · {}", cfg.base_url()));
                    me.on_connected(api::Client::new(&cfg.host, cfg.port));
                }
                Err(e) => {
                    if attempt % 5 == 0 { me.append_log(&format!("Backend not reachable yet: {e}")); }
                    me.title.set_subtitle(&format!("Waiting for {}…", cfg.base_url()));
                    glib::timeout_future(Duration::from_secs(2)).await;
                    me.poll_backend(cfg, attempt + 1);
                }
            }
        });
    }
}

impl Workspace {
    /// Backend is up: load node definitions and start the event stream.
    fn on_connected(&self, client: api::Client) {
        *self.client.borrow_mut() = Some(client.clone());

        let me = self.clone();
        let c = client.clone();
        glib::spawn_future_local(async move {
            let t = std::time::Instant::now();
            match gio::spawn_blocking(move || c.get("/object_info")).await {
                Ok(Ok(v)) => {
                    let reg = registry::Registry::from_object_info(&v);
                    me.append_log(&format!(
                        "Loaded {} node types in {} categories ({} ms)",
                        reg.nodes.len(),
                        reg.by_category().len(),
                        t.elapsed().as_millis()
                    ));
                    *me.registry.borrow_mut() = reg;
                    // Titles/widget labels come from /object_info.
                    me.redraw_all();
                }
                Ok(Err(e)) => me.append_log(&format!("[object_info] {e}")),
                Err(_) => me.append_log("[object_info] worker panicked"),
            }
        });

        self.refresh_workflows();

        let (tx, rx) = async_channel::unbounded();
        client.spawn_ws(tx);
        let me = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                me.on_ws_event(ev);
            }
        });
    }

    fn on_ws_event(&self, ev: api::WsEvent) {
        use api::WsEvent::*;
        match ev {
            Connected => self.append_log("[ws] connected"),
            Disconnected(why) => self.append_log(&format!("[ws] disconnected: {why} (retrying)")),
            Status { queue_remaining } => {
                self.append_log(&format!("[ws] queue: {queue_remaining} remaining"));
                self.refresh_queue();
            }
            Progress { value, max, node } => {
                self.title.set_subtitle(&format!("Running node {} · {value}/{max}", node.unwrap_or_default()))
            }
            Executing { node: None, prompt_id } => {
                self.append_log(&format!("[ws] prompt {} finished", prompt_id.unwrap_or_default()));
                self.refresh_queue();
            }
            Executing { node: Some(n), .. } => {
                self.append_log(&format!("[ws] executing node {n}"));
                self.refresh_queue();
            }
            ExecutionStart { prompt_id } => {
                self.append_log(&format!("[ws] prompt {prompt_id} started"));
                self.refresh_queue();
            }
            ExecutionError { message } => {
                self.append_log(&format!("[ws] ERROR {message}"));
                self.refresh_queue();
            }
            Preview(bytes) => self.show_image(bytes, "live preview"),
            Executed { node, images } => {
                self.append_log(&format!("[ws] executed {}: {} images", node.unwrap_or_default(), images.len()));
                if let Some(img) = images.into_iter().next() {
                    let Some(client) = self.client.borrow().clone() else { return };
                    let me = self.clone();
                    let caption = format!("{}{}", img.subfolder, img.filename);
                    glib::spawn_future_local(async move {
                        match gio::spawn_blocking(move || client.view(&img)).await {
                            Ok(Ok(bytes)) => me.show_image(bytes, &caption),
                            Ok(Err(e)) => me.append_log(&format!("[preview] {e}")),
                            Err(_) => me.append_log("[preview] worker panicked"),
                        }
                    });
                }
            }
            // DaSiWa monitor pushes ~1 msg/s: header readout, never the log.
            Other { kind, data } if kind == "dasiwa.system_monitor" => {
                self.update_monitor(&data);
            }
            Other { kind, .. } if kind.starts_with("execution_") || kind == "progress_state" => {
                self.refresh_queue();
            }
            Other { kind, data } => {
                let d = data.to_string();
                self.append_log(&format!("[ws] {kind} {}", &d[..d.floor_char_boundary(160)]))
            }
        }
    }
}

/// Drop ANSI CSI escape sequences (`ESC [ … final-byte`), e.g. colored log levels.
// ponytail: strips colors instead of rendering them; map SGR to TextTags if wanted.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            it.next();
            for c in it.by_ref() {
                if ('@'..='~').contains(&c) { break; }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn strip_ansi() {
        assert_eq!(super::strip_ansi("\x1b[32m[INFO]\x1b[0m hi \x1b[1;31mX"), "[INFO] hi X");
    }
}
