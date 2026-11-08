// SPDX-License-Identifier: GPL-3.0-only
//! Session persistence: open tabs, unsaved drafts and viewports across launches.
//!
//! Mirrors the web UI's `Comfy.Workflow.Persist` (default on): the last tab set is
//! restored, including edits that were never saved. Drafts are plain workflow JSON,
//! so a restored tab is a normal doc (Save writes it back to its file).

use super::{DocRef, Workspace};
use crate::workflow;
use adw::glib;
use serde_json::{json, Value};
use std::path::PathBuf;

fn path() -> PathBuf {
    glib::user_config_dir().join("comfy-gnome").join("session.json")
}

impl Workspace {
    /// Coalesce the burst of changes a drag or a widget edit produces.
    pub(super) fn schedule_session(&self) {
        if !self.persist.get() || self.restoring.get() || self.session_queued.replace(true) {
            return;
        }
        let me = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            me.session_queued.set(false);
            me.save_session();
            glib::ControlFlow::Break
        });
    }

    pub(super) fn save_session(&self) {
        if !self.persist.get() || self.restoring.get() {
            return;
        }
        // Canvas moves/wires live in the canvas until synced; flush before serialising.
        for d in self.docs.borrow().iter() {
            self.sync_doc(d);
        }
        let active = self
            .tabs
            .selected_page()
            .and_then(|p| self.docs.borrow().iter().position(|d| d.borrow().page == p))
            .unwrap_or(0);
        let tabs: Vec<Value> = self
            .docs
            .borrow()
            .iter()
            .map(|d| {
                let d = d.borrow();
                let (zoom, off) = d.canvas.view();
                json!({
                    "file": d.file,
                    "temp": d.temp,
                    "dirty": d.dirty,
                    "view": [zoom, off.0, off.1],
                    "workflow": d.wf.to_json(),
                })
            })
            .collect();
        let body = json!({"version": 1, "active": active, "tabs": tabs});
        let p = path();
        let Some(dir) = p.parent() else { return };
        if let Err(e) = std::fs::create_dir_all(dir) {
            self.append_log(&format!("[session] {e}"));
            return;
        }
        let tmp = p.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, body.to_string()).and_then(|_| std::fs::rename(&tmp, &p)) {
            self.append_log(&format!("[session] {e}"));
        }
    }

    /// Reopen the saved tab set. Called after settings load, before the blank fallback.
    pub(super) fn restore_session(&self) {
        if !self.persist.get() {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path()) else { return };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            self.append_log("[session] ignoring unreadable session.json");
            return;
        };
        let Some(tabs) = v.get("tabs").and_then(Value::as_array) else { return };
        self.restoring.set(true);
        let mut restored: Vec<(DocRef, Option<(f64, f64, f64)>, bool)> = Vec::new();
        for t in tabs {
            let Some(file) = t.get("file").and_then(Value::as_str) else { continue };
            let Some(wfv) = t.get("workflow").cloned() else { continue };
            let Ok(wf) = workflow::Workflow::from_json(wfv) else { continue };
            let temp = t.get("temp").and_then(Value::as_bool).unwrap_or(true);
            let dirty = t.get("dirty").and_then(Value::as_bool).unwrap_or(false);
            let already = self.docs.borrow().iter().find(|d| d.borrow().file == file).cloned();
            self.open_doc(file.to_owned(), wf, temp);
            let doc = already.or_else(|| self.docs.borrow().iter().find(|d| d.borrow().file == file).cloned());
            let Some(doc) = doc else { continue };
            let view = t.get("view").and_then(Value::as_array).and_then(|a| {
                Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?))
            });
            restored.push((doc, view, dirty));
        }
        for (doc, view, dirty) in restored {
            if let Some((z, x, y)) = view {
                doc.borrow().canvas.set_view(z, (x, y));
            }
            if dirty {
                {
                    let mut d = doc.borrow_mut();
                    d.dirty = true;
                    super::tabmenu::retitle(&d);
                }
            }
        }
        let active = v.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        if let Some(doc) = self.docs.borrow().get(active) {
            self.tabs.set_selected_page(&doc.borrow().page);
        }
        self.restoring.set(false);
        self.append_log(&format!("[session] restored {} tab(s)", self.docs.borrow().len()));
    }
}
