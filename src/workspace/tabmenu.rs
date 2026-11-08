// SPDX-License-Identifier: GPL-3.0-only
//! Workflow tab right-click menu (mirrors the ComfyUI frontend's tab menu), the actions
//! behind it, drag-and-drop of workflows/images, and unsaved-changes handling on close.

use super::*;

/// Is Shift held right now? (frontend: shift = skip the "unsaved changes" prompt)
pub fn shift_down() -> bool {
    gtk::gdk::Display::default()
        .and_then(|d| d.default_seat())
        .and_then(|s| s.keyboard())
        .is_some_and(|k| k.modifier_state().contains(gtk::gdk::ModifierType::SHIFT_MASK))
}

type Done = Option<Rc<dyn Fn(bool)>>;

pub(super) fn stem(file: &str) -> String {
    let f = file.strip_prefix("temp:").and_then(|r| r.split_once(':')).map_or(file, |(_, n)| n);
    f.rsplit('/').next().unwrap_or(f).trim_end_matches(".json").to_string()
}

impl Workspace {
    /// Right-click on a tab → menu; Shift-close, drops, close-guard.
    pub(super) fn setup_tabs(&self, bar: &adw::TabBar) {
        let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (me, b) = (self.clone(), bar.clone());
        click.connect_pressed(move |g, _, x, y| {
            let Some(page) = page_at(&b, x, y) else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            me.tabs.set_selected_page(&page);
            me.tab_menu(&b, x, y, &page);
        });
        bar.add_controller(click);

        // Close guard: unsaved changes ask, unless Shift is held.
        let me = self.clone();
        self.tabs.connect_close_page(move |tv, page| {
            let doc = me.doc_of(page);
            match doc {
                Some(d) if d.borrow().dirty && !shift_down() => {
                    let (tv2, page2, d2, me2) = (tv.clone(), page.clone(), d.clone(), me.clone());
                    let dlg = adw::AlertDialog::builder().heading("Save changes?").body(format!("“{}” has unsaved changes.", stem(&d.borrow().file))).build();
                    dlg.add_responses(&[("cancel", "Cancel"), ("discard", "Discard"), ("save", "Save")]);
                    dlg.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
                    dlg.set_response_appearance("save", adw::ResponseAppearance::Suggested);
                    dlg.set_default_response(Some("save"));
                    dlg.set_close_response("cancel");
                    dlg.connect_response(None, move |_, r| match r {
                        "discard" => tv2.close_page_finish(&page2, true),
                        "save" => {
                            let (tv3, p3) = (tv2.clone(), page2.clone());
                            me2.save_doc(&d2, Some(Rc::new(move |ok| tv3.close_page_finish(&p3, ok))));
                        }
                        _ => tv2.close_page_finish(&page2, false),
                    });
                    dlg.present(Some(&me.root));
                }
                _ => tv.close_page_finish(page, true),
            }
            glib::Propagation::Stop
        });

        // Drop a workflow .json or a ComfyUI image onto the canvas → temporary tab.
        let drop = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        let me = self.clone();
        drop.connect_drop(move |_, v, _, _| {
            let Ok(list) = v.get::<gtk::gdk::FileList>() else { return false };
            for f in list.files() {
                me.open_dropped(&f);
            }
            true
        });
        self.tabs.add_controller(drop);
    }

    fn open_dropped(&self, f: &gio::File) {
        self.open_dropped_at(f, self.viewport_centre())
    }

    /// Canvas position at the middle of the visible viewport (where a drop lands by default).
    fn viewport_centre(&self) -> (f64, f64) {
        let Some(doc) = self.active_doc() else { return (0.0, 0.0) };
        let d = doc.borrow();
        let area = &d.canvas.area;
        let (zoom, off) = d.canvas.view();
        ((area.width() as f64 / 2.0 - off.0) / zoom, (area.height() as f64 / 2.0 - off.1) / zoom)
    }

    /// Same as [`open_dropped`](Self::open_dropped) but lands images at `pos`.
    pub(super) fn open_dropped_at(&self, f: &gio::File, pos: (f64, f64)) {
        let Some(path) = f.path() else { return };
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped".into());
        let Ok(bytes) = std::fs::read(&path) else { return self.append_log(&format!("[drop] cannot read {}", path.display())) };
        let text = if crate::pngmeta::is_png(&bytes) {
            match crate::pngmeta::workflow(&bytes) {
                Some(t) => t,
                None => {
                    // No workflow inside: drop it into a preloaded Load Image node instead.
                    self.append_log(&format!("[drop] {name}: no workflow metadata → Load Image"));
                    return self.insert_image_node_at(&path, pos);
                }
            }
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        match serde_json::from_str(&text).map_err(|e| e.to_string()).and_then(|v| workflow::Workflow::from_json(v).map_err(|e| e.to_string())) {
            Ok(wf) => self.open_temp(&name, wf),
            Err(e) => self.append_log(&format!("[drop] {name}: {e}")),
        }
    }

    /// Open `wf` in an unsaved, never-persisted tab.
    pub(super) fn open_temp(&self, name: &str, wf: workflow::Workflow) {
        let n = self.temp_seq.get() + 1;
        self.temp_seq.set(n);
        self.open_doc(format!("temp:{n}:{name}.json"), wf, true);
    }

    fn doc_of(&self, page: &adw::TabPage) -> Option<DocRef> {
        self.docs.borrow().iter().find(|d| &d.borrow().page == page).cloned()
    }

    fn tab_menu(&self, bar: &adw::TabBar, x: f64, y: f64, page: &adw::TabPage) {
        let Some(doc) = self.doc_of(page) else { return };
        let (n, pos) = (self.tabs.n_pages(), self.tabs.page_position(page));
        let (temp, file) = {
            let d = doc.borrow();
            (d.temp, d.file.clone())
        };
        let marked = self.bookmarks.borrow().contains(&file);
        let me = |f: fn(&Workspace, &DocRef)| {
            let (w, d) = (self.clone(), doc.clone());
            move || f(&w, &d)
        };
        let close = |which: u8| {
            let (w, p) = (self.clone(), page.clone());
            move || w.close_tabs(which, &p)
        };
        let entries = vec![
            item_i("cg-document-edit-symbolic", "Rename", !temp, me(Workspace::rename_dialog)),
            item_i("cg-edit-copy-symbolic", "Duplicate", true, me(Workspace::duplicate_tab)),
            item_i("cg-user-bookmarks-symbolic", if marked { "Remove from Bookmarks" } else { "Add to Bookmarks" }, !temp, me(Workspace::toggle_bookmark)),
            Entry::Line,
            item_i("cg-document-save-symbolic", "Save", true, me(|w, d| w.save_doc(d, None))),
            item_i("cg-document-save-as-symbolic", "Save As", true, me(|w, d| w.save_as_dialog(d, None))),
            Entry::Line,
            item_i("cg-document-send-symbolic", "Export", true, me(|w, d| w.export(d, false))),
            item_i("cg-document-send-symbolic", "Export (API)", true, me(|w, d| w.export(d, true))),
            Entry::Line,
            item_i("cg-user-trash-symbolic", "Clear Workflow", true, me(Workspace::clear_workflow)),
            Entry::Line,
            item_i("cg-window-close-symbolic", "Close Tab", true, close(0)),
            item_i("cg-window-close-symbolic", "Close Tabs to Left", pos > 0, close(1)),
            item_i("cg-window-close-symbolic", "Close Tabs to Right", pos + 1 < n, close(2)),
            item_i("cg-window-close-symbolic", "Close Other Tabs", n > 1, close(3)),
        ];
        self.show_menu(bar, x, y, entries);
    }

    /// 0 = this tab, 1 = to the left, 2 = to the right, 3 = all others.
    fn close_tabs(&self, which: u8, page: &adw::TabPage) {
        let pos = self.tabs.page_position(page);
        let pages: Vec<adw::TabPage> = (0..self.tabs.n_pages())
            .filter(|&i| match which {
                0 => i == pos,
                1 => i < pos,
                2 => i > pos,
                _ => i != pos,
            })
            .map(|i| self.tabs.nth_page(i))
            .collect();
        for p in pages {
            self.tabs.close_page(&p);
        }
        self.schedule_session();
    }

    fn duplicate_tab(&self, doc: &DocRef) {
        self.sync_doc(doc);
        let (name, json) = {
            let d = doc.borrow();
            (stem(&d.file), d.wf.to_json())
        };
        match workflow::Workflow::from_json(json) {
            Ok(wf) => self.open_temp(&format!("{name} (Copy)"), wf),
            Err(e) => self.append_log(&format!("[duplicate] {e}")),
        }
    }

    pub(super) fn name_dialog(&self, heading: &str, ok: &str, initial: &str, f: impl Fn(String) + 'static) {
        let dlg = adw::AlertDialog::builder().heading(heading).build();
        let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
        dlg.set_extra_child(Some(&entry));
        dlg.add_responses(&[("cancel", "Cancel"), ("ok", ok)]);
        dlg.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dlg.set_default_response(Some("ok"));
        dlg.set_close_response("cancel");
        let e2 = entry.clone();
        dlg.connect_response(None, move |_, r| {
            let t = e2.text().trim().to_string();
            if r == "ok" && !t.is_empty() {
                f(t);
            }
        });
        dlg.present(Some(&self.root));
        entry.grab_focus();
    }

    fn rename_dialog(&self, doc: &DocRef) {
        let old = doc.borrow().file.clone();
        let (me, d) = (self.clone(), doc.clone());
        self.name_dialog("Rename Workflow", "Rename", &stem(&old), move |name| {
            let dir = old.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
            let new = format!("{dir}{}", with_ext(&name));
            if new == old {
                return;
            }
            let Some(client) = me.client.borrow().clone() else { return me.append_log("[rename] not connected") };
            let (me, d, old) = (me.clone(), d.clone(), old.clone());
            glib::spawn_future_local(async move {
                let (o, n) = (old.clone(), new.clone());
                match gio::spawn_blocking(move || client.move_workflow(&o, &n)).await {
                    Ok(Ok(())) => {
                        {
                            let mut bm = me.bookmarks.borrow_mut();
                            if bm.remove(&old) {
                                bm.insert(new.clone());
                            }
                        }
                        d.borrow_mut().file = new.clone();
                        retitle(&d.borrow());
                        me.append_log(&format!("[rename] {old} → {new}"));
                        me.refresh_workflows();
                    }
                    Ok(Err(e)) => me.append_log(&format!("[rename] {e}")),
                    Err(_) => me.append_log("[rename] worker panicked"),
                }
            });
        });
    }

    fn toggle_bookmark(&self, doc: &DocRef) {
        let file = doc.borrow().file.clone();
        {
            let mut bm = self.bookmarks.borrow_mut();
            if !bm.remove(&file) {
                bm.insert(file);
            }
        }
        self.store_bookmarks();
        self.refresh_workflows();
    }

    /// Persist bookmarks like the frontend: `workflows/.index.json` → `{"favorites": [...]}`.
    fn store_bookmarks(&self) {
        let Some(client) = self.client.borrow().clone() else { return };
        let mut favs: Vec<String> = self.bookmarks.borrow().iter().cloned().collect();
        favs.sort();
        let body = serde_json::json!({ "favorites": favs }).to_string();
        let me = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(Err(e)) = gio::spawn_blocking(move || client.save_workflow(".index.json", body.as_bytes())).await {
                me.append_log(&format!("[bookmarks] {e}"));
            }
        });
    }

    /// Save to the doc's file; temporary docs go through Save As. `done(ok)` runs afterwards.
    pub(super) fn save_doc(&self, doc: &DocRef, done: Done) {
        if doc.borrow().temp {
            return self.save_as_dialog(doc, done);
        }
        let file = doc.borrow().file.clone();
        self.write_doc(doc, file, done);
    }

    fn write_doc(&self, doc: &DocRef, file: String, done: Done) {
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[save] not connected");
            return done.iter().for_each(|f| f(false));
        };
        self.sync_doc(doc);
        let body = doc.borrow().wf.to_json().to_string();
        let (me, doc) = (self.clone(), doc.clone());
        self.append_log(&format!("[save] {file}"));
        glib::spawn_future_local(async move {
            let f = file.clone();
            let ok = match gio::spawn_blocking(move || client.save_workflow(&f, body.as_bytes())).await {
                Ok(Ok(())) => true,
                Ok(Err(e)) => {
                    me.append_log(&format!("[save] {e}"));
                    false
                }
                Err(_) => {
                    me.append_log("[save] worker panicked");
                    false
                }
            };
            if ok {
                let renamed = {
                    let mut d = doc.borrow_mut();
                    let r = d.file != file;
                    d.file = file;
                    d.temp = false;
                    r
                };
                if renamed {
                    me.refresh_workflows();
                }
                me.append_log("[save] saved");
                doc.borrow_mut().dirty = true; // force set_dirty to retitle
                me.set_dirty(&doc, false);
                me.schedule_session();
                doc.borrow().page.set_tooltip(&doc.borrow().file);
            }
            if let Some(f) = done {
                f(ok);
            }
        });
    }

    pub(super) fn save_as_dialog(&self, doc: &DocRef, done: Done) {
        let cur = doc.borrow().file.clone();
        let (me, d) = (self.clone(), doc.clone());
        let done2 = done.clone();
        let dlg = adw::AlertDialog::builder().heading("Save As").build();
        let entry = gtk::Entry::builder().text(stem(&cur)).activates_default(true).build();
        dlg.set_extra_child(Some(&entry));
        dlg.add_responses(&[("cancel", "Cancel"), ("ok", "Save")]);
        dlg.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dlg.set_default_response(Some("ok"));
        dlg.set_close_response("cancel");
        let e2 = entry.clone();
        dlg.connect_response(None, move |_, r| {
            let name = e2.text().trim().to_string();
            if r != "ok" || name.is_empty() {
                return done2.iter().for_each(|f| f(false));
            }
            let new = with_ext(&name);
            let exists = me.files.borrow().iter().any(|f| f.eq_ignore_ascii_case(&new)) && new != cur;
            if !exists {
                return me.write_doc(&d, new, done2.clone());
            }
            let ow = adw::AlertDialog::builder().heading("Overwrite?").body(format!("“{}” already exists.", stem(&new))).build();
            ow.add_responses(&[("cancel", "Cancel"), ("ok", "Overwrite")]);
            ow.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
            ow.set_close_response("cancel");
            let (me2, d2, done3) = (me.clone(), d.clone(), done2.clone());
            ow.connect_response(None, move |_, r| {
                if r == "ok" {
                    me2.write_doc(&d2, new.clone(), done3.clone());
                } else {
                    done3.iter().for_each(|f| f(false));
                }
            });
            ow.present(Some(&me.root));
        });
        dlg.present(Some(&self.root));
        entry.grab_focus();
    }

    /// Save the workflow (or its API prompt) to a user-chosen file.
    fn export(&self, doc: &DocRef, api: bool) {
        self.sync_doc(doc);
        let (name, text) = {
            let d = doc.borrow();
            let v = if api { prompt::build(&d.wf, &self.registry.borrow()).0 } else { d.wf.to_json() };
            let suffix = if api { "_api" } else { "" };
            (format!("{}{suffix}.json", stem(&d.file)), serde_json::to_string_pretty(&v).unwrap_or_default())
        };
        let dialog = gtk::FileDialog::builder().title(if api { "Export (API)" } else { "Export" }).initial_name(name).build();
        let parent = self.tabs.root().and_downcast::<gtk::Window>();
        let me = self.clone();
        dialog.save(parent.as_ref(), gio::Cancellable::NONE, move |res| {
            let Some(path) = res.ok().and_then(|f| f.path()) else { return };
            match std::fs::write(&path, text) {
                Ok(()) => me.append_log(&format!("[export] {}", path.display())),
                Err(e) => me.append_log(&format!("[export] {}: {e}", path.display())),
            }
        });
    }

    pub(super) fn clear_workflow(&self, doc: &DocRef) {
        let dlg = adw::AlertDialog::builder().heading("Clear workflow?").body("This removes every node, link and group from the graph being shown.").build();
        dlg.add_responses(&[("cancel", "Cancel"), ("ok", "Clear")]);
        dlg.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
        dlg.set_close_response("cancel");
        let (me, d) = (self.clone(), doc.clone());
        let in_subgraph = !d.borrow().path.is_empty();
        dlg.connect_response(None, move |_, r| {
            if r != "ok" {
                return;
            }
            {
                let mut dm = d.borrow_mut();
                let g = graph_mut(&mut dm);
                // Inside a subgraph only the non-IO nodes go: clearing the IO nodes would break
                // the definition (same rule as the web UI's Comfy.ClearWorkflow).
                if in_subgraph {
                    g.nodes.retain(|n| n.id == workflow::SUBGRAPH_INPUT_ID || n.id == workflow::SUBGRAPH_OUTPUT_ID);
                } else {
                    g.nodes.clear();
                }
                g.links.clear();
                g.groups.clear();
            }
            me.render_doc(&d);
            me.set_dirty(&d, true);
        });
        dlg.present(Some(&self.root));
    }
}

fn with_ext(name: &str) -> String {
    if name.to_lowercase().ends_with(".json") { name.to_string() } else { format!("{name}.json") }
}

/// Re-derive the tab title from `file` + dirty flag.
pub(super) fn retitle(d: &Doc) {
    let name = stem(&d.file);
    d.page.set_title(&if d.dirty { format!("{name} \u{2022}") } else { name });
}

/// The `AdwTab` page under (x, y) of the tab bar.
fn page_at(bar: &adw::TabBar, x: f64, y: f64) -> Option<adw::TabPage> {
    let mut w = bar.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(cur) = w {
        if cur.find_property("page").is_some() {
            if let Some(p) = cur.property::<Option<adw::TabPage>>("page") {
                return Some(p);
            }
        }
        w = cur.parent();
    }
    None
}
