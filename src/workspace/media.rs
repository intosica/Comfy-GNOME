// SPDX-License-Identifier: GPL-3.0-only
//! MTB-style input/output browser (`reference/MTB.png`), improved over the web panel:
//!
//! * a folder dropdown (input/output and their subfolders) plus double-click to enter a folder;
//! * uniform thumbnail grid with a zoom slider and a sort selector;
//! * GNOME multi-select: Ctrl+click toggles, Shift+click extends, Ctrl+drag rubber-bands (from anywhere);
//! * right-click → delete the selection after a confirmation that moves files to the Trash, and
//!   "Show Item Location" opens the file manager with the file selected;
//! * every image is draggable into the canvas (metadata → workflow, otherwise a Load Image node).

use super::*;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// ponytail: cap how many thumbnails a single folder materialises; 400 full-size decodes in RAM
/// would be ~1.5 GB, and a 1000-file folder is unreadable anyway. Narrow the folder instead.
const MAX_ITEMS: usize = 400;

#[derive(Clone)]
pub(super) struct Media {
    ws: Workspace,
    /// `input` or `output`.
    root: Rc<RefCell<String>>,
    /// Subfolder segments below the root.
    path: Rc<RefCell<Vec<String>>>,
    flow: gtk::FlowBox,
    items: Rc<RefCell<Vec<PathBuf>>>,
    /// Folder chooser; its labels are `output - realism`-style, like the web MTB panel.
    chooser: gtk::DropDown,
    /// Labels and the path they stand for, parallel to `chooser`'s model.
    chooser_map: Rc<RefCell<Vec<(String, String, Vec<String>)>>>,
    /// Guards `chooser` against firing its own `notify` while we sync the selection.
    chooser_sync: Rc<Cell<bool>>,
    roots: gtk::DropDown,
    /// Selected paths. The flow box runs without GTK selection (its BROWSE/MULTIPLE modes
    /// start a rubber band on any drag, which stole thumbnail drags); selection is ours.
    sel: Rc<RefCell<std::collections::HashSet<PathBuf>>>,
    anchor: Rc<RefCell<Option<PathBuf>>>,
    back: gtk::Button,
    status: gtk::Label,
    zoom: Rc<Cell<i32>>,
    sort: Rc<Cell<u8>>,
    /// Bumped on every reload so in-flight thumbnail decodes do not paint into recycled tiles.
    gen: Rc<Cell<u32>>,
    cache: Rc<RefCell<HashMap<PathBuf, gtk::gdk::Texture>>>,
    fixed: gtk::Fixed,
    /// The panel's overlay. Event coordinates arrive in surface (window) space, so every hit test
    /// and every band move must first map them here.
    ov: gtk::Overlay,
}

pub(super) fn page(ws: &Workspace) -> gtk::Widget {
    let flow = gtk::FlowBox::builder()
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(24)
        .column_spacing(4)
        .row_spacing(4)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(4)
        .margin_end(4)
        .selection_mode(gtk::SelectionMode::None)
        // Without this the box stretches every cell to the full pane height, which makes the
        // whole panel "inside a thumbnail" for hit testing (no empty space to start a band on).
        .valign(gtk::Align::Start)
        // Ctrl+click must only toggle the selection; a single click should not preview.
        .activate_on_single_click(false)
        .build();
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&flow).build();

    // Rubber band: a translucent box moved inside a `Fixed` that covers the list.
    let band = gtk::Box::new(gtk::Orientation::Vertical, 0);
    band.add_css_class("mtb-band");
    band.set_visible(false);
    band.set_can_target(false);
    let fixed = gtk::Fixed::new();
    fixed.set_can_target(false);
    fixed.put(&band, 0.0, 0.0);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroller));
    overlay.add_overlay(&fixed);

    let chooser = gtk::DropDown::from_strings(&[]);
    chooser.set_hexpand(true);
    chooser.set_enable_search(true);
    // Search needs an expression to match on; substring so "10-24" finds "output - AIXG - 2026-10-24".
    chooser.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    chooser.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    chooser.set_tooltip_text(Some("Choose a folder (type to filter)"));
    let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Up one folder").sensitive(false).build();
    let status = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["dim-label", "caption"]).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
    // The root chooser is fixed-width so the path field gets the room the paths need.
    let root = gtk::DropDown::from_strings(&["Input", "Output"]);
    root.set_hexpand(false);
    root.set_tooltip_text(Some("Browse input/ or output/"));
    root.set_valign(gtk::Align::Center);
    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Reload folder").build();
    let zoom = Rc::new(Cell::new(88));
    let sort = gtk::DropDown::from_strings(&["Modified", "Name"]);
    sort.set_tooltip_text(Some("Sort order"));
    let zoom_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 56.0, 260.0, 4.0);
    zoom_scale.set_value(88.0);
    zoom_scale.set_hexpand(true);
    zoom_scale.set_draw_value(false);
    zoom_scale.set_tooltip_text(Some("Thumbnail size"));

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.set_margin_top(6);
    top.set_margin_bottom(6);
    top.set_margin_start(6);
    top.set_margin_end(6);
    top.append(&back);
    top.append(&chooser);
    top.append(&refresh);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom.set_margin_start(6);
    bottom.set_margin_end(6);
    bottom.append(&root);
    bottom.append(&sort);
    bottom.append(&zoom_scale);
    let foot = gtk::Box::new(gtk::Orientation::Vertical, 2);
    foot.set_margin_bottom(4);
    foot.append(&bottom);
    foot.append(&status);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.append(&top);
    page.append(&overlay);
    page.append(&foot);

    let me = Media {
        ws: ws.clone(),
        root: Rc::new(RefCell::new("input".into())),
        path: Rc::new(RefCell::new(Vec::new())),
        flow: flow.clone(),
        items: Rc::new(RefCell::new(Vec::new())),
        chooser: chooser.clone(),
        chooser_map: Rc::new(RefCell::new(Vec::new())),
        chooser_sync: Rc::new(Cell::new(false)),
        roots: root.clone(),
        sel: Rc::new(RefCell::new(std::collections::HashSet::new())),
        anchor: Rc::new(RefCell::new(None)),
        back: back.clone(),
        status: status.clone(),
        zoom: zoom.clone(),
        sort: Rc::new(Cell::new(0)),
        gen: Rc::new(Cell::new(0)),
        cache: Rc::new(RefCell::new(HashMap::new())),
        fixed: fixed.clone(),
        ov: overlay.clone(),
    };
    *ws.media.borrow_mut() = Some(me.clone());

    // --- navigation ---------------------------------------------------------
    {
        let me = me.clone();
        root.connect_selected_notify(move |d| {
            let want = if d.selected() == 1 { "output" } else { "input" };
            if *me.root.borrow() == want {
                return;
            }
            *me.root.borrow_mut() = want.into();
            me.roots.set_selected(if want == "output" { 1 } else { 0 });
            me.path.borrow_mut().clear();
            me.reload();
        });
    }
    {
        let me = me.clone();
        chooser.connect_selected_notify(move |d| {
            if me.chooser_sync.get() {
                return;
            }
            let idx = d.selected() as usize;
            let map = me.chooser_map.borrow();
            let Some((_, root, parts)) = map.get(idx).cloned() else { return };
            drop(map);
            *me.root.borrow_mut() = root.clone();
            me.roots.set_selected(if root == "output" { 1 } else { 0 });
            *me.path.borrow_mut() = parts;
            me.reload();
        });
    }
    {
        let me = me.clone();
        back.connect_clicked(move |_| {
            if me.path.borrow_mut().pop().is_some() {
                me.reload();
            }
        });
    }
    {
        let me = me.clone();
        refresh.connect_clicked(move |_| {
            me.cache.borrow_mut().clear();
            me.refresh_choices();
            me.reload();
        });
    }
    {
        let me = me.clone();
        sort.connect_selected_notify(move |d| {
            me.sort.set(d.selected().min(1) as u8);
            me.reload();
        });
    }
    {
        let me = me.clone();
        zoom_scale.connect_value_changed(move |s| {
            let px = s.value() as i32;
            if px != me.zoom.get() {
                me.zoom.set(px);
                me.reload();
            }
        });
    }

    // --- activation ---------------------------------------------------------
    {
        let me = me.clone();
        flow.connect_child_activated(move |_, fc| {
            let Some(p) = me.path_of(fc) else { return };
            if p.is_dir() {
                if let Some(name) = p.file_name() {
                    me.path.borrow_mut().push(name.to_string_lossy().into_owned());
                    me.reload();
                }
            } else {
                me.preview(&p);
            }
        });
    }

    // --- pointer tracking: Ctrl+drag rubber band, plain drag to the canvas ----
    // A `GtkGestureDrag` is unusable here: it claims the sequence as soon as it starts, which
    // cancels every gesture below it (the flow box's clicks and the tiles' drag source). An
    // event controller in the capture phase sees press/motion/release without claiming anything.
    {
        #[derive(Clone)]
        struct Press {
            x: f64,
            y: f64,
            ctrl: bool,
            moved: bool,
            path: Option<PathBuf>,
        }
        let press_state: Rc<RefCell<Option<Press>>> = Rc::new(RefCell::new(None));
        let ctl = gtk::EventControllerLegacy::new();
        ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (m, ps, f, b) = (me.clone(), press_state.clone(), fixed.clone(), band.clone());
        ctl.connect_event(move |_, ev| {
            let Some((x, y)) = ev.position() else { return gtk::glib::Propagation::Proceed };
            match ev.event_type() {
                gtk::gdk::EventType::ButtonPress => {
                    let Some(be) = ev.downcast_ref::<gtk::gdk::ButtonEvent>() else { return gtk::glib::Propagation::Proceed };
                    if be.button() != 1 {
                        return gtk::glib::Propagation::Proceed;
                    }
                    let (fx, fy) = m.to_flow(x, y);
                    let hit = m.flow.child_at_pos(fx, fy).is_some();
                    let state = be.modifier_state();
                    let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                    let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                    let path = hit.then(|| m.path_at(x, y)).flatten();
                    // Selection is ours (the flow box runs without GTK selection so it cannot
                    // start a rubber band on a thumbnail drag). Ctrl toggles, Shift extends,
                    // plain click replaces; clicking an already-selected tile keeps the set
                    // so a multi-selection can still be dragged.
                    // Ctrl on a tile toggles on release, so Ctrl+drag can band from anywhere
                    // (a full pane has no empty space to start on).
                    match path.as_ref() {
                        Some(_) if ctrl => {}
                        Some(p) if shift => m.select_range_to(p),
                        Some(p) if !m.sel.borrow().contains(p) => m.select_only(p),
                        Some(_) => {}
                        None if !ctrl => m.clear_selection(),
                        None => {}
                    }
                    *ps.borrow_mut() = Some(Press { x, y, ctrl, moved: false, path });
                }
                gtk::gdk::EventType::MotionNotify => {
                    let Some(mut p) = ps.borrow().clone() else { return gtk::glib::Propagation::Proceed };
                    if !p.ctrl || (!p.moved && (x - p.x).abs() + (y - p.y).abs() < 6.0) {
                        return gtk::glib::Propagation::Proceed;
                    }
                    p.moved = true;
                    *ps.borrow_mut() = Some(p.clone());
                    let (ox, oy) = m.to_overlay(x, y);
                    let (px, py) = m.to_overlay(p.x, p.y);
                    f.move_(&b, px.min(ox), py.min(oy));
                    b.set_size_request((ox - px).abs().max(1.0) as i32, (oy - py).abs().max(1.0) as i32);
                    b.set_visible(true);
                }
                gtk::gdk::EventType::ButtonRelease => {
                    let Some(be) = ev.downcast_ref::<gtk::gdk::ButtonEvent>() else { return gtk::glib::Propagation::Proceed };
                    if be.button() != 1 {
                        return gtk::glib::Propagation::Proceed;
                    }
                    let Some(p) = ps.take() else { return gtk::glib::Propagation::Proceed };
                    b.set_visible(false);
                    if p.ctrl && !p.moved {
                        if let Some(path) = p.path.as_ref() { m.toggle_path(path); }
                    } else if p.moved && p.ctrl {
                        let (ox, oy) = m.to_overlay(x, y);
                        let (px, py) = m.to_overlay(p.x, p.y);
                        m.select_rect(px, py, ox, oy);
                    } else if !p.ctrl && (x - p.x).abs() + (y - p.y).abs() > 12.0 {
                        if let Some(path) = p.path.clone().or_else(|| m.path_at(p.x, p.y)) {
                            let (ox, oy) = m.to_overlay(x, y);
                            m.drop_at(&path, ox, oy);
                        }
                    }
                }
                _ => {}
            }
            gtk::glib::Propagation::Proceed
        });
        overlay.add_controller(ctl);
    }

    // --- right-click on empty space -----------------------------------------
    {
        let me = me.clone();
        let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
        click.connect_pressed(move |g, _, x, y| {
            if me.flow.child_at_pos(x as i32, y as i32).is_some() {
                return; // a thumbnail handles its own menu
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(w) = g.widget() { me.empty_menu(&w, x, y); }
        });
        overlay.add_controller(click);
    }

    // --- control-a ----------------------------------------------------------
    {
        let me = me.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, state| {
            if key == gtk::gdk::Key::a && state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                me.select_all();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        overlay.add_controller(keys);
    }

    page.upcast()
}

impl Media {
    fn base(&self) -> Option<PathBuf> {
        let root = self.ws.root_dir.borrow().clone()?;
        let mut p = root.join(&*self.root.borrow());
        for s in self.path.borrow().iter() {
            p = p.join(s);
        }
        Some(p)
    }

    /// `output/AIXG/2026-11-07` — what the top field shows.
    fn label(&self) -> String {
        let mut s = self.root.borrow().clone();
        for p in self.path.borrow().iter() {
            s.push('/');
            s.push_str(p);
        }
        s
    }

    /// The i-th `FlowBoxChild` (FlowBox has no index lookup in GTK4).
    fn child(&self, i: usize) -> Option<gtk::FlowBoxChild> {
        let mut idx = 0;
        let mut c = self.flow.first_child();
        while let Some(w) = c {
            if let Ok(fc) = w.clone().downcast::<gtk::FlowBoxChild>() {
                if idx == i {
                    return Some(fc);
                }
                idx += 1;
            }
            c = w.next_sibling();
        }
        None
    }

    fn index_of(&self, fc: &gtk::FlowBoxChild) -> Option<usize> {
        let mut idx = 0;
        let mut c = self.flow.first_child();
        while let Some(w) = c {
            if w == *fc {
                return Some(idx);
            }
            if w.is::<gtk::FlowBoxChild>() {
                idx += 1;
            }
            c = w.next_sibling();
        }
        None
    }

    fn path_of(&self, fc: &gtk::FlowBoxChild) -> Option<PathBuf> {
        let i = self.index_of(fc)?;
        self.items.borrow().get(i).cloned()
    }

    /// Rebuild the folder dropdown: `input`, `input - 3D`, `output - realism`, … — both roots,
    /// top-level folders only (subfolders are reached by double-clicking in the grid).
    fn refresh_choices(&self) {
        let Some(base) = self.ws.root_dir.borrow().clone() else { return };
        let mut map: Vec<(String, String, Vec<String>)> = Vec::new();
        for root in ["input", "output"] {
            map.push((root.to_owned(), root.to_owned(), Vec::new()));
            let mut stack = vec![(base.join(root), Vec::<String>::new())];
            while let Some((dir, parts)) = stack.pop() {
                if !parts.is_empty() || map.len() > 800 {
                    continue;
                }
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                let mut subs: Vec<(String, PathBuf)> = rd
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                    .collect();
                subs.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, path) in subs.into_iter().rev() {
                    let mut p = parts.clone();
                    p.push(name);
                    let label = format!("{root} - {}", p.join(" - "));
                    if !map.iter().any(|(l, ..)| *l == label) {
                        map.push((label, root.to_owned(), p.clone()));
                    }
                    stack.push((path, p));
                }
            }
        }
        map.sort_by(|a, b| (a.1 != "input", &a.0).cmp(&(b.1 != "input", &b.0)));
        let labels: Vec<&str> = map.iter().map(|(l, ..)| l.as_str()).collect();
        let model = gtk::StringList::new(&labels);
        self.chooser_sync.set(true);
        self.chooser.set_model(Some(&model));
        *self.chooser_map.borrow_mut() = map;
        self.chooser_sync.set(false);
        self.sync_chooser();
    }

    /// Point the dropdown at the folder currently shown.
    fn sync_chooser(&self) {
        let map = self.chooser_map.borrow();
        let (root, path) = (self.root.borrow().clone(), self.path.borrow().clone());
        // The dropdown lists only top-level folders; deeper paths show their top-level parent.
        let idx = map.iter().position(|(_, r, p)| *r == root && p[..] == path[..path.len().min(1)]);
        drop(map);
        if let Some(i) = idx {
            self.chooser_sync.set(true);
            self.chooser.set_selected(i as u32);
            self.chooser_sync.set(false);
        }
    }

    pub(super) fn reload(&self) {
        self.gen.set(self.gen.get() + 1);
        let gen = self.gen.get();
        while let Some(c) = self.flow.first_child() {
            self.flow.remove(&c);
        }
        self.items.borrow_mut().clear();
        self.sel.borrow_mut().clear();
        *self.anchor.borrow_mut() = None;
        if self.chooser_map.borrow().is_empty() {
            self.refresh_choices();
        }
        self.sync_chooser();
        self.back.set_sensitive(!self.path.borrow().is_empty());
        let Some(base) = self.base() else {
            self.status.set_text("Set the ComfyUI folder on the launch screen to browse input/ and output/");
            return;
        };
        let read = std::fs::read_dir(&base).map(|rd| rd.flatten().collect::<Vec<_>>()).unwrap_or_default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for e in read {
            let p = e.path();
            if p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true) {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else if is_image(&p) {
                files.push(p);
            }
        }
        dirs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        if self.sort.get() == 1 {
            files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        } else {
            files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
        }
        let total = dirs.len() + files.len();
        dirs.extend(files);
        dirs.truncate(MAX_ITEMS);
        let px = self.zoom.get();
        for p in dirs.iter() {
            let tile = self.tile(&base, p, px, gen);
            self.flow.insert(&tile, -1);
            self.items.borrow_mut().push(p.clone());
        }
        let label = self.label();
        let mut text = format!("{label} · {} images · {} folders", self.items.borrow().iter().filter(|p| !p.is_dir()).count(), self.items.borrow().iter().filter(|p| p.is_dir()).count());
        if total > MAX_ITEMS {
            text.push_str(&format!(" · showing {MAX_ITEMS} of {total}"));
        }
        self.status.set_text(&text);
    }

    /// Overlay coordinates → flow box coordinates (the two differ by the panes above it).
    fn to_flow(&self, x: f64, y: f64) -> (i32, i32) {
        let (x, y) = self.to_overlay(x, y);
        match self.ov.compute_point(&self.flow, &gtk::graphene::Point::new(x as f32, y as f32)) {
            Some(p) => (p.x() as i32, p.y() as i32),
            None => (x as i32, y as i32),
        }
    }

    /// Raw GDK event coordinates are surface coordinates: shift them into the panel overlay.
    fn to_overlay(&self, x: f64, y: f64) -> (f64, f64) {
        let Some(root) = self.ov.root() else { return (x, y) };
        match self.ov.compute_bounds(&root) {
            Some(b) => (x - b.x() as f64, y - b.y() as f64),
            None => (x, y),
        }
    }

    /// The file under an overlay point, if any.
    fn path_at(&self, x: f64, y: f64) -> Option<PathBuf> {
        let (fx, fy) = self.to_flow(x, y);
        self.path_of(&self.flow.child_at_pos(fx, fy)?)
    }

    /// Select every cell intersecting the rectangle `(x0, y0)–(x, y)` in overlay coordinates.
    fn select_rect(&self, x0: f64, y0: f64, x: f64, y: f64) {
        let rect = gtk::graphene::Rect::new(x0.min(x) as f32, y0.min(y) as f32, (x - x0).abs() as f32, (y - y0).abs() as f32);
        self.clear_selection();
        for i in 0..self.items.borrow().len() {
            if let Some(fc) = self.child(i) {
                let Some(b) = fc.compute_bounds(&self.fixed) else { continue };
                if b.intersection(&rect).is_some() {
                    let p = self.items.borrow()[i].clone();
                    self.mark(&p, true);
                }
            }
        }
        let n = self.sel.borrow().len();
        if n > 0 {
            self.ws.append_log(&format!("[mtb] {n} selected"));
        }
    }

    /// Dropping a thumbnail on the canvas: a ComfyUI image opens its workflow, anything else
    /// becomes a preloaded Load Image node at the drop point.
    fn drop_at(&self, path: &Path, x: f64, y: f64) {
        // `x`/`y` are overlay-local; the canvas needs them in its own space.
        if path.is_dir() {
            return;
        }
        // No open workflow: the drop itself decides (a metadata image opens a tab, a plain
        // picture opens a blank workflow with a Load Image node already placed).
        let pos = match self.ws.active_doc() {
            Some(doc) => {
                let area = doc.borrow().canvas.area.clone();
                let Some(p) = self.ov.compute_point(&self.ws.root, &gtk::graphene::Point::new(x as f32, y as f32)) else { return };
                let Some(bounds) = area.compute_bounds(&self.ws.root) else { return };
                if p.x() < bounds.x() || p.y() < bounds.y() || p.x() > bounds.x() + bounds.width() || p.y() > bounds.y() + bounds.height() {
                    return;
                }
                let Some(local) = self.ws.root.compute_point(&area, &p) else { return };
                let (zoom, off) = doc.borrow().canvas.view();
                ((local.x() as f64 - off.0) / zoom, (local.y() as f64 - off.1) / zoom)
            }
            None => (160.0, 120.0),
        };
        self.ws.open_dropped_at(&gio::File::for_path(path), pos);
    }

    /// One grid cell: thumbnail (or folder icon), filename, drag source, context menu.
    fn tile(&self, _base: &Path, path: &Path, px: i32, gen: u32) -> gtk::Box {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        holder.add_css_class("mtb-tile");
        if self.sel.borrow().contains(path) {
            holder.add_css_class("mtb-selected");
        }
        holder.set_halign(gtk::Align::Center);
        holder.set_valign(gtk::Align::Start);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let is_dir = path.is_dir();
        if is_dir {
            let ov = gtk::Overlay::new();
            ov.set_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
            let icon = gtk::Image::builder().icon_name("folder-symbolic").pixel_size(px / 2).halign(gtk::Align::Center).valign(gtk::Align::Center).css_classes(["dim-label"]).build();
            ov.add_overlay(&icon);
            ov.set_size_request(px, px);
            holder.append(&ov);
        } else {
            // `Gtk.Picture` reports the texture's pixel size as its natural size, which would make
            // the cell as wide as the image; `Gtk.Image` with a pixel size scales it to the cell
            // instead — and, unlike a `Gtk.ScrolledWindow`, it does not steal drag gestures from
            // the tile's `DragSource`.
            let pic = gtk::Image::builder().pixel_size(px).css_classes(["mtb-thumb"]).build();
            if let Some(t) = self.cache.borrow().get(path) {
                pic.set_paintable(Some(t));
            } else {
                let key = path.to_path_buf();
                let (pic2, p2, cache, g) = (pic.clone(), key.clone(), self.cache.clone(), self.gen.clone());
                glib::spawn_future_local(async move {
                    let bytes = gio::spawn_blocking(move || std::fs::read(&p2)).await.ok().and_then(|r| r.ok());
                    if g.get() != gen {
                        return;
                    }
                    if let Some(b) = bytes {
                        if let Ok(t) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(b)) {
                            cache.borrow_mut().insert(key.clone(), t.clone());
                            pic2.set_paintable(Some(&t));
                        }
                    }
                });
            }
            holder.append(&pic);
        }
        holder.append(&gtk::Label::builder().label(&name).ellipsize(gtk::pango::EllipsizeMode::Middle).max_width_chars(12).lines(1).xalign(0.5).css_classes(["caption"]).build());

        let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
        {
            let (me, p) = (self.clone(), path.to_path_buf());
            click.connect_pressed(move |g, _, x, y| {
                if !me.sel.borrow().contains(&p) {
                    me.select_only(&p);
                }
                g.set_state(gtk::EventSequenceState::Claimed);
                if let Some(w) = g.widget() { me.selection_menu(&w, x, y); }
            });
        }
        holder.add_controller(click);
        holder
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.sel.borrow().iter().cloned().collect();
        out.sort();
        out
    }

    fn mark(&self, path: &Path, on: bool) {
        if on {
            self.sel.borrow_mut().insert(path.to_path_buf());
        } else {
            self.sel.borrow_mut().remove(path);
        }
        if let Some(i) = self.items.borrow().iter().position(|p| p == path) {
            if let Some(fc) = self.child(i) {
                if let Some(w) = fc.child() {
                    if on {
                        w.add_css_class("mtb-selected");
                    } else {
                        w.remove_css_class("mtb-selected");
                    }
                }
            }
        }
    }

    fn clear_selection(&self) {
        self.sel.borrow_mut().clear();
        *self.anchor.borrow_mut() = None;
        for i in 0..self.items.borrow().len() {
            if let Some(fc) = self.child(i) {
                if let Some(w) = fc.child() {
                    w.remove_css_class("mtb-selected");
                }
            }
        }
    }

    fn select_only(&self, path: &Path) {
        self.clear_selection();
        self.mark(path, true);
        *self.anchor.borrow_mut() = Some(path.to_path_buf());
    }

    fn toggle_path(&self, path: &Path) {
        let on = !self.sel.borrow().contains(path);
        self.mark(path, on);
        *self.anchor.borrow_mut() = Some(path.to_path_buf());
    }

    fn select_range_to(&self, path: &Path) {
        let items = self.items.borrow().clone();
        let Some(to) = items.iter().position(|p| p == path) else { return };
        let anchor = self.anchor.borrow().clone().and_then(|a| items.iter().position(|p| *p == a)).unwrap_or(to);
        self.clear_selection();
        for p in items.iter().take(to.max(anchor) + 1).skip(to.min(anchor)) {
            self.mark(p, true);
        }
        *self.anchor.borrow_mut() = Some(items[anchor].clone());
    }

    fn select_all(&self) {
        self.clear_selection();
        for p in self.items.borrow().iter() {
            self.mark(p, true);
        }
    }

    fn preview(&self, path: &Path) {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(t) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes)) {
                self.ws.preview.set_paintable(Some(&t));
                self.ws.preview_caption.set_text(&path.display().to_string());
                self.ws.bottom.set_visible_child_name("preview");
                self.ws.set_panel_collapsed(false);
            }
        }
    }

    fn selection_menu(&self, anchor: &gtk::Widget, x: f64, y: f64) {
        let paths = self.selected_paths();
        let pop = gtk::Popover::new();
        pop.set_parent(anchor);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pop.set_child(Some(&list));
        let loc = gtk::Button::with_label("Show Item Location");
        {
            let (me, p) = (self.clone(), paths.first().cloned());
            loc.connect_clicked(move |_| {
                if let Some(p) = p.as_ref() {
                    show_location(&me.ws, p);
                }
            });
        }
        list.append(&loc);
        let del = gtk::Button::with_label(&format!("Delete {} Selected…", paths.len()));
        del.add_css_class("destructive-action");
        del.set_sensitive(!paths.is_empty());
        {
            let me = self.clone();
            let (pop, p) = (pop.clone(), paths);
            del.connect_clicked(move |_| {
                pop.popdown();
                delete_selected(me.clone(), p.clone());
            });
        }
        list.append(&del);
        pop.popup();
    }

    fn empty_menu(&self, anchor: &gtk::Widget, x: f64, y: f64) {
        let pop = gtk::Popover::new();
        pop.set_parent(anchor);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pop.set_child(Some(&list));
        let open = gtk::Button::with_label("Show Folder in Files");
        {
            let me = self.clone();
            open.connect_clicked(move |_| {
                if let Some(base) = me.base() {
                    show_location(&me.ws, &base);
                }
            });
        }
        list.append(&open);
        let paths = self.selected_paths();
        let del = gtk::Button::with_label(&format!("Delete {} Selected…", paths.len()));
        del.add_css_class("destructive-action");
        del.set_sensitive(!paths.is_empty());
        {
            let me = self.clone();
            let (pop, paths) = (pop.clone(), paths);
            del.connect_clicked(move |_| {
                pop.popdown();
                delete_selected(me.clone(), paths.clone());
            });
        }
        list.append(&del);
        let refresh = gtk::Button::with_label("Refresh");
        {
            let me = self.clone();
            let pop2 = pop.clone();
            refresh.connect_clicked(move |_| {
                pop2.popdown();
                me.cache.borrow_mut().clear();
                me.reload();
            });
        }
        list.append(&refresh);
        pop.popup();
    }
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif")
    )
}

/// Move the files to the Trash after an explicit confirmation.
fn delete_selected(me: Media, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let win = me.ws.tabs.root().and_downcast::<gtk::Window>();
    let dlg: adw::AlertDialog = adw::AlertDialog::builder()
        .heading(format!("Delete {} item(s)?", paths.len()))
        .body("They will be moved to the Trash, not deleted permanently.")
        .close_response("cancel")
        .build();
    dlg.add_response("cancel", "Cancel");
    dlg.add_response("trash", "Move to Trash");
    dlg.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
    dlg.set_default_response(Some("cancel"));
    dlg.choose(win.as_ref(), gio::Cancellable::NONE, move |answer| {
        if answer.as_str() != "trash" {
            return;
        }
        let mut moved = 0;
        for p in paths.iter() {
            me.cache.borrow_mut().remove(p);
            match trash(p) {
                Ok(()) => moved += 1,
                Err(e) => me.ws.append_log(&format!("[mtb] {}: {e}", p.display())),
            }
        }
        me.ws.append_log(&format!("[mtb] moved {moved} item(s) to the Trash"));
        me.reload();
    });
}

/// Move `path` to the freedesktop trash (`~/.local/share/Trash`) by hand.
///
/// `gio::File::trash()` is not usable here: without a session bus (or a trash portal) gio falls
/// back to an unlink that silently deletes the file for good — unacceptable for a delete action.
fn trash(path: &Path) -> Result<(), String> {
    let data = glib::user_data_dir().join("Trash");
    let files = data.join("files");
    let info = data.join("info");
    std::fs::create_dir_all(&files).map_err(|e| format!("{e}"))?;
    std::fs::create_dir_all(&info).map_err(|e| format!("{e}"))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no file name")?;
    // "name.png", "name.png.2", "name.png.3" … like `g_trash_make_name`.
    let mut tag = name.clone();
    let mut n = 1;
    while files.join(&tag).exists() || info.join(format!("{tag}.trashinfo")).exists() {
        n += 1;
        tag = format!("{name}.{n}");
    }
    let target = files.join(&tag);
    let original = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    // ComfyUI's folders are often on another drive (`/media/...`), where rename(2) cannot work:
    // fall back to a copy + unlink (18 = EXDEV), still leaving a restorable trash entry behind.
    std::fs::rename(path, &target).or_else(|e| {
        if e.raw_os_error() == Some(18) {
            std::fs::copy(path, &target).and_then(|_| std::fs::remove_file(path))
        } else {
            Err(e)
        }
    })
    .map_err(|e| {
        let _ = std::fs::remove_file(&target);
        format!("{e}")
    })?;
    let when = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%dT%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
    let body = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", escape(&original), when);
    std::fs::write(info.join(format!("{tag}.trashinfo")), body).map_err(|e| format!("{e}"))?;
    Ok(())
}

/// Percent-encode everything but unreserved characters and `/`, as the trash spec requires.
fn escape(p: &Path) -> String {
    let mut out = String::new();
    for &b in p.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Ask the desktop's file manager to open the folder with `path` selected (FileManager1 D-Bus);
/// fall back to opening the containing folder when no file manager answers.
fn show_location(ws: &Workspace, path: &Path) {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.to_path_buf())
    };
    let uri = gio::File::for_path(path).uri().to_string();
    let ok = (|| -> Result<(), String> {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).map_err(|e| e.to_string())?;
        conn.call_sync(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&(vec![uri.clone()], String::new()).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = ok {
        ws.append_log(&format!("[mtb] file manager: {e}"));
        let _ = gtk::UriLauncher::new(&gio::File::for_path(dir).uri()).launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
    }
}
