// SPDX-License-Identifier: GPL-3.0-only
//! Main workspace shell: header bar, sidebar (workflows/queue), canvas, log panel.

use crate::{api, backend, canvas, registry};
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
        side_stack.add_titled_with_icon(
            &placeholder("folder-symbolic", "Workflows", "Saved workflows appear here (Phase 3)"),
            Some("workflows"), "Workflows", "folder-symbolic",
        );
        side_stack.add_titled_with_icon(
            &placeholder("view-list-symbolic", "Queue", "Prompt queue appears here (Phase 2)"),
            Some("queue"), "Queue", "view-list-symbolic",
        );
        let switcher = adw::ViewSwitcher::builder()
            .stack(&side_stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        let side_tb = adw::ToolbarView::new();
        side_tb.add_top_bar(&adw::HeaderBar::builder().title_widget(&switcher).show_end_title_buttons(false).build());
        side_tb.set_content(Some(&side_stack));

        // Canvas + log -----------------------------------------------------------
        let canvas = canvas::Canvas::new();
        canvas.add_demo_nodes();

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
        header.pack_start(&toggle);
        header.pack_end(&reset);
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

        Self {
            root: split.upcast(),
            title,
            log,
            proc: Rc::default(),
            client: Rc::default(),
            registry: Rc::default(),
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
            Status { queue_remaining } => self.append_log(&format!("[ws] queue: {queue_remaining} remaining")),
            Progress { value, max, node } => {
                self.title.set_subtitle(&format!("Running node {} · {value}/{max}", node.unwrap_or_default()))
            }
            Executing { node: None, prompt_id } => {
                self.append_log(&format!("[ws] prompt {} finished", prompt_id.unwrap_or_default()))
            }
            Executing { node: Some(n), .. } => self.append_log(&format!("[ws] executing node {n}")),
            ExecutionStart { prompt_id } => self.append_log(&format!("[ws] prompt {prompt_id} started")),
            ExecutionError { message } => self.append_log(&format!("[ws] ERROR {message}")),
            Preview(bytes) => self.append_log(&format!("[ws] preview frame ({} bytes)", bytes.len())),
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
