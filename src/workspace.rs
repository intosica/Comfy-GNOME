// SPDX-License-Identifier: GPL-3.0-only
//! Main workspace shell: header bar, sidebar (workflows/queue), canvas, log panel.

use crate::{api, backend, canvas, graph_view, prompt, registry, workflow};
use adw::{gio, glib, gtk, prelude::*};
use serde_json::Value;
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
    crumbs: gtk::Box,
    back: gtk::Button,
    queue_list: gtk::ListBox,
    /// Serialised last `/queue` + `/history` snapshot — skip rebuilding identical lists.
    queue_key: Rc<RefCell<String>>,
    /// Finished runs, oldest first (`GET /history`) — the queue's "Completed" list.
    history: Rc<RefCell<Vec<api::HistoryEntry>>>,
    /// Queue filter: `true` = only finished runs.
    queue_done: Rc<std::cell::Cell<bool>>,
    q_clear: gtk::Button,
    /// Queue-row thumbnails, keyed by `/view` query.
    thumbs: Rc<RefCell<std::collections::HashMap<String, gtk::gdk::Texture>>>,
    /// Images the Preview tab is showing (caption, `/view` query) — what a click enlarges.
    preview_items: Rc<RefCell<Vec<(String, String)>>>,
    monitor: gtk::Label,
    monitor_detail: gtk::Label,
    monitor_btn: gtk::MenuButton,
    preview: gtk::Picture,
    preview_caption: gtk::Label,
    preview_empty: gtk::Box,
    /// Batch count: how many copies of the prompt Run queues (frontend `batchCount`).
    count: gtk::SpinButton,
    /// Workflows of the prompts we queued, by prompt id — `/history` keeps `extra_pnginfo`
    /// only for clients that sent it, so this is the reliable "open job workflow".
    job_wf: Rc<RefCell<std::collections::HashMap<String, Value>>>,
    bottom: adw::ViewStack,
    /// Tab that queued the prompt now running (receives output previews / running outline).
    run_doc: Rc<RefCell<Option<std::rc::Weak<RefCell<Doc>>>>>,
    /// Favourite workflow files (`workflows/.index.json`).
    bookmarks: Rc<RefCell<std::collections::HashSet<String>>>,
    temp_seq: Rc<std::cell::Cell<u32>>,
    /// Backend reachable (websocket up). Off → requests are skipped instead of spamming errors.
    online: Rc<std::cell::Cell<bool>>,
    /// `pysssss.SnapToGrid` + `Comfy.SnapToGrid.GridSize`, mirrored onto every canvas.
    snap: Rc<RefCell<(bool, f64)>>,
    /// ComfyUI install folder — the MTB panel browses `<it>/input` and `<it>/output`.
    root_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    /// The MTB panel, once built (it owns its own state).
    media: Rc<RefCell<Option<media::Media>>>,
    /// Bottom-dock collapse toggle, so panels can expand the dock when they need it.
    collapse: gtk::ToggleButton,
    /// `Comfy.LinkRenderMode` for every canvas (-1 hidden, 0 straight, 1 linear, 2 spline).
    link_mode: Rc<std::cell::Cell<i32>>,
    /// `Comfy.Execution.PreviewMethod` sent as `extra_data.preview_method`.
    preview_method: Rc<RefCell<String>>,
    /// Thin sampler-progress strip under the workspace tabs.
    progress: gtk::ProgressBar,
    /// Queue-page interrupt button (enabled only while something is running).
    interrupt_btn: gtk::Button,
    /// Header cancel button, the red X beside Run in the web action bar.
    stop_btn: gtk::Button,
    /// `Comfy.Workflow.Persist`: remember tabs, drafts and viewports across launches.
    persist: Rc<std::cell::Cell<bool>>,
    /// True while replaying the saved session; suppresses writes of the partial state.
    restoring: Rc<std::cell::Cell<bool>>,
    /// A save is already scheduled (coalesces canvas-change bursts).
    session_queued: Rc<std::cell::Cell<bool>>,
}

mod ere;
mod library;
mod media;
mod reaper;
mod session;
mod settings;
mod templates;
mod tabmenu;

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
    /// Not stored on the server (dropped image, duplicate): no rename/bookmark, Save = Save As.
    temp: bool,
    /// Root node id → `/view` query keys of its last execution outputs.
    outputs: std::collections::HashMap<String, Vec<String>>,
    /// Path of the graph currently drawn on this doc's canvas (drill-in / back refits on change).
    rendered: Vec<String>,
    /// Undo history: whole-workflow snapshots (viewport stripped), like the web `ChangeTracker`.
    undo: Vec<Value>,
    redo: Vec<Value>,
    /// Snapshot of the current state (what an edit is diffed against).
    snap: Value,
}

/// ponytail: full JSON snapshots, cap 50; diff-based history if huge workflows make this slow.
const UNDO_MAX: usize = 50;

/// Workflow JSON without the viewport (panning is not an edit).
fn snapshot(wf: &workflow::Workflow) -> Value {
    let mut v = wf.to_json();
    if let Some(e) = v.get_mut("extra").and_then(Value::as_object_mut) {
        e.remove("ds");
    }
    v
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
    Item { label: String, danger: bool, enabled: bool, icon: Option<&'static str>, f: Box<dyn Fn()> },
    Line,
    Swatches(Vec<(u32, Box<dyn Fn()>)>),
}

fn item(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: false, enabled: true, icon: None, f: Box::new(f) }
}

/// Menu item with an icon, optionally greyed out.
fn item_i(icon: &'static str, label: impl Into<String>, enabled: bool, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: false, enabled, icon: Some(icon), f: Box::new(f) }
}

fn danger(label: impl Into<String>, f: impl Fn() + 'static) -> Entry {
    Entry::Item { label: label.into(), danger: true, enabled: true, icon: None, f: Box::new(f) }
}

/// Plain closure action, reachable from menus as `wb.<name>`.
/// Hover previews for model files in every combo list (`canvas::widgets::edit_combo`).
/// The model index loads on first hover (worker thread); textures are cached per option.
/// ponytail: index never refreshes while connected; reconnect re-installs it.
fn install_model_previews(client: api::Client) {
    use crate::model_preview::{fetch, Index};
    use std::collections::HashMap;
    type Cb = Box<dyn FnOnce(Option<gtk::gdk::Texture>)>;
    let index: std::sync::Arc<std::sync::OnceLock<Index>> = Default::default();
    let cache: Rc<RefCell<HashMap<String, Option<gtk::gdk::Texture>>>> = Default::default();
    let pending: Rc<RefCell<HashMap<String, Vec<Cb>>>> = Default::default();
    canvas::widgets::set_preview_provider(Rc::new(move |option: &str, cb: Cb| {
        if let Some(t) = cache.borrow().get(option) {
            return cb(t.clone());
        }
        let mut p = pending.borrow_mut();
        if let Some(waiting) = p.get_mut(option) {
            return waiting.push(cb);
        }
        p.insert(option.to_owned(), vec![cb]);
        let (client, index, cache, pending, key) = (client.clone(), index.clone(), cache.clone(), pending.clone(), option.to_owned());
        glib::spawn_future_local(async move {
            let k = key.clone();
            let bytes = gio::spawn_blocking(move || {
                let ix = index.get_or_init(|| Index::load(&client));
                ix.get(&k).and_then(|e| fetch(&client, e))
            })
            .await
            .ok()
            .flatten();
            let tex = bytes.and_then(|b| gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(b)).ok());
            cache.borrow_mut().insert(key.clone(), tex.clone());
            for cb in pending.borrow_mut().remove(&key).unwrap_or_default() {
                cb(tex.clone());
            }
        });
    }));
}

/// Sum of the natural widths of the switcher's tab buttons (= every tab touching its neighbour).
fn tabs_width(w: &impl IsA<gtk::Widget>) -> f64 {
    let mut sum = 0;
    let mut stack: Vec<gtk::Widget> = w.first_child().into_iter().collect();
    while let Some(c) = stack.pop() {
        if let Some(n) = c.next_sibling() {
            stack.push(n);
        }
        if c.is::<gtk::ToggleButton>() {
            sum += c.measure(gtk::Orientation::Horizontal, -1).1;
        } else if let Some(f) = c.first_child() {
            stack.push(f);
        }
    }
    sum as f64
}

/// A few static rules on top of libadwaita (no animations, no blur: nothing that costs GPU).
fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".queue-thumb { border-radius: 6px; }
         textview.prompt-editor, textview.prompt-editor text { font-size: 11.5pt; }
         popover.prompt-popover > contents { padding: 8px; }
         box.mtb-band { background: alpha(@accent_bg_color, 0.25); border: 1px solid @accent_bg_color; border-radius: 4px; }
         box.mtb-thumb, window.mtb-thumb { border-radius: 4px; }
         box.mtb-tile { padding: 4px; border-radius: 8px; }
         box.mtb-tile.mtb-selected { background: alpha(@accent_bg_color, 0.32); box-shadow: inset 0 0 0 2px @accent_bg_color; }
         button.sidebar-reveal { background: alpha(white, 0.94); color: #2e3436; min-width: 20px; min-height: 52px; padding: 0 2px; border-radius: 0 999px 999px 0; box-shadow: 0 1px 8px alpha(black, 0.45); }
         button.sidebar-reveal:hover { background: white; }
         box.dock-bar { padding: 3px 6px; border-top: 1px solid alpha(currentColor, 0.12); background: alpha(currentColor, 0.04); }
         togglebutton.dock-tab { padding: 2px 12px; min-height: 22px; border-radius: 999px; }
         togglebutton.dock-tab:checked { background: alpha(@accent_bg_color, 0.2); color: @accent_color; }
         textview.log-view { font-size: 9.5pt; }
         label.rr-h { font-weight: bold; margin-bottom: 6px; }
         label.rr-label, label.rr-title { font-weight: bold; }
         label.rr-value { font-size: 0.9em; opacity: 0.75; }
         label.rr-note { margin-top: 6px; }
         box.rr-item { padding: 8px 10px; border-radius: 8px; background: alpha(currentColor, 0.06); border-left: 3px solid @accent_bg_color; }
         box.rr-item.rr-crit { border-left-color: @error_color; }
         box.rr-item.rr-warn { border-left-color: @warning_color; }
         box.rr-item.rr-ok { border-left-color: @success_color; }
         progressbar.rr-ok > trough > progress { background-color: @success_color; }
         progressbar.rr-warn > trough > progress { background-color: @warning_color; }
         progressbar.rr-crit > trough > progress { background-color: @error_color; }
         progressbar.sampler-progress { min-height: 3px; padding: 0; }
         progressbar.sampler-progress > trough { min-height: 3px; margin: 0; border-radius: 0; }
         progressbar.sampler-progress > trough > progress { min-height: 3px; border-radius: 0; }",
    );
    if let Some(d) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&d, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

impl Workspace {
    pub fn new() -> Self {
        load_css();
        // Sidebar ----------------------------------------------------------------
        let side_stack = adw::ViewStack::new();
        // Single, not None: GTK disables click activation entirely in NONE mode.
        let wf_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
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
        let q_clear = gtk::Button::builder().label("Clear queue").tooltip_text("Remove every pending prompt").sensitive(false).build();
        q_clear.add_css_class("flat");
        let q_reload = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Reload queue").build();
        q_reload.add_css_class("flat");
        let q_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        q_bar.set_margin_top(6);
        q_bar.set_margin_start(6);
        q_bar.set_margin_end(6);
        q_bar.append(&q_clear);
        let q_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        q_spacer.set_hexpand(true);
        q_bar.append(&q_spacer);
        q_bar.append(&q_reload);
        // Filter tabs (`reference/Queue Logic.png`): All = running/pending + completed.
        let q_all = gtk::ToggleButton::builder().label("All").active(true).build();
        let q_done = gtk::ToggleButton::builder().label("Completed").group(&q_all).build();
        let q_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        q_tabs.add_css_class("linked");
        q_tabs.set_halign(gtk::Align::Start);
        q_tabs.set_margin_start(6);
        q_tabs.set_margin_end(6);
        q_tabs.set_margin_bottom(6);
        q_tabs.append(&q_all);
        q_tabs.append(&q_done);
        let queue_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        queue_page.append(&q_bar);
        queue_page.append(&q_tabs);
        queue_page.append(&q_scroller);
        side_stack.add_titled_with_icon(&queue_page, Some("queue"), "Queue", "view-list-symbolic");
        // Nodes + MTB pages are added after the Workspace exists (they hold a clone of it).
        let side_stack_late = side_stack.clone();
        // Icon-only tabs, so the sidebar can narrow until the icons just touch.
        let switcher = gtk::Box::builder().spacing(0).halign(gtk::Align::Center).css_classes(["linked"]).build();
        let mut first: Option<gtk::ToggleButton> = None;
        for (name, icon, tip) in [
            ("workflows", "folder-symbolic", "Workflows"),
            ("queue", "view-list-symbolic", "Queue"),
            ("nodes", "node-graph-symbolic", "Nodes"),
            ("media", "folder-pictures-symbolic", "MTB"),
            ("memory", "power-profile-performance-symbolic", "Memory"),
        ] {
            let b = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(tip).build();
            match &first {
                Some(f) => b.set_group(Some(f)),
                None => {
                    b.set_active(true);
                    first = Some(b.clone());
                }
            }
            let st = side_stack.clone();
            b.connect_toggled(move |b| if b.is_active() { st.set_visible_child_name(name) });
            switcher.append(&b);
        }
        let side_tb = adw::ToolbarView::new();
        side_tb.add_top_bar(&adw::HeaderBar::builder().title_widget(&switcher).show_end_title_buttons(false).build());
        // Pages differ in minimum width; External keeps the widest from propping the sidebar open.
        side_tb.set_content(Some(&gtk::ScrolledWindow::builder().child(&side_stack).hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::Never).build()));

        // Canvas + log -----------------------------------------------------------
        let tabs = adw::TabView::new();
        tabs.set_vexpand(true);
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tabs));
        tab_bar.set_autohide(false);
        tab_bar.set_expand_tabs(false);
        let canvas_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        canvas_pane.append(&tab_bar);
        let progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::Start)
            .height_request(3)
            .hexpand(true)
            .visible(false)
            .build();
        progress.add_css_class("sampler-progress");
        canvas_pane.append(&progress);
        canvas_pane.append(&tabs);

        let log = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .cursor_visible(false)
            .css_classes(["log-view"])
            .pixels_below_lines(1)
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
        let preview_caption = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["dim-label", "caption"]).visible(false).build();
        let preview_empty = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(10).halign(gtk::Align::Center).valign(gtk::Align::Center).vexpand(true).css_classes(["dim-label"]).build();
        preview_empty.append(&gtk::Image::builder().icon_name("image-x-generic-symbolic").pixel_size(32).build());
        let hint = gtk::Box::new(gtk::Orientation::Vertical, 2);
        hint.append(&gtk::Label::builder().label("No preview yet").xalign(0.0).css_classes(["heading"]).build());
        hint.append(&gtk::Label::builder().label("Images appear here while a prompt runs").xalign(0.0).css_classes(["caption"]).build());
        preview_empty.append(&hint);
        preview.set_visible(false);
        preview.set_margin_start(8);
        preview.set_margin_end(8);
        let preview_page = gtk::Box::new(gtk::Orientation::Vertical, 4);
        preview_page.set_margin_top(6);
        preview_page.set_margin_bottom(6);
        preview_page.append(&preview_empty);
        preview_page.append(&preview);
        preview_page.append(&preview_caption);

        let bottom = adw::ViewStack::new();
        bottom.add_titled_with_icon(&log_scroller, Some("log"), "Log", "utilities-terminal-symbolic");
        bottom.add_titled_with_icon(&preview_page, Some("preview"), "Preview", "image-x-generic-symbolic");
        // Slim pill tabs (icon + label inline) instead of the tall icon-over-label switcher.
        let dock_tab = |icon: &str, label: &str| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&gtk::Image::from_icon_name(icon));
            row.append(&gtk::Label::new(Some(label)));
            gtk::ToggleButton::builder().css_classes(["flat", "dock-tab"]).child(&row).build()
        };
        let log_tab = dock_tab("utilities-terminal-symbolic", "Log");
        let prev_tab = dock_tab("image-x-generic-symbolic", "Preview");
        prev_tab.set_group(Some(&log_tab));
        log_tab.set_active(true);
        let clear_log = gtk::Button::builder().icon_name("edit-clear-all-symbolic").tooltip_text("Clear log").css_classes(["flat"]).build();
        {
            let (b, c) = (bottom.clone(), clear_log.clone());
            let (lt, pt) = (log_tab.clone(), prev_tab.clone());
            bottom.connect_visible_child_name_notify(move |s| {
                let preview = s.visible_child_name().as_deref() == Some("preview");
                (if preview { &pt } else { &lt }).set_active(true);
                c.set_visible(!preview);
                let _ = &b;
            });
            let b = bottom.clone();
            log_tab.connect_toggled(move |t| if t.is_active() { b.set_visible_child_name("log") });
            let b = bottom.clone();
            prev_tab.connect_toggled(move |t| if t.is_active() { b.set_visible_child_name("preview") });
            let log = log.clone();
            clear_log.connect_clicked(move |_| log.buffer().set_text(""));
        }
        let bottom_switcher = gtk::Box::builder().spacing(4).build();
        bottom_switcher.append(&log_tab);
        bottom_switcher.append(&prev_tab);
        // Collapse strip: hides the whole Log/Preview dock (and gives the canvas the space).
        let collapse = gtk::ToggleButton::builder()
            .icon_name("pan-down-symbolic")
            .tooltip_text("Hide Log & Preview")
            .css_classes(["flat"])
            .build();
        let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        gap.set_hexpand(true);
        let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bottom_bar.add_css_class("dock-bar");
        bottom_bar.append(&bottom_switcher);
        bottom_bar.append(&gap);
        bottom_bar.append(&clear_log);
        bottom_bar.append(&collapse);
        let bottom_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom_page.append(&bottom_bar);
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
        {
            let (bs, bp, b, paned, bottom2) = (bottom_switcher.clone(), bottom_page.clone(), collapse.clone(), paned.clone(), bottom.clone());
            let saved = Rc::new(std::cell::Cell::new(520));
            collapse.connect_toggled(move |btn| {
                let show = !btn.is_active();
                // The bar itself stays: it is the only way back (`reference/Toggle Bottom Panel.png`).
                bs.set_visible(show);
                bottom2.set_visible(show);
                bp.set_visible(true);
                if show {
                    paned.set_position(saved.get());
                    b.set_icon_name("pan-down-symbolic");
                    b.set_tooltip_text(Some("Hide Log & Preview"));
                } else {
                    let p = paned.position();
                    if p > 40 {
                        saved.set(p);
                    }
                    paned.set_position(i32::MAX / 2);
                    b.set_icon_name("pan-up-symbolic");
                    b.set_tooltip_text(Some("Show Log & Preview"));
                }
            });
        }

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
        // Web parity (`reference/Top Bar.png`): red X directly right of Run cancels the
        // current execution; insensitive when nothing is running.
        let stop = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text("Cancel current run (Ctrl+Alt+Enter)")
            .css_classes(["destructive-action"])
            .sensitive(false)
            .build();
        // Batch count sits left of Run, like the web action bar.
        let count = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
        count.set_numeric(true);
        count.set_digits(0);
        count.set_width_chars(2);
        count.set_tooltip_text(Some("Batch count — copies of this prompt queued per Run"));
        let lora = gtk::Button::builder().icon_name("application-x-addon-symbolic").tooltip_text("LoRA Manager").build();
        header.pack_start(&toggle);
        let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Leave subgraph").visible(false).build();
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let tmpl = gtk::Button::builder().icon_name("folder-templates-symbolic").tooltip_text("Templates").build();
        let gear = gtk::Button::builder().icon_name("emblem-system-symbolic").tooltip_text("Settings (Ctrl+,)").build();
        header.pack_start(&back);
        header.pack_start(&crumbs);
        header.pack_start(&tmpl);
        header.pack_start(&lora);
        header.pack_start(&gear);
        // Fixed width: the readout changes every second and would otherwise shove the
        // toolbar buttons around under the pointer.
        let monitor = gtk::Label::builder().width_chars(17).xalign(0.0).build();
        let mon_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        mon_box.append(&gtk::Image::from_icon_name("power-profile-performance-symbolic"));
        mon_box.append(&monitor);
        let mon_btn = gtk::MenuButton::builder().child(&mon_box).always_show_arrow(false).tooltip_text("Backend system monitor (DaSiWa nodes)").build();
        let mon_pop = gtk::Popover::new();
        let mon_detail = gtk::Label::builder().xalign(0.0).wrap(true).build();
        mon_pop.set_child(Some(&mon_detail));
        mon_btn.set_popover(Some(&mon_pop));
        mon_btn.set_visible(false);
        header.pack_end(&run);
        header.pack_end(&stop);
        header.pack_end(&count);
        let save = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Save Workflow (Ctrl+S)").build();
        header.pack_end(&save);
        header.pack_end(&fit);
        header.pack_end(&reset);
        header.pack_end(&mon_btn);

        let content_tb = adw::ToolbarView::new();
        content_tb.add_top_bar(&header);
        content_tb.set_content(Some(&paned));

        // Drag handle on the sidebar's edge (the web sidebar is resizable). The split view
        // only sizes by fraction within [min, max], so a drag pins both to the new width.
        let handle = gtk::Box::builder().width_request(6).cursor(&gtk::gdk::Cursor::from_name("col-resize", None).unwrap()).build();
        handle.add_css_class("sidebar-handle");
        let side_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        side_tb.set_hexpand(true);
        side_box.append(&side_tb);
        side_box.append(&handle);
        let split = adw::OverlaySplitView::builder()
            .sidebar(&side_box)
            .content(&content_tb)
            // Four stacked pages need ~270 px for the switcher to stay clickable. The
            // default max (280sp) froze the sidebar at one width; raise it so the panel
            // grows with the window and MTB/Queue/Workflows get the extra rows/columns.
            .min_sidebar_width(240.0)
            .max_sidebar_width(620.0)
            .sidebar_width_fraction(0.34)
            .build();
        toggle.bind_property("active", &split, "show-sidebar").bidirectional().sync_create().build();
        {
            let drag = gtk::GestureDrag::new();
            let (sp2, sw, tg, hd) = (split.clone(), switcher.clone(), toggle.clone(), handle.clone());
            drag.connect_drag_update(move |g, dx, _| {
                // Pointer x in split coordinates (the handle itself moves while resizing, so a
                // plain `dx` would feed back on itself).
                let Some((sx, sy)) = g.start_point() else { return };
                let Some(p) = hd.compute_point(&sp2, &gtk::graphene::Point::new((sx + dx) as f32, sy as f32)) else { return };
                let w = p.x() as f64 + 3.0;
                // Narrowest width: the switcher's tabs just touching. Dragging well past it hides the sidebar.
                let min_w = tabs_width(&sw) + 24.0;
                if w < min_w - 40.0 {
                    tg.set_active(false);
                    return;
                }
                let w = w.clamp(min_w, 900.0);
                sp2.set_min_sidebar_width(0.0); // keep min <= max while shrinking
                sp2.set_max_sidebar_width(w);
                sp2.set_min_sidebar_width(w);
            });
            handle.add_controller(drag);
        }

        // Hidden sidebar: a thin hot zone on the left edge reveals a small white arrow to bring it back.
        let shell = gtk::Overlay::new();
        shell.set_child(Some(&split));
        let reveal_btn = gtk::Button::builder().icon_name("go-next-symbolic").tooltip_text("Show Sidebar").css_classes(["sidebar-reveal"]).build();
        let reveal = gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideRight).transition_duration(120).valign(gtk::Align::Center).child(&reveal_btn).build();
        let edge = gtk::Box::builder().width_request(14).halign(gtk::Align::Start).valign(gtk::Align::Fill).margin_top(48).build();
        edge.append(&reveal);
        let motion = gtk::EventControllerMotion::new();
        let r = reveal.clone();
        motion.connect_enter(move |_, _, _| r.set_reveal_child(true));
        let r = reveal.clone();
        motion.connect_leave(move |_| r.set_reveal_child(false));
        edge.add_controller(motion);
        {
            let tg = toggle.clone();
            reveal_btn.connect_clicked(move |_| tg.set_active(true));
            let (e, r) = (edge.clone(), reveal.clone());
            split.connect_show_sidebar_notify(move |sp| {
                e.set_visible(!sp.shows_sidebar());
                r.set_reveal_child(false);
            });
            edge.set_visible(!split.shows_sidebar());
        }
        shell.add_overlay(&edge);

        let ws = Self {
            root: shell.upcast(),
            title,
            log,
            proc: Rc::default(),
            client: Rc::default(),
            registry: Rc::default(),
            tabs,
            docs: Rc::default(),
            wf_list,
            files: Rc::default(),
            crumbs,
            back,
            queue_list,
            queue_key: Rc::default(),
            history: Rc::default(),
            queue_done: Rc::default(),
            q_clear: q_clear.clone(),
            thumbs: Rc::default(),
            preview_items: Rc::default(),
            monitor,
            monitor_detail: mon_detail.clone(),
            monitor_btn: mon_btn.clone(),
            preview,
            preview_caption,
            preview_empty,
            count: count.clone(),
            job_wf: Rc::default(),
            bottom,
            run_doc: Rc::default(),
            bookmarks: Rc::default(),
            temp_seq: Rc::default(),
            online: Rc::default(),
            snap: Rc::default(),
            root_dir: Rc::default(),
            media: Rc::default(),
            collapse: collapse.clone(),
            link_mode: Rc::new(std::cell::Cell::new(2)),
            preview_method: Rc::new(RefCell::new("default".into())),
            progress: progress.clone(),
            interrupt_btn: interrupt.clone(),
            stop_btn: stop.clone(),
            persist: Rc::new(std::cell::Cell::new(true)),
            restoring: Rc::default(),
            session_queued: Rc::default(),
        };
        {
            let me = ws.clone();
            run.connect_clicked(move |_| me.queue_prompt(tabmenu::shift_down()));
            let me = ws.clone();
            save.connect_clicked(move |_| me.save_active());
            let me = ws.clone();
            tmpl.connect_clicked(move |_| me.open_templates());
            let me = ws.clone();
            gear.connect_clicked(move |_| me.open_settings());
            let me = ws.clone();
            lora.connect_clicked(move |_| me.open_lora_manager());
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
            q_reload.connect_clicked(move |_| {
                me.load_history();
                me.refresh_queue();
            });
            let me = ws.clone();
            q_clear.connect_clicked(move |_| {
                let Some(c) = me.client.borrow().clone() else { return };
                let me2 = me.clone();
                me.append_log("[queue] clear");
                glib::spawn_future_local(async move {
                    match gio::spawn_blocking(move || c.clear_queue()).await {
                        Ok(Ok(())) => me2.refresh_queue(),
                        Ok(Err(e)) => me2.append_log(&format!("[queue] {e}")),
                        Err(_) => {}
                    }
                });
            });
            q_all.add_css_class("accent");
            for (btn, done) in [(&q_all, false), (&q_done, true)] {
                let me = ws.clone();
                let (a, d) = (q_all.clone(), q_done.clone());
                btn.connect_toggled(move |b| {
                    if !b.is_active() {
                        return;
                    }
                    me.queue_done.set(done);
                    // The linked-toggle background does not always repaint on X11; make the
                    // active tab explicit instead of trusting the theme's checked style.
                    for (t, on) in [(&a, !done), (&d, done)] {
                        if on {
                            t.add_css_class("accent");
                        } else {
                            t.remove_css_class("accent");
                        }
                    }
                    me.append_log(&format!("[queue] showing {}", if done { "completed" } else { "all" }));
                    me.refresh_queue();
                });
            }
            let me = ws.clone();
            reload.connect_clicked(move |_| me.refresh_workflows());
            let me = ws.clone();
            let click = gtk::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                let items = me.preview_items.borrow().clone();
                me.show_lightbox(&items, 0);
            });
            ws.preview.add_controller(click);
            let me = ws.clone();
            interrupt.connect_clicked(move |_| me.interrupt());
            let me = ws.clone();
            stop.connect_clicked(move |_| me.interrupt());
            let me = ws.clone();
            let keys = gtk::ShortcutController::new();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>comma"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.open_settings();
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>Return"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.queue_prompt(false);
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control><Shift>Return"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.queue_prompt(true);
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control><Alt>Return"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.interrupt();
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("period"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(d) = me.active_doc() { d.borrow().canvas.fit_view(); }
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("Escape"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(d) = me.active_doc() { if d.borrow_mut().path.pop().is_some() { me.render_doc(&d); } }
                    gtk::glib::Propagation::Stop
                })),
            ));
            let me = ws.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Alt>c"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    me.with_selection(|ws, d, ids| ws.collapse(d, ids));
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
                    // While a text field has focus Ctrl+A belongs to the text, not the canvas.
                    let typing = me
                        .root
                        .root()
                        .and_downcast::<gtk::Window>()
                        .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w))
                        .is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::Entry>());
                    if !typing {
                        if let Some(d) = me.active_doc() {
                            me.select_all(&d);
                        }
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
            for (trigger, redo) in [("<Control>z", false), ("<Control><Shift>z", true), ("<Control>y", true)] {
                let me = ws.clone();
                keys.add_shortcut(gtk::Shortcut::new(
                    gtk::ShortcutTrigger::parse_string(trigger),
                    Some(gtk::CallbackAction::new(move |_, _| {
                        me.undo(redo);
                        gtk::glib::Propagation::Stop
                    })),
                ));
            }
            ws.root.add_controller(keys);
        }
        ws.setup_navigation();
        ws.setup_tabs(&tab_bar);
        {
            let me = ws.clone();
            ws.tabs.connect_selected_page_notify(move |_| {
                if !me.restoring.get() {
                    me.schedule_session();
                }
            });
        }
        // Node Library (`reference/Node Library.png`): search + category tree over `/object_info`.
        side_stack_late.add_titled_with_icon(&library::page(&ws), Some("nodes"), "Nodes", "node-graph-symbolic");
        // MTB browser (`reference/MTB.png`): input/ and output/ with subfolders and thumbnails.
        side_stack_late.add_titled_with_icon(&media::page(&ws), Some("media"), "MTB", "folder-pictures-symbolic");
        // Resource Reaper (`reference/Memory and what to use.png`): ComfyUI_Cleaner's /reaper/* routes.
        side_stack_late.add_titled_with_icon(&reaper::page(&ws), Some("memory"), "Memory", "power-profile-performance-symbolic");
        ws
    }

    /// Convert the open workflow to an API prompt and queue it.
    /// `front`: queue ahead of everything pending (frontend: shift+Run / Ctrl+Shift+Enter).
    fn queue_prompt(&self, front: bool) {
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[run] not connected");
            return;
        };
        if !self.online.get() {
            self.append_log(&format!("[run] backend {} is offline — waiting for it to come back (start ComfyUI)", client.base_url()));
            return;
        }
        let Some(doc) = self.active_doc() else {
            self.append_log("[run] no workflow open");
            return;
        };
        if self.registry.borrow().nodes.is_empty() {
            self.append_log("[run] node definitions not loaded yet");
            return;
        }
        self.sync_doc(&doc);
        // Batch count: N copies, each with its own seed bump (web `app.queuePrompt(0, batchCount)`).
        let copies = self.count.value().clamp(1.0, 100.0) as u32;
        let reg = self.registry.borrow();
        let mut jobs = Vec::new();
        for i in 0..copies {
            let (api_prompt, warnings, raw) = {
                let mut d = doc.borrow_mut();
                for line in crate::dasiwa::before_queue(&mut d.wf) {
                    self.append_log(&line);
                }
                let (p, w) = prompt::build(&d.wf, &reg);
                (p, w, d.wf.to_json())
            };
            let n = api_prompt.as_object().map_or(0, |o| o.len());
            if i == 0 {
                for w in warnings {
                    self.append_log(&format!("[run] {w}"));
                }
            }
            if n == 0 {
                self.append_log("[run] prompt is empty (all nodes muted, bypassed or unknown)");
                return;
            }
            if i == 0 {
                self.append_log(&format!("[run] queueing {n} nodes…"));
            }
            jobs.push((api_prompt, raw));
            graph_view::bump_controls(&mut doc.borrow_mut().wf, &reg);
        }
        drop(reg);
        if front {
            // "Queue front" prepends, so send our copies back to front to keep their order.
            jobs.reverse();
        }
        let me = self.clone();
        let preview = self.preview_method.borrow().clone();
        glib::spawn_future_local(async move {
            let queued = gio::spawn_blocking(move || {
                let mut out = Vec::new();
                for (api_prompt, raw) in jobs {
                    let id = client.queue_prompt(api_prompt, Some(raw.clone()), front, Some(&preview))?;
                    out.push((id, raw));
                }
                Ok::<Vec<(String, Value)>, String>(out)
            })
            .await;
            match queued {
                Ok(Ok(queued)) => {
                    // Keep the graph we sent: history only carries `extra_pnginfo` when the
                    // client put it there, and a foreign/old job may have none.
                    let ids: Vec<String> = queued.iter().map(|(id, _)| id.clone()).collect();
                    {
                        let mut cache = me.job_wf.borrow_mut();
                        if cache.len() > 200 {
                            cache.clear(); // ponytail: drop the lot, they are pruned by history anyway
                        }
                        cache.extend(queued);
                    }
                    me.append_log(&format!("[run] queued {} prompt(s): {}", ids.len(), ids.join(", ")));
                    me.stop_btn.set_sensitive(true);
                    me.interrupt_btn.set_sensitive(true);
                    *me.run_doc.borrow_mut() = Some(Rc::downgrade(&doc));
                    // Like the web frontend: step seeds (control_after_generate) once queued.
                    me.edited(&doc);
                }
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
        let wf_list = self.wf_list.clone();
        self.tabs.connect_page_detached(move |_, page, _| {
            docs.borrow_mut().retain(|d| &d.borrow().page != page);
            // Clearing the selection is what lets a single click re-open the file:
            // row-selected only fires when the selection actually changes.
            wf_list.unselect_all();
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

    /// Expand/collapse the Log & Preview dock (the strip toggle keeps working either way).
    pub(super) fn set_panel_collapsed(&self, collapsed: bool) {
        self.collapse.set_active(collapsed);
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
                // Persist the viewport like the web frontend (`extra.ds`): litegraph draws
                // `(pos + offset) * scale`, ours is `pos * zoom + off`.
                let ds = serde_json::json!({"scale": zoom, "offset": [off.0 / zoom, off.1 / zoom]});
                if let Some(extra) = d.wf.raw.get_mut("extra").and_then(|e| e.as_object_mut()) {
                    extra.insert("ds".into(), ds);
                }
            }
            Some(p) => {
                if let Some(sg) = d.wf.subgraphs.get_mut(p) {
                    graph_view::sync(&mut sg.graph, &nodes, &links, &groups);
                }
                // A wire dropped on the Inputs node's trailing socket becomes a new input;
                // one cut again takes the input (and the host node's socket) with it.
                graph_view::grow_inputs(&mut d.wf, p);
                if let Some(sg) = d.wf.subgraphs.get_mut(p) {
                    graph_view::sync_io(sg);
                }
                graph_view::prune_inputs(&mut d.wf, p);
            }
        }
    }

    fn set_dirty(&self, doc: &DocRef, dirty: bool) {
        if dirty {
            self.checkpoint(doc);
            self.schedule_session(); // every edit path lands here, widget edits included
        }
        let mut d = doc.borrow_mut();
        if d.dirty == dirty {
            return;
        }
        d.dirty = dirty;
        tabmenu::retitle(&d);
    }

    /// Record an undo step if the workflow changed since the last snapshot.
    /// Every edit path ends in `set_dirty(doc, true)`, so this is the single hook.
    fn checkpoint(&self, doc: &DocRef) {
        self.sync_doc(doc);
        let mut d = doc.borrow_mut();
        let now = snapshot(&d.wf);
        if now == d.snap {
            return;
        }
        let prev = std::mem::replace(&mut d.snap, now);
        d.undo.push(prev);
        if d.undo.len() > UNDO_MAX {
            d.undo.remove(0);
        }
        d.redo.clear();
    }

    /// Ctrl+Z (`redo == false`) / Ctrl+Shift+Z, Ctrl+Y.
    fn undo(&self, redo: bool) {
        let Some(doc) = self.active_doc() else { return };
        self.checkpoint(&doc); // pending canvas changes become a step first
        {
            let mut d = doc.borrow_mut();
            let Some(mut target) = (if redo { d.redo.pop() } else { d.undo.pop() }) else {
                drop(d);
                self.append_log(if redo { "[edit] nothing to redo" } else { "[edit] nothing to undo" });
                return;
            };
            // Keep the current viewport.
            if let Some(ds) = d.wf.raw.get("extra").and_then(|e| e.get("ds")).cloned() {
                if let Some(e) = target.get_mut("extra").and_then(Value::as_object_mut) {
                    e.insert("ds".into(), ds);
                }
            }
            let Ok(wf) = workflow::Workflow::from_json(target) else { return };
            let cur = std::mem::replace(&mut d.snap, snapshot(&wf));
            if redo { d.undo.push(cur) } else { d.redo.push(cur) }
            d.wf = wf;
            // Drop subgraph levels that no longer exist.
            let keep = d.path.iter().take_while(|p| d.wf.subgraphs.contains_key(*p)).count();
            d.path.truncate(keep);
            d.dirty = true;
            tabmenu::retitle(&d);
        }
        self.refresh_doc(&doc);
    }

    fn save_active(&self) {
        match self.active_doc() {
            Some(doc) => self.save_doc(&doc, None),
            None => self.append_log("[save] no workflow open"),
        }
    }

    /// `GET /queue` → rebuild the sidebar list (cheap; called on every WS status event).
    fn refresh_queue(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        if !self.online.get() { return }
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
        let done_only = self.queue_done.get();
        let hist = self.history.borrow();
        let key = format!("{q:?}|{done_only}|{}", hist.len());
        if *self.queue_key.borrow() == key {
            return;
        }
        *self.queue_key.borrow_mut() = key;
        while let Some(row) = self.queue_list.row_at_index(0) {
            self.queue_list.remove(&row);
        }
        self.q_clear.set_sensitive(!q.pending.is_empty());
        let busy = !q.running.is_empty();
        self.stop_btn.set_sensitive(busy);
        self.interrupt_btn.set_sensitive(busy);
        let client = self.client.borrow().clone();
        if !done_only {
            // `once` here used to cap the list at one running + one pending row, which is why
            // only two queued jobs ever showed up.
            for (running, item) in q.running.iter().map(|i| (true, i)).chain(q.pending.iter().map(|i| (false, i))) {
                let row = adw::ActionRow::builder()
                    .title(format!("{} #{}", if running { "Running" } else { "Pending" }, item.number))
                    .subtitle(format!(
                        "{} · {} nodes · {}",
                        &item.prompt_id[..item.prompt_id.len().min(8)],
                        item.nodes,
                        item.classes
                    ))
                    .activatable(true)
                    .tooltip_text("Open workflow in a new tab")
                    .build();
                let (me, id) = (self.clone(), item.prompt_id.clone());
                row.connect_activated(move |_| me.open_job_workflow(&id));
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
                let (me2, id2) = (self.clone(), item.prompt_id.clone());
                let more = gtk::Button::builder()
                    .icon_name("view-more-symbolic")
                    .tooltip_text("Job actions")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                more.connect_clicked(move |b| {
                    let f = {
                        let (me3, id3) = (me2.clone(), id2.clone());
                        move || me3.open_job_workflow(&id3)
                    };
                    let entries = vec![
                        item_i("document-open-symbolic", "Open Workflow in New Tab", true, f),
                        Entry::Line,
                        if running {
                            danger("Interrupt", { let me3 = me2.clone(); move || me3.interrupt() })
                        } else {
                            let (me3, id3) = (me2.clone(), id2.clone());
                            danger("Remove from queue", move || me3.drop_queued(&id3))
                        },
                    ];
                    let (w, h) = (b.width() as f64, b.height() as f64);
                    me2.show_menu(b, w, h, entries);
                });
                row.add_suffix(&more);
                self.queue_list.append(&row);
            }
        }
        let live = !done_only && !(q.running.is_empty() && q.pending.is_empty());
        if !live && hist.is_empty() {
            let msg = if done_only { "No finished runs yet" } else { "Queue is empty" };
            let row = adw::ActionRow::builder().title(msg).css_classes(["dim-label"]).build();
            self.queue_list.append(&row);
        }
        // Finished runs, newest first, grouped by day like the web UI's history list.
        let mut day = String::new();
        for run in hist.iter().rev() {
            let label = match run.finished {
                Some(ms) => day_label(ms),
                None => String::new(),
            };
            if !label.is_empty() && label != day {
                day = label;
                let head = gtk::Label::builder()
                    .label(&day)
                    .halign(gtk::Align::Start)
                    .margin_start(12)
                    .margin_top(10)
                    .margin_bottom(2)
                    .css_classes(["dim-label", "caption"])
                    .build();
                self.queue_list.append(&head);
            }
            // Runs without image outputs still list: their workflow can be reopened.
            let first = run.images.first();
            let dur = match run.duration() {
                Some(s) => format!("{s:.2}s"),
                None => "finished".to_string(),
            };
            let count = match run.images.len() {
                0 | 1 => dur,
                n => format!("{dur} · {n} images"),
            };
            let row = adw::ActionRow::builder()
                .title(first.map_or_else(|| format!("Job {}", &run.prompt_id[..run.prompt_id.len().min(8)]), |f| f.filename.clone()))
                .subtitle(count)
                .title_lines(1)
                .subtitle_lines(1)
                .activatable(true)
                .tooltip_text(if first.is_some() { "Open at full size" } else { "Open workflow in a new tab" })
                .build();
            let thumb = gtk::Image::from_paintable(None::<&gtk::gdk::Texture>);
            let q = first.map(api::OutputImage::query).unwrap_or_default();
            thumb.set_pixel_size(44);
            thumb.set_css_classes(&["card", "queue-thumb"]);
            if first.is_none() {
                thumb.set_icon_name(Some("document-open-symbolic"));
                thumb.set_pixel_size(20);
                thumb.set_size_request(44, 44);
            }
            match self.thumbs.borrow().get(&q) {
                Some(t) => thumb.set_paintable(Some(t)),
                None if q.is_empty() => {}
                None => {
                    if let Some(client) = client.clone() {
                        let (me, pic, key) = (self.clone(), thumb.clone(), q.clone());
                        glib::spawn_future_local(async move {
                            let res = gio::spawn_blocking(move || client.view_query(&key)).await;
                            if let Ok(Ok(bytes)) = res {
                                if let Ok(t) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                                    me.thumbs.borrow_mut().insert(q, t.clone());
                                    pic.set_paintable(Some(&t));
                                }
                            }
                        });
                    }
                }
            }
            row.add_prefix(&thumb);
            let items: Vec<(String, String)> =
                run.images.iter().map(|i| (i.filename.clone(), i.query())).collect();
            let (me, act, pid) = (self.clone(), items.clone(), run.prompt_id.clone());
            row.connect_activated(move |_| {
                if act.is_empty() { me.open_job_workflow(&pid) } else { me.show_lightbox(&act, 0) }
            });
            let (me2, id2, imgs) = (self.clone(), run.prompt_id.clone(), items.clone());
            let more = gtk::Button::builder()
                .icon_name("view-more-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            more.connect_clicked(move |b| {
                let mut entries = vec![item_i("document-open-symbolic", "Open Workflow in New Tab", true, {
                    let (me3, id3) = (me2.clone(), id2.clone());
                    move || me3.open_job_workflow(&id3)
                })];
                if !imgs.is_empty() {
                    let (me3, imgs3) = (me2.clone(), imgs.clone());
                    entries.push(item_i("zoom-in-symbolic", "Open Image at Full Size", true, move || {
                        me3.show_lightbox(&imgs3, 0)
                    }));
                }
                let (w, h) = (b.width() as f64, b.height() as f64);
                me2.show_menu(b, w, h, entries);
            });
            row.add_suffix(&more);
            self.queue_list.append(&row);
        }
    }

    /// Open the graph a queued (running or pending) or finished prompt was made from,
    /// in a new temporary tab — the web UI's "Open Workflow in New Tab".
    fn open_job_workflow(&self, prompt_id: &str) {
        let label = prompt_id[..prompt_id.len().min(8)].to_string();
        // Our own jobs first: the graph we queued is exactly what was run.
        let cached = self.job_wf.borrow().get(prompt_id).cloned();
        if let Some(v) = cached {
            match workflow::Workflow::from_json(v) {
                Ok(wf) => {
                    self.open_temp(&format!("Job {label}"), wf);
                    self.append_log(&format!("[queue] opened the workflow of job {label}"));
                }
                Err(e) => self.append_log(&format!("[queue] job {label}: {e}")),
            }
            return;
        }
        let Some(client) = self.client.borrow().clone() else { return };
        let (me, id) = (self.clone(), prompt_id.to_string());
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || client.job_workflow(&id)).await;
            match res {
                Ok(Ok(Some(v))) => match workflow::Workflow::from_json(v) {
                    Ok(wf) => {
                        me.open_temp(&format!("Job {label}"), wf);
                        me.append_log(&format!("[queue] opened the workflow of job {label}"));
                    }
                    Err(e) => me.append_log(&format!("[queue] job {label}: {e}")),
                },
                Ok(Ok(None)) => me.append_log(&format!("[queue] job {label} carries no embedded workflow (queued by another client)")),
                Ok(Err(e)) => me.append_log(&format!("[queue] {e}")),
                Err(_) => {}
            }
        });
    }

    /// Drop one pending prompt from the queue.
    fn drop_queued(&self, id: &str) {
        let Some(client) = self.client.borrow().clone() else { return };
        let (me, id) = (self.clone(), id.to_string());
        me.append_log("[queue] remove");
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.delete_queued(&[id])).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => me.append_log(&format!("[queue] {e}")),
                Err(_) => {}
            }
        });
    }

    /// Pull the last finished runs from `/history` (they survive an app restart).
    fn load_history(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.history(100)).await {
                Ok(Ok(list)) => {
                    *me.history.borrow_mut() = list;
                    me.refresh_queue();
                }
                Ok(Err(e)) => me.append_log(&format!("[history] {e}")),
                Err(_) => {}
            }
        });
    }

    /// A prompt just finished: record it in the completed list. The server may write the
    /// history entry a moment after the websocket event, so a miss re-reads the whole list.
    fn note_finished(&self, prompt_id: Option<String>) {
        let Some(client) = self.client.borrow().clone() else { return };
        let id = prompt_id.unwrap_or_default();
        let me = self.clone();
        glib::spawn_future_local(async move {
            if id.is_empty() {
                me.load_history();
                return;
            }
            match gio::spawn_blocking(move || client.history_entry(&id)).await {
                Ok(Ok(Some(e))) => {
                    {
                        let mut h = me.history.borrow_mut();
                        if !h.iter().any(|x| x.prompt_id == e.prompt_id) {
                            h.push(e);
                        }
                    }
                    me.refresh_queue();
                }
                Ok(Ok(None)) => {
                    let me2 = me.clone();
                    glib::timeout_add_local_once(Duration::from_millis(500), move || me2.load_history());
                }
                Ok(Err(e)) => me.append_log(&format!("[history] {e}")),
                Err(_) => {}
            }
        });
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
                self.preview_caption.set_visible(true);
                self.preview.set_visible(true);
                self.preview_empty.set_visible(false);
                self.bottom.set_visible_child_name("preview");
            }
            Err(e) => self.append_log(&format!("[preview] {caption}: {e}")),
        }
    }

    /// Paint a live sampler frame on the node the backend says is displaying it.
    /// `display_node_id` already resolves ephemeral/subgraph workers to their host.
    fn show_live_preview(&self, node: Option<&str>, bytes: &[u8]) {
        let Some(id) = node else { return };
        let Some(doc) = self.run_doc.borrow().as_ref().and_then(|w| w.upgrade()) else { return };
        let key = format!("live:{id}");
        let canvas = doc.borrow().canvas.clone();
        canvas.put_image(&key, bytes);
        canvas.set_node_images(id, vec![key]);
    }

    /// Full-size image over the dimmed workspace (`reference/Image Preview.png`): the picture
    /// keeps its own resolution, floats at 90 % of the window and leaves the chrome visible.
    fn show_lightbox(&self, items: &[(String, String)], start: usize) {
        let Some(client) = self.client.borrow().clone() else { return };
        if items.is_empty() {
            return;
        }
        let win = self.root.root().and_downcast::<gtk::Window>();
        let (w, h) = win.as_ref().map(|w| (w.width(), w.height())).unwrap_or((1280, 800));
        let dlg = adw::Dialog::builder()
            .content_width((w * 9 / 10).clamp(480, 2400))
            .content_height((h * 9 / 10).clamp(360, 1500))
            .build();

        let items = Rc::new(items.to_vec());
        let at = Rc::new(std::cell::Cell::new(start.min(items.len() - 1)));
        let pic = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let caption = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(14)
            .css_classes(["osd", "caption"])
            .build();

        // `gen` drops a decode that finished after the user already flipped to the next image.
        let gen = Rc::new(std::cell::Cell::new(0u32));
        let load: Rc<dyn Fn()> = {
            let (client, pic, caption, gen, items, at) =
                (client.clone(), pic.clone(), caption.clone(), gen.clone(), items.clone(), at.clone());
            Rc::new(move || {
                let n = gen.get().wrapping_add(1);
                gen.set(n);
                let i = at.get();
                let (name, query) = items[i].clone();
                caption.set_text(&format!("{name}  ·  {}/{}", i + 1, items.len()));
                let (client, pic, caption, gen) = (client.clone(), pic.clone(), caption.clone(), gen.clone());
                glib::spawn_future_local(async move {
                    match gio::spawn_blocking(move || client.view_query(&query)).await {
                        Ok(Ok(bytes)) => {
                            if gen.get() != n {
                                return;
                            }
                            match gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                                Ok(tex) => pic.set_paintable(Some(&tex)),
                                Err(e) => caption.set_text(&format!("{name}: {e}")),
                            }
                        }
                        Ok(Err(e)) => {
                            if gen.get() == n {
                                caption.set_text(&format!("{name}: {e}"));
                            }
                        }
                        Err(_) => {}
                    }
                });
            })
        };

        let step: Rc<dyn Fn(i32)> = {
            let (items, at, load) = (items.clone(), at.clone(), load.clone());
            Rc::new(move |d: i32| {
                let n = items.len() as i32;
                at.set(((at.get() as i32 + d).rem_euclid(n)) as usize);
                load();
            })
        };

        let overlay = gtk::Overlay::builder().child(&pic).build();
        let nav = |icon: &str, halign: gtk::Align, d: i32, tip: &str| {
            let b = gtk::Button::builder()
                .icon_name(icon)
                .halign(halign)
                .valign(gtk::Align::Center)
                .margin_start(10)
                .margin_end(10)
                .tooltip_text(tip)
                .css_classes(["osd", "circular"])
                .build();
            let step = step.clone();
            b.connect_clicked(move |_| step(d));
            b
        };
        let prev = nav("go-previous-symbolic", gtk::Align::Start, -1, "Previous image");
        let next = nav("go-next-symbolic", gtk::Align::End, 1, "Next image");
        overlay.add_overlay(&prev);
        overlay.add_overlay(&next);
        overlay.add_overlay(&caption);
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(8)
            .margin_end(8)
            .tooltip_text("Close")
            .css_classes(["osd", "circular"])
            .build();
        {
            let dlg = dlg.clone();
            close.connect_clicked(move |_| {
                dlg.close();
            });
        }
        overlay.add_overlay(&close);
        for b in [&prev, &next] {
            b.set_visible(items.len() > 1);
        }

        let keys = gtk::EventControllerKey::new();
        {
            let (step, dlg) = (step.clone(), dlg.clone());
            keys.connect_key_pressed(move |_, key, _, _| match key {
                gtk::gdk::Key::Left | gtk::gdk::Key::Page_Up => {
                    step(-1);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Right | gtk::gdk::Key::Page_Down => {
                    step(1);
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Escape => {
                    dlg.close();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            });
        }
        dlg.add_controller(keys);
        dlg.set_child(Some(&overlay));
        dlg.present(win.as_ref());
        load();
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
        if !self.online.get() { return }
        let me = self.clone();
        glib::spawn_future_local(async move {
            // Bookmarks come from workflows/.index.json; read once (later edits are local).
            let first = me.files.borrow().is_empty();
            let c2 = client.clone();
            match gio::spawn_blocking(move || {
                let favs = if first {
                    c2.workflow_json(".index.json").ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).map(|v| {
                        v["favorites"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_owned)).collect::<Vec<_>>()
                    })
                } else {
                    None
                };
                client.workflows().map(|f| (f, favs))
            })
            .await
            {
                Ok(Ok((files, favs))) => {
                    if let Some(f) = favs {
                        *me.bookmarks.borrow_mut() = f.into_iter().collect();
                    }
                    me.set_workflow_list(files)
                }
                Ok(Err(e)) => me.append_log(&format!("[workflows] {e}")),
                Err(_) => me.append_log("[workflows] worker panicked"),
            }
        });
    }

    fn set_workflow_list(&self, mut files: Vec<String>) {
        files.retain(|f| f.ends_with(".json") && !f.starts_with('.') && !f.contains("/."));
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
            if self.bookmarks.borrow().contains(f) {
                row.add_prefix(&gtk::Image::from_icon_name("starred-symbolic"));
            }
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
        self.open_doc(file, wf, false);
    }

    fn open_doc(&self, file: String, wf: workflow::Workflow, temp: bool) {
        if let Some(doc) = self.docs.borrow().iter().find(|d| d.borrow().file == file).cloned() {
            let page = doc.borrow().page.clone();
            self.tabs.set_selected_page(&page);
            return;
        }
        let canvas = canvas::Canvas::new();
        if let Some(c) = self.client.borrow().as_ref() {
            canvas.set_api_base(c.base_url());
        }
        // Drop an image or workflow file straight onto the canvas at the pointer.
        let drop = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        let (me2, c2) = (self.clone(), canvas.clone());
        drop.connect_drop(move |_, v, x, y| {
            let Ok(list) = v.get::<gtk::gdk::FileList>() else { return false };
            let (zoom, off) = c2.view();
            let pos = ((x - off.0) / zoom, (y - off.1) / zoom);
            for f in list.files() {
                me2.open_dropped_at(&f, pos);
            }
            true
        });
        canvas.area.add_controller(drop);
        let page = self.tabs.append(&canvas.area);
        page.set_title(&tabmenu::stem(&file));
        page.set_tooltip(if temp { "Unsaved (temporary) workflow" } else { &file });
        let doc: DocRef = Rc::new(RefCell::new(Doc {
            file,
            wf,
            path: Vec::new(),
            canvas: canvas.clone(),
            page: page.clone(),
            // Unedited docs (temp or not) close silently, like upstream `isModified`.
            dirty: false,
            temp,
            outputs: Default::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            rendered: Vec::new(),
            snap: Value::Null,
        }));
        {
            let mut d = doc.borrow_mut();
            d.snap = snapshot(&d.wf);
        }
        {
            let (always, grid) = *self.snap.borrow();
            canvas.set_snap(always, grid);
            canvas.set_link_mode(self.link_mode.get());
        }
        self.docs.borrow_mut().push(doc.clone());
        {
            // Double-click: subgraph instance → drill in; empty space → Add Node search.
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_activate(move |id, pos| {
                if let Some(d) = weak.upgrade() {
                    if id.is_empty() {
                        me.add_node_dialog(&d, pos, None);
                    } else {
                        me.open_node(&d, id);
                    }
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
            // Widget edits go straight into the model.
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_widget(move |id, key, v, structural| {
                if let Some(d) = weak.upgrade() {
                    me.on_widget(&d, id, key, v, structural);
                }
            });
            // A wire dropped on empty canvas: same picker as double-click, filtered to
            // nodes with a compatible socket, and connected when picked.
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_link_menu(move |id, slot, out, ty, pos| {
                if let Some(d) = weak.upgrade() {
                    me.add_node_dialog(&d, pos, Some((id.to_owned(), slot, out, ty.to_owned())));
                }
            });
            // Any model change marks the tab dirty (and keeps the model current).
            let me = self.clone();
            let weak = Rc::downgrade(&doc);
            canvas.connect_change(move || {
                if let Some(d) = weak.upgrade() {
                    let inputs = |d: &DocRef| {
                        let d = d.borrow();
                        d.path.last().and_then(|p| d.wf.subgraphs.get(p)).map(|s| s.inputs.len()).unwrap_or(0)
                    };
                    let before = inputs(&d);
                    me.set_dirty(&d, true); // sync may grow a subgraph's inputs
                    if inputs(&d) != before {
                        me.render_doc(&d); // show the new socket and move the add-slot down
                    }
                }
            });
        }
        self.render_doc(&doc);
        self.tabs.set_selected_page(&page);
        self.schedule_session();
    }

    /// A canvas widget changed (or a button was pressed): write it to the model.
    fn on_widget(&self, doc: &DocRef, id: &str, key: &str, v: serde_json::Value, structural: bool) {
        if let Some(i) = key.strip_prefix("upload:").and_then(|i| i.parse::<usize>().ok()) {
            self.upload_image(doc, id, i.saturating_sub(1));
            return;
        }
        if key == "ere" {
            return self.on_ere(doc, id, v);
        }
        if structural {
            self.sync_doc(doc); // re-render rebuilds from the model: keep moves and wires
        }
        let first = self.registry.borrow().get("LoraLoader").and_then(|d| d.inputs.iter().find(|i| i.name == "lora_name")).and_then(|i| match &i.kind {
            registry::Kind::Combo(o) => o.first().cloned(),
            _ => None,
        });
        let ok = {
            let mut d = doc.borrow_mut();
            let path = d.path.clone();
            graph_view::apply_widget(&mut d.wf, &path, id, key, v, &first.unwrap_or_default())
        };
        if !ok {
            return;
        }
        if structural {
            self.edited(doc);
        } else {
            self.set_dirty(doc, true);
        }
    }

    /// Pick a local image, upload it to ComfyUI's input folder and point `widgets_values[idx]` at it.
    fn upload_image(&self, doc: &DocRef, id: &str, idx: usize) {        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[upload] not connected");
            return;
        };
        let (me, doc, id) = (self.clone(), doc.clone(), id.to_owned());
        let dialog = gtk::FileDialog::builder().title("Choose an image to upload").build();
        let parent = self.tabs.root().and_downcast::<gtk::Window>();
        dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
            let Some(path) = res.ok().and_then(|f| f.path()) else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            glib::spawn_future_local(async move {
                let up = gio::spawn_blocking(move || -> Result<String, String> {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    client.upload_image(&name, &bytes)
                })
                .await;
                match up {
                    Ok(Ok(value)) => {
                        me.append_log(&format!("[upload] {value}"));
                        me.on_widget(&doc, &id, &idx.to_string(), serde_json::Value::from(value), true);
                    }
                    Ok(Err(e)) => me.append_log(&format!("[upload] {e}")),
                    Err(_) => me.append_log("[upload] worker panicked"),
                }
            });
        });
    }

    /// The active tab, or a fresh empty workflow when nothing is open — dropping a picture or
    /// picking a node from the library should not need a pre-opened file.
    fn doc_or_blank(&self, name: &str) -> Option<DocRef> {
        if let Some(d) = self.active_doc() {
            return Some(d);
        }
        let blank = serde_json::json!({"nodes": [], "links": [], "groups": []});
        match workflow::Workflow::from_json(blank) {
            Ok(wf) => self.open_temp(name, wf),
            Err(e) => {
                self.append_log(&format!("[workflow] {e}"));
                return None;
            }
        }
        self.active_doc()
    }

    /// MTB / file-manager drop of an image without ComfyUI metadata: upload it and drop a
    /// preloaded native `LoadImage` node at `pos` (`reference/MTB.png`).
    fn insert_image_node_at(&self, path: &std::path::Path, pos: (f64, f64)) {
        let Some(client) = self.client.borrow().clone() else {
            return self.append_log("[drop] not connected");
        };
        let Some(doc) = self.doc_or_blank("dropped") else { return };
        let Ok(bytes) = std::fs::read(path) else {
            return self.append_log(&format!("[drop] cannot read {}", path.display()));
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.png".into());
        let me = self.clone();
        glib::spawn_future_local(async move {
            let up = gio::spawn_blocking(move || client.upload_image(&name, &bytes)).await;
            match up {
                Ok(Ok(stored)) => {
                    me.append_log(&format!("[drop] uploaded {stored} → Load Image"));
                    me.create_node(&doc, "LoadImage", pos);
                    let id = doc.borrow().canvas.selected().first().cloned();
                    if let Some(id) = id {
                        me.on_widget(&doc, &id, "0", Value::from(stored), true);
                    }
                }
                Ok(Err(e)) => me.append_log(&format!("[drop] {e}")),
                Err(_) => me.append_log("[drop] upload worker panicked"),
            }
        });
    }

    /// Fetch previews the canvas wants but has not got (LoadImage thumbnails).
    fn load_images(&self, doc: &DocRef) {
        let Some(client) = self.client.borrow().clone() else { return };
        let canvas = doc.borrow().canvas.clone();
        for key in canvas.missing_images() {
            let (client, canvas) = (client.clone(), canvas.clone());
            glib::spawn_future_local(async move {
                let q = key.clone();
                if let Ok(Ok(bytes)) = gio::spawn_blocking(move || client.view_query(&q)).await {
                    canvas.put_image(&key, &bytes);
                }
            });
        }
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

    /// Breadcrumb: workflow name, then one clickable crumb per open subgraph. Clicking an
    /// ancestor leaves the subgraphs below it; the active crumb opens the subgraph menu.
    fn show_trail(&self, doc: &DocRef) {
        while let Some(c) = self.crumbs.first_child() {
            self.crumbs.remove(&c);
        }
        let (file, path, names): (String, Vec<String>, Vec<String>) = {
            let d = doc.borrow();
            (d.file.clone(), d.path.clone(), d.path.iter().map(|p| d.wf.subgraphs[p].name.clone()).collect())
        };
        let home = gtk::Button::builder()
            .label(tabmenu::stem(&file))
            .tooltip_text(&file)
            .css_classes(["flat"])
            .build();
        // Long names must not squeeze the sidebar / window title out of the header.
        if let Some(l) = home.child().and_downcast::<gtk::Label>() {
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            l.set_max_width_chars(18);
        }
        home.connect_clicked({
            let (me, d) = (self.clone(), doc.clone());
            move |_| {
                d.borrow_mut().path.clear();
                me.render_doc(&d);
            }
        });
        self.crumbs.append(&home);
        for (i, name) in names.iter().enumerate() {
            let sep = gtk::Label::new(Some("›"));
            sep.add_css_class("dim-label");
            self.crumbs.append(&sep);
            let last = i + 1 == names.len();
            let b = gtk::Button::builder().label(if last { format!("{name}  ▾") } else { name.clone() }).css_classes(["flat"]).build();
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                l.set_max_width_chars(if last { 30 } else { 16 });
            }
            if last {
                let (me, d, id) = (self.clone(), doc.clone(), path[i].clone());
                let bx = b.clone();
                b.connect_clicked(move |_| {
                    let (x, y) = (bx.width() as f64 / 2.0, bx.height() as f64);
                    let (me, d, id) = (me.clone(), d.clone(), id.clone());
                    me.show_menu(&bx, x, y, vec![
                        Entry::Item { label: "Rename".into(), danger: false, enabled: true, icon: Some("document-edit-symbolic"), f: Box::new({ let (me, d, id) = (me.clone(), d.clone(), id.clone()); move || me.rename_subgraph(&d, &id) }) },
                        Entry::Line,
                        Entry::Item { label: "Clear Workflow".into(), danger: true, enabled: true, icon: Some("edit-clear-all-symbolic"), f: Box::new({ let (me, d) = (me.clone(), d.clone()); move || me.clear_workflow(&d) }) },
                    ]);
                });
            } else {
                let (me, d, n) = (self.clone(), doc.clone(), i + 1);
                b.connect_clicked(move |_| {
                    d.borrow_mut().path.truncate(n);
                    me.render_doc(&d);
                });
            }
            self.crumbs.append(&b);
        }
        self.back.set_visible(!path.is_empty());
    }

    /// Rename the subgraph definition and every host node that instantiates it, like the web UI
    /// (`forEachSubgraphNode(node => node.title = title)`).
    fn rename_subgraph(&self, doc: &DocRef, id: &str) {
        let Some(cur) = doc.borrow().wf.subgraphs.get(id).map(|s| s.name.clone()) else { return };
        let (me, d, id) = (self.clone(), doc.clone(), id.to_owned());
        self.name_dialog("Rename Subgraph", "Rename", &cur, move |name| {
            {
                let mut dm = d.borrow_mut();
                let wf = &mut dm.wf;
                if let Some(sg) = wf.subgraphs.get_mut(&id) {
                    sg.name = name.clone();
                }
                let graphs = std::iter::once(&mut wf.root).chain(wf.subgraphs.values_mut().map(|s| &mut s.graph));
                for g in graphs {
                    for n in g.nodes.iter_mut().filter(|n| n.ty == id) {
                        n.title = Some(name.clone());
                    }
                }
            }
            me.render_doc(&d);
            me.set_dirty(&d, true);
        });
    }

    fn render_doc(&self, doc: &DocRef) {
        let depth_changed = {
            let mut d = doc.borrow_mut();
            let changed = d.rendered != d.path;
            d.rendered = d.path.clone();
            changed
        };
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
                    // litegraph offset is in canvas units (see sync_doc).
                    Some((scale, (off.first()?.as_f64()? * scale, off.get(1)?.as_f64()? * scale)))
                });
            (d.canvas.clone(), nodes, links, groups, saved_view)
        };
        self.show_trail(doc);
        // A re-render (widget edit, new socket, subgraph growth…) must not yank the view;
        // only the first draw of a freshly opened tab takes the file's saved viewport.
        let first = depth_changed || canvas.scene().0.is_empty();
        let view = canvas.view();
        canvas.set_graph(nodes, links, groups);
        match (first, saved_view) {
            (true, Some((zoom, off))) => canvas.set_view(zoom, off), // opened tab: file viewport
            (true, None) => {}                                        // new depth: keep the auto-fit
            (false, _) => canvas.set_view(view.0, view.1),             // edit: keep what you see
        }
        {
            let d = doc.borrow();
            if d.path.is_empty() {
                for (id, keys) in &d.outputs {
                    canvas.set_node_images(id, keys.clone());
                }
            }
        }
        self.load_images(doc);
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
    fn create_node(&self, doc: &DocRef, name: &str, pos: (f64, f64)) -> Option<String> {
        let Some(def) = self.registry.borrow().get(name).cloned() else {
            self.append_log(&format!("unknown node type {name}"));
            return None;
        };
        let id = {
            let mut d = doc.borrow_mut();
            let g = graph_mut(&mut d);
            let id = g.next_id();
            g.nodes.push(workflow::Node::from_def(&def, id.clone(), pos));
            id
        };
        self.refresh_doc(doc);
        doc.borrow().canvas.select(&[id.clone()]);
        self.set_dirty(doc, true);
        self.append_log(&format!("added {} at ({:.0}, {:.0})", def.display_name, pos.0, pos.1));
        Some(id)
    }

    /// Connect the node just created by the wire-drop picker to the socket the wire came
    /// from. The dragged type decides which of the new node's sockets is the partner.
    fn connect_new(&self, doc: &DocRef, new_id: &str, req: &(String, usize, bool, String)) {
        let (src, slot, out, ty) = req;
        let (from, from_slot, to, to_slot) = {
            let d = doc.borrow();
            let g = graph_ref(&d);
            let Some(new) = g.nodes.iter().find(|n| n.id == new_id) else { return };
            if *out {
                let Some((i, _)) = new.inputs.iter().enumerate().find(|(_, inp)| canvas::compatible(ty, &inp.ty)) else { return };
                (src.clone(), *slot, new_id.to_owned(), i)
            } else {
                let Some((i, _)) = new.outputs.iter().enumerate().find(|(_, o)| canvas::compatible(&o.ty, ty)) else { return };
                (new_id.to_owned(), i, src.clone(), *slot)
            }
        };
        doc.borrow().canvas.connect_ids(&from, from_slot, &to, to_slot);
        self.sync_doc(doc);
        self.edited(doc);
    }

    /// Open a popover menu at widget coordinates. Plain buttons, not GMenuModel:
    /// menu-item action resolution through widget action groups proved unreliable here.
    fn show_menu(&self, area: &impl IsA<gtk::Widget>, x: f64, y: f64, entries: Vec<Entry>) {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for e in entries {
            match e {
                Entry::Item { label, danger, enabled, icon, f } => {
                    let lb = gtk::Label::builder().label(&label).xalign(0.0).build();
                    if danger {
                        lb.add_css_class("error");
                    }
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    row.set_margin_start(10);
                    row.set_margin_end(18);
                    if let Some(i) = icon {
                        let im = gtk::Image::from_icon_name(i);
                        im.set_margin_end(8);
                        row.append(&im);
                    }
                    row.append(&lb);
                    let b = gtk::Button::builder().child(&row).sensitive(enabled).build();
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

    /// Run a widget-row menu action (see `canvas::widgets::menu`).
    fn widget_row_action(&self, doc: &DocRef, id: &str, row: usize, action: canvas::widgets::Action) {
        let Some((_, key, value)) = doc.borrow().canvas.widget_at_id(id, row) else { return };
        let mut d = doc.borrow_mut();
        let path = d.path.clone();
        let ok = match action {
            canvas::widgets::Action::Toggle => {
                let mut v = value;
                if let Some(o) = v.as_object_mut() {
                    let on = o.get("on").and_then(Value::as_bool).unwrap_or(true);
                    o.insert("on".to_owned(), Value::Bool(!on));
                }
                graph_view::apply_widget(&mut d.wf, &path, id, &key, v, "")
            }
            canvas::widgets::Action::MoveUp | canvas::widgets::Action::MoveDown => {
                let i = key.strip_prefix("lora:").and_then(|i| i.parse::<usize>().ok()).unwrap_or(0);
                let j = if action == canvas::widgets::Action::MoveUp { i.saturating_sub(1) } else { i + 1 };
                graph_view::swap_widget_rows(&mut d.wf, &path, id, i, j)
            }
            canvas::widgets::Action::Remove => graph_view::apply_widget(&mut d.wf, &path, id, &key, Value::Null, ""),
        };
        drop(d);
        if ok {
            self.render_doc(doc);
            self.set_dirty(doc, true);
        }
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
        // A widget row can carry its own actions (Power Lora rows, …). The list comes from
        // `canvas::widgets::menu`, so a custom widget kind opts in by adding one match arm
        // there rather than by teaching this menu about each node type.
        if let canvas::Hit::Widget(id, w) = &hit {
            let actions = doc.borrow().canvas.widget_menu(id, *w);
            if !actions.is_empty() {
                for (label, action, enabled) in actions {
                    let (me, d, id2, w2) = (self.clone(), doc.clone(), id.clone(), *w);
                    entries.push(item_i("", label, enabled, move || me.widget_row_action(&d, &id2, w2, action)));
                }
            }
        }
        if let canvas::Hit::Widget(id, _) = &hit {
            if entries.is_empty() {
                // Nothing row-specific: fall back to the node's menu.
                let node = canvas::Hit::Node(id.clone());
                return self.context_menu(doc, x, y, node);
            }
        }
        match hit {
            canvas::Hit::Widget(..) => {}
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
                    entries.push(item("Add Node…", move || me.add_node_dialog(&d, pos, None)));
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
    fn add_node_dialog(&self, doc: &DocRef, pos: (f64, f64), link: Option<(String, usize, bool, String)>) {
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
            let link = link.clone();
            Rc::new(move |q: &str| {
                while let Some(c) = results.first_child() {
                    results.remove(&c);
                }
                let q = q.to_lowercase();
                let words: Vec<&str> = q.split_whitespace().collect();
                let reg = me.registry.borrow();
                // Every word must match name, class or category; rank like the web search box:
                // exact name, then name prefix, then word-start, then anywhere.
                let mut hits: Vec<(u8, &crate::registry::NodeDef)> = reg
                    .nodes
                    .values()
                    .filter_map(|def| {
                        let name = def.display_name.to_lowercase();
                        let hay = format!("{name} {} {}", def.name.to_lowercase(), def.category.to_lowercase());
                        if !words.iter().all(|w| hay.contains(w)) {
                            return None;
                        }
                        // When the picker was opened by dropping a wire, only nodes that can
                        // take that wire are useful (the web UI shows the same filtered menu).
                        if let Some((_, _, out, ty)) = link.as_ref() {
                            let ok = if *out {
                                def.inputs.iter().any(|i| canvas::compatible(ty, i.type_name()))
                            } else {
                                def.outputs.iter().any(|o| canvas::compatible(&o.0, ty))
                            };
                            if !ok {
                                return None;
                            }
                        }
                        let rank = if q.is_empty() {
                            0
                        } else if name == q || def.name.to_lowercase() == q.replace(' ', "") {
                            0
                        } else if name.starts_with(&q) {
                            1
                        } else if name.contains(&q) && name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(words[0])) {
                            2
                        } else if name.contains(&q) {
                            3
                        } else {
                            4
                        };
                        Some((rank, def))
                    })
                    .collect();
                hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.display_name.len().cmp(&b.1.display_name.len())).then(a.1.display_name.cmp(&b.1.display_name)));
                // 4775 rows would be slow to build; show the first matches only.
                hits.truncate(60);
                let shown = hits.len();
                for (_, def) in hits {
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
                    let link2 = link.clone();
                    b.connect_clicked(move |_| {
                        if let Some(new_id) = me2.create_node(&doc2, &ty, pos) {
                            if let Some(req) = link2.clone() {
                                me2.connect_new(&doc2, &new_id, &req);
                            }
                        }
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
        // Enter adds the top hit, Esc closes — the keyboard flow of the web search box.
        {
            let results = results.clone();
            search.connect_activate(move |_| {
                if let Some(b) = results.first_child().and_downcast::<gtk::Button>() {
                    b.emit_clicked();
                }
            });
            let d = dlg.clone();
            search.connect_stop_search(move |_| d.close());
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
        if let Some(b) = self.proc.borrow_mut().take() {
            self.append_log("[backend] shutting down the launch we started…");
            b.stop_hard();
        }
    }

    /// LoRA Manager (comfyui-lora-manager) is a full web app the backend serves at
    /// `/loras`; it has no native equivalent. Rather than link WebKit (the host has
    /// no webkitgtk-6.0 headers to build against), hand it to the host browser in
    /// app mode — no tabs, no URL bar, the page fills a rounded CSD window, which is
    /// exactly the "GNOME window with the UI filling it" look.
    /// Close the utility dialogs (Settings, Templates). Called when the app is activated again.
    pub fn hide_dialogs(&self) {
        crate::workspace::settings::close_open();
    }

    fn open_settings(&self) {
        // Second click / Ctrl+, closes it again, so the window can always be dismissed.
        if settings::close_open() {
            return;
        }
        settings::open(self);
    }

    fn open_lora_manager(&self) {
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[lora] not connected");
            return;
        };
        let url = format!("{}/loras", client.base_url());
        // Chromium-family app mode first; Firefox has no app mode, so a plain window.
        // `xdg-open` is the last resort. Flatpak cannot see host browsers, so the
        // lookup runs on the host side.
        // Own profile per display: Chromium hands a URL to an already-running instance of the same
        // profile, which opens it on *that* instance's display — a browser left on another X server
        // (e.g. a headless test display) swallowed every window. Our profile is only ours.
        let display = std::env::var("WAYLAND_DISPLAY").or_else(|_| std::env::var("DISPLAY")).unwrap_or_default();
        let tag: String = display.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let profile = glib::user_cache_dir().join("comfy-gnome").join(format!("browser-{tag}"));
        let profile = profile.display();
        let script = format!(
            "for b in thorium-browser chromium chromium-browser google-chrome-stable google-chrome \
                 brave-browser microsoft-edge; do \
               command -v \"$b\" >/dev/null 2>&1 && exec \"$b\" --app='{url}' --new-window \
                 --user-data-dir='{profile}' --no-first-run --no-default-browser-check; done; \
             command -v firefox >/dev/null 2>&1 && exec firefox --new-window '{url}'; \
             exec xdg-open '{url}'"
        );
        let mut argv: Vec<std::ffi::OsString> = Vec::new();
        if backend::in_flatpak() {
            argv.extend(["flatpak-spawn", "--host"].map(Into::into));
        }
        argv.extend(["sh", "-c", &script].map(Into::into));
        let argv: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();
        match gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE).spawn(&argv) {
            Ok(sub) => {
                self.append_log(&format!("[lora] {url} → browser window"));
                glib::spawn_future_local(async move {
                    let _ = sub.wait_future().await;
                });
            }
            Err(e) => self.append_log(&format!("[lora] could not open a window: {e}")),
        }
    }

    pub fn append_log(&self, line: &str) {
        let buf = self.log.buffer();
        let from = buf.end_iter().offset();
        let text = strip_ansi(line.trim_end_matches('\n'));
        buf.insert(&mut buf.end_iter(), &text);
        let tag = if text.contains("ERROR") || text.contains("rror:") || text.contains("Traceback") {
            Some(("log-err", "#e5484d"))
        } else if text.contains("WARNING") || text.contains("lost connection") {
            Some(("log-warn", "#d9a000"))
        } else {
            None
        };
        if let Some((name, colour)) = tag {
            let table = buf.tag_table();
            if table.lookup(name).is_none() {
                buf.create_tag(Some(name), &[("foreground", &colour)]);
            }
            buf.apply_tag_by_name(name, &buf.iter_at_offset(from), &buf.end_iter());
        }
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
        // The MTB panel browses `<comfy_dir>/input` and `<comfy_dir>/output`; the "Skip" button
        // launches with an empty config, so fall back to the saved folder from launch.ini.
        let dir = cfg.comfy_dir.clone().or_else(|| backend::LaunchConfig::load().comfy_dir);
        if dir.is_some() || self.root_dir.borrow().is_none() {
            *self.root_dir.borrow_mut() = dir;
        }
        if let Some(m) = self.media.borrow().clone() {
            m.reload();
        }
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
                    // Port already served by something else: our launch lost the bind race.
                    // Saying so out loud explains "why does my old queue show up".
                    if me.proc.borrow().as_ref().is_some_and(|b| !b.is_running()) {
                        me.append_log(&format!(
                            "[backend] the process we launched exited at once — {} is served by an already-running backend",
                            cfg.base_url()
                        ));
                    }
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
        self.online.set(true);
        self.load_snap_settings(client.clone());
        self.load_history();
        self.load_registry();
        self.refresh_workflows();
        install_model_previews(client.clone());

        let (tx, rx) = async_channel::unbounded();
        client.spawn_ws(tx);
        let me = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                me.on_ws_event(ev);
            }
        });
    }

    /// Snap settings for every canvas, present and future.
    fn set_snap(&self, always: bool, grid: f64) {
        *self.snap.borrow_mut() = (always, grid);
        for d in self.docs.borrow().iter() {
            d.borrow().canvas.set_snap(always, grid);
        }
    }

    /// `Comfy.LinkRenderMode` for every canvas, present and future.
    fn set_link_mode(&self, mode: i32) {
        self.link_mode.set(mode);
        for d in self.docs.borrow().iter() {
            d.borrow().canvas.set_link_mode(mode);
        }
    }

    /// `Comfy.Execution.PreviewMethod` (live preview frames during sampling).
    pub fn set_preview_method(&self, method: &str) {
        *self.preview_method.borrow_mut() = method.to_owned();
    }

    /// `Comfy.SnapToGrid.GridSize` + `pysssss.SnapToGrid` decide whether dragging snaps.
    fn load_snap_settings(&self, c: api::Client) {
        let me = self.clone();
        glib::spawn_future_local(async move {
            let v = match gio::spawn_blocking(move || c.settings()).await {
                Ok(Ok(m)) => Value::Object(m.into_iter().collect()),
                Ok(Err(e)) => {
                    me.append_log(&format!("[settings] {e}"));
                    Value::Null
                }
                Err(_) => Value::Null,
            };
            let grid = v.get("Comfy.SnapToGrid.GridSize").and_then(Value::as_f64).unwrap_or(0.0);
            let always = v.get("pysssss.SnapToGrid").and_then(Value::as_bool).unwrap_or(false);
            me.set_snap(always, grid);
            // Litegraph's enum: -1 hidden, 0 straight, 1 linear, 2 spline (the default).
            me.set_link_mode(v.get("Comfy.LinkRenderMode").and_then(Value::as_i64).map_or(2, |m| m as i32));
            *me.preview_method.borrow_mut() = v
                .get("Comfy.Execution.PreviewMethod")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_owned();
            me.persist.set(v.get("Comfy.Workflow.Persist").and_then(Value::as_bool).unwrap_or(true));
            me.restore_session();
            if me.docs.borrow().is_empty() {
                me.doc_or_blank("Untitled");
            }
        });
    }

    /// `GET /object_info` → node registry (again after a reconnect: the backend may have new nodes).
    fn load_registry(&self) {
        let Some(c) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let t = std::time::Instant::now();
            // `/i18n` carries the custom-node display names the V3 schema omits
            // (`easy wildcards` -> "Wildcards"). It is small, so fetch it alongside.
            match gio::spawn_blocking(move || {
                let v = c.get("/object_info")?;
                let i18n = c.get("/i18n").unwrap_or(Value::Null);
                Ok::<_, String>((v, i18n))
            })
            .await
            {
                Ok(Ok((v, i18n))) => {
                    let mut reg = registry::Registry::from_object_info(&v);
                    reg.apply_display_names(&i18n);
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
    }

    /// Outline the running node (subgraph inner ids "75:73" map to their host "75").
    fn set_running(&self, node: Option<&str>) {
        let doc = self.run_doc.borrow().as_ref().and_then(|w| w.upgrade());
        if let Some(d) = doc {
            let id = node.map(|n| n.split(':').next().unwrap_or(n).to_owned());
            d.borrow().canvas.set_running(id);
        }
    }

    /// Remember a root node's output images and show them on its canvas node.
    fn show_outputs(&self, node: Option<&str>, images: &[api::OutputImage]) {
        let doc = self.run_doc.borrow().as_ref().and_then(|w| w.upgrade());
        let (Some(d), Some(id)) = (doc, node.filter(|n| !n.contains(':'))) else { return };
        let keys: Vec<String> = images.iter().map(api::OutputImage::query).collect();
        d.borrow_mut().outputs.insert(id.to_owned(), keys.clone());
        if d.borrow().path.is_empty() {
            d.borrow().canvas.set_node_images(id, keys);
            self.load_images(&d);
        }
    }

    fn on_ws_event(&self, ev: api::WsEvent) {
        use api::WsEvent::*;
        match ev {
            Connected => {
                let base = self.client.borrow().as_ref().map(|c| c.base_url().to_owned()).unwrap_or_default();
                self.title.set_subtitle(&format!("Connected · {base}"));
                if self.online.replace(true) {
                    self.append_log("[ws] connected");
                } else {
                    // Backend restarted: node packs / queue / workflows may all have changed.
                    self.append_log("[ws] reconnected — reloading node definitions");
                    self.load_registry();
                    self.load_history();
                    self.refresh_workflows();
                }
                self.refresh_queue();
            }
            // Log the drop once; the ws thread keeps retrying every 2 s silently.
            Disconnected(why) => {
                if self.online.replace(false) {
                    self.append_log(&format!("[ws] lost connection: {why} — reconnecting…"));
                    let base = self.client.borrow().as_ref().map(|c| c.base_url().to_owned()).unwrap_or_default();
                    self.title.set_subtitle(&format!("Disconnected · waiting for {base}…"));
                    self.progress.set_visible(false);
                    // Nothing can be running on a backend that is gone; the queue key reset
                    // makes the next snapshot after reconnect re-apply button state.
                    self.stop_btn.set_sensitive(false);
                    self.interrupt_btn.set_sensitive(false);
                    self.queue_key.borrow_mut().clear();
                    self.set_running(None);
                }
            }
            Status { queue_remaining } => {
                self.append_log(&format!("[ws] queue: {queue_remaining} remaining"));
                self.refresh_queue();
            }
            Progress { value, max, node } => {
                self.title.set_subtitle(&format!("Running node {} · {value}/{max}", node.unwrap_or_default()));
                let fraction = if max > 0 { (value as f64 / max as f64).clamp(0.0, 1.0) } else { 0.0 };
                self.progress.set_fraction(fraction);
                self.progress.set_visible(true);
            }
            Executing { node: None, prompt_id } => {
                self.progress.set_visible(false);
                self.set_running(None);
                self.append_log(&format!("[ws] prompt {} finished", prompt_id.clone().unwrap_or_default()));
                self.refresh_queue();
                self.note_finished(prompt_id);
            }
            Executing { node: Some(n), .. } => {
                self.progress.set_fraction(0.0);
                self.progress.set_visible(false);
                self.set_running(Some(&n));
                self.append_log(&format!("[ws] executing node {n}"));
                self.refresh_queue();
            }
            ExecutionStart { prompt_id } => {
                self.progress.set_fraction(0.0);
                self.progress.set_visible(false);
                self.append_log(&format!("[ws] prompt {prompt_id} started"));
                self.refresh_queue();
            }
            ExecutionError { message } => {
                self.progress.set_visible(false);
                self.append_log(&format!("[ws] ERROR {message}"));
                self.refresh_queue();
            }
            Preview { bytes, display_node } => {
                // Live frames are transient (no filename to re-fetch) — enlarge nothing.
                self.preview_items.borrow_mut().clear();
                // Same rule as the web UI: the frame goes on the backend's display node
                // (the outer host for subgraphs/custom samplers), not the inner worker.
                self.show_live_preview(display_node.as_deref(), &bytes);
                self.show_image(bytes, "live preview")
            }
            Executed { node, images } => {
                self.append_log(&format!("[ws] executed {}: {} images", node.clone().unwrap_or_default(), images.len()));
                self.show_outputs(node.as_deref(), &images);
                *self.preview_items.borrow_mut() =
                    images.iter().map(|i| (i.filename.clone(), i.query())).collect();
                if let Some(img) = images.into_iter().next() {
                    let Some(client) = self.client.borrow().clone() else { return };
                    let me = self.clone();
                    let caption = format!("{}{}", img.subfolder, img.filename);
                    glib::spawn_future_local(async move {
                        match gio::spawn_blocking(move || client.view_query(&img.query())).await {
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

/// "Today" / "Yesterday" / "7 Nov 2026" for an epoch-millisecond timestamp.
fn day_label(ms: i64) -> String {
    let Ok(dt) = glib::DateTime::from_unix_local(ms / 1000) else { return String::new() };
    let Ok(now) = glib::DateTime::now_local() else { return String::new() };
    let day = |d: &glib::DateTime| (d.year(), d.month(), d.day_of_month());
    if day(&dt) == day(&now) {
        return "Today".into();
    }
    if let Ok(y) = now.add_days(-1) {
        if day(&dt) == day(&y) {
            return "Yesterday".into();
        }
    }
    dt.format("%e %b %Y").ok().map(|s| s.to_string()).unwrap_or_default()
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
