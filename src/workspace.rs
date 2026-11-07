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
    canvas: canvas::Canvas,
    nav: Rc<RefCell<Nav>>,
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
}

/// Currently displayed workflow and the subgraph path drilled into.
#[derive(Default)]
struct Nav {
    wf: Option<workflow::Workflow>,
    file: Option<String>,
    path: Vec<String>,
}

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
                    q.is_empty()
                        || row
                            .child()
                            .and_then(|c| c.downcast::<adw::ActionRow>().ok())
                            .is_some_and(|r| r.title().to_lowercase().contains(&q))
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
        let canvas = canvas::Canvas::new();

        let log = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(6).bottom_margin(6).left_margin(8).right_margin(8)
            .build();
        let log_scroller = gtk::ScrolledWindow::builder().child(&log).height_request(80).build();

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&canvas.area)
            .end_child(&log_scroller)
            .resize_end_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        paned.set_position(560);

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
        let monitor = gtk::Label::new(None);
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
        header.pack_end(&fit);
        header.pack_end(&reset);
        header.pack_end(&mon_btn);
        {
            let canvas = canvas.clone();
            fit.connect_clicked(move |_| canvas.fit_view());
        }
        {
            let canvas = canvas.clone();
            reset.connect_clicked(move |_| canvas.reset_view());
        }

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
            canvas,
            nav: Rc::default(),
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
        };
        {
            let me = ws.clone();
            run.connect_clicked(move |_| me.queue_prompt());
            let me = ws.clone();
            q_reload.connect_clicked(move |_| me.refresh_queue());
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
        let nav = self.nav.borrow();
        let Some(wf) = &nav.wf else {
            drop(nav);
            self.append_log("[run] no workflow open");
            return;
        };
        let reg = self.registry.borrow();
        if reg.nodes.is_empty() {
            drop(reg);
            self.append_log("[run] node definitions not loaded yet");
            return;
        }
        let (api_prompt, warnings) = prompt::build(wf, &reg);
        drop(reg);
        let raw = wf.raw.clone();
        let n = api_prompt.as_object().map_or(0, |o| o.len());
        drop(nav);
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
        self.canvas.connect_activate(move |id| me.open_node(id));

        let me = self.clone();
        self.queue_list.connect_map(move |_| me.refresh_queue());

        // row-selected, not row-activated: libadwaita rows don't fire activation reliably,
        // and this also gives keyboard navigation.
        let me = self.clone();
        self.wf_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let file = me.files.borrow().get(row.index() as usize).cloned();
            if let Some(f) = file {
                me.load_workflow(f);
            }
        });

        let me = self.clone();
        self.back.connect_clicked(move |_| {
            me.nav.borrow_mut().path.pop();
            me.render();
        });

        let me = self.clone();
        self.wf_list.connect_map(move |_| me.refresh_workflows());
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
            me.append_log(&format!(
                "Opened {file}: {} nodes, {} links, {} subgraphs",
                wf.root.nodes.len(),
                wf.root.links.len(),
                wf.subgraphs.len()
            ));
            *me.nav.borrow_mut() = Nav { wf: Some(wf), file: Some(file), path: Vec::new() };
            me.render();
        });
    }

    /// Double-click: drill into a subgraph instance.
    fn open_node(&self, id: &str) {
        let nav = self.nav.borrow();
        let Some(wf) = &nav.wf else { return };
        let graph = nav.path.last().and_then(|p| wf.subgraphs.get(p)).map(|s| &s.graph).unwrap_or(&wf.root);
        let Some(node) = graph.node(id) else { return };
        if !wf.subgraphs.contains_key(&node.ty) {
            return;
        }
        let ty = node.ty.clone();
        drop(nav);
        self.nav.borrow_mut().path.push(ty);
        self.render();
    }

    fn render(&self) {
        let nav = self.nav.borrow();
        let Some(wf) = &nav.wf else { return };
        let def = nav.path.last().and_then(|p| wf.subgraphs.get(p));
        let graph = def.map(|s| &s.graph).unwrap_or(&wf.root);
        let (nodes, links, groups) = graph_view::scene(wf, graph, def, &self.registry.borrow());
        // Root graph: restore the viewport the web frontend saved (`extra.ds`).
        let saved_view = (nav.path.is_empty())
            .then(|| wf.raw["extra"]["ds"].as_object())
            .flatten()
            .and_then(|ds| {
                let scale = ds.get("scale")?.as_f64()?;
                let off = ds.get("offset")?.as_array()?;
                Some((scale, (off.first()?.as_f64()?, off.get(1)?.as_f64()?)))
            });
        let mut trail = vec![nav.file.as_deref().unwrap_or_default().to_owned()];
        trail.extend(nav.path.iter().map(|p| wf.subgraphs[p].name.clone()));
        self.crumb.set_text(&trail.join(" › "));
        self.back.set_visible(!nav.path.is_empty());
        drop(nav);
        self.canvas.set_graph(nodes, links, groups);
        if let Some((zoom, off)) = saved_view {
            self.canvas.set_view(zoom, off);
        }
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
            Preview(bytes) => self.append_log(&format!("[ws] preview frame ({} bytes)", bytes.len())),
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
