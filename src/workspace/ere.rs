// SPDX-License-Identifier: GPL-3.0-only
//! EreNodes actions: tag edits write `properties._tagDataJSON`, then the node's generated
//! prompt (`widgets_values[0]`) is rebuilt, fetching tag-group files from the server.

use super::*;
use crate::erenodes;
use serde_json::{json, Value};
use std::collections::HashMap;

fn node_tags(d: &Doc, id: &str) -> Option<(Value, String)> {
    let n = graph_ref(d).nodes.iter().find(|n| n.id == id)?;
    let sep = n.properties["_tagSeparator"].as_str().filter(|s| !s.is_empty()).unwrap_or(", ").replacen("\\n", "\n", 1);
    Some((erenodes::tags(&n.properties), sep))
}

fn split_ext(path: &str) -> (String, String) {
    match path.rfind('.') {
        Some(i) if i > path.rfind('/').map_or(0, |s| s + 1) => (path[..i].to_owned(), path[i..].to_owned()),
        _ => (path.to_owned(), String::new()),
    }
}

impl Workspace {
    /// Entry point for every edit of an Ere cloud widget (tag array or `{op:…}`).
    pub(super) fn on_ere(&self, doc: &DocRef, id: &str, v: Value) {
        let at = v["at"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect::<Vec<_>>()).unwrap_or_default();
        match v["op"].as_str() {
            Some("convert") => {
                let ty = v["type"].as_str().unwrap_or_default().to_owned();
                self.sync_doc(doc);
                if let Some(n) = graph_mut(&mut doc.borrow_mut()).nodes.iter_mut().find(|n| n.id == id) {
                    n.ty = ty;
                }
                self.edited(doc);
            }
            Some("add") => self.ere_add_menu(doc, id, &at),
            Some("load") => self.ere_load(doc, id, &at),
            _ => self.ere_commit(doc, id, v),
        }
    }

    fn ere_commit(&self, doc: &DocRef, id: &str, v: Value) {
        self.sync_doc(doc);
        let ok = {
            let mut d = doc.borrow_mut();
            let path = d.path.clone();
            graph_view::apply_widget(&mut d.wf, &path, id, "ere", v, "")
        };
        if ok {
            self.ere_regen(doc, id);
            self.edited(doc);
        }
    }

    /// Rebuild the prompt text (group tags expand through `/erenodes/get_tag_group`).
    fn ere_regen(&self, doc: &DocRef, id: &str) {
        let Some((tags, sep)) = node_tags(&doc.borrow(), id) else { return };
        let files: Vec<String> = tags.as_array().into_iter().flatten().filter(|t| erenodes::active(t) && erenodes::kind(t) == "group").map(erenodes::group_file).collect();
        let client = self.client.borrow().clone();
        let (me, doc, id) = (self.clone(), doc.clone(), id.to_owned());
        glib::spawn_future_local(async move {
            let got: HashMap<String, Value> = match client {
                Some(c) if !files.is_empty() => gio::spawn_blocking(move || {
                    files.into_iter().filter_map(|f| c.get(&format!("/erenodes/get_tag_group?filename={}", api::percent_encode(&f))).ok().map(|v| (f, v))).collect()
                })
                .await
                .unwrap_or_default(),
                _ => HashMap::new(),
            };
            let text = erenodes::build_text(&tags, &sep, &|f| got.get(f).cloned());
            {
                let mut d = doc.borrow_mut();
                if let Some(n) = graph_mut(&mut d).nodes.iter_mut().find(|n| n.id == id) {
                    if !n.widgets_values.is_array() {
                        n.widgets_values = json!([]);
                    }
                    let a = n.widgets_values.as_array_mut().expect("array");
                    if a.is_empty() {
                        a.push(Value::Null);
                    }
                    a[0] = Value::String(text);
                }
            }
            me.set_dirty(&doc, true);
        });
    }

    fn ere_push(&self, doc: &DocRef, id: &str, tag: Value) {
        let Some((tags, _)) = node_tags(&doc.borrow(), id) else { return };
        let mut a = tags.as_array().cloned().unwrap_or_default();
        a.push(tag);
        self.ere_commit(doc, id, Value::Array(a));
    }

    fn ere_add_menu(&self, doc: &DocRef, id: &str, at: &[f64]) {
        let (x, y) = (at.first().copied().unwrap_or(0.0), at.get(1).copied().unwrap_or(0.0) + at.get(3).copied().unwrap_or(0.0));
        let area = doc.borrow().canvas.area.clone();
        let loras: Vec<String> = self.registry.borrow().get("LoraLoader").and_then(|d| d.inputs.iter().find(|i| i.name == "lora_name")).map(|i| match &i.kind {
            registry::Kind::Combo(o) => o.clone(),
            _ => vec![],
        }).unwrap_or_default();
        let rect = [x as i32, y as i32, 1, 1];
        let mut entries = Vec::new();
        {
            let (me, d, i) = (self.clone(), doc.clone(), id.to_owned());
            entries.push(item("Tag…", move || {
                let (me2, d2, i2) = (me.clone(), d.clone(), i.clone());
                me.text_dialog("Add Tag", "", move |t| {
                    let t = t.trim();
                    if !t.is_empty() {
                        me2.ere_push(&d2, &i2, json!({"name": t, "type": "tag", "active": true}));
                    }
                });
            }));
        }
        {
            let (me, d, i, a) = (self.clone(), doc.clone(), id.to_owned(), area.clone());
            entries.push(item("LoRA…", move || {
                let (me2, d2, i2) = (me.clone(), d.clone(), i.clone());
                crate::canvas::widgets::edit_combo(&a, rect, &loras, "", None, move |sel| {
                    if let Some(l) = sel {
                        let (name, ext) = split_ext(&l);
                        me2.ere_push(&d2, &i2, json!({"name": name, "type": "lora", "extension": ext, "strength": 1.0, "active": true}));
                    }
                });
            }));
        }
        {
            let (me, d, i) = (self.clone(), doc.clone(), id.to_owned());
            entries.push(item("Tag Group…", move || {
                let (me2, d2, i2) = (me.clone(), d.clone(), i.clone());
                me.pick_group(&d, rect, move |file| {
                    let (name, ext) = split_ext(&file);
                    me2.ere_push(&d2, &i2, json!({"name": name, "type": "group", "extension": ext, "active": true}));
                });
            }));
        }
        self.show_menu(&area, x, y, entries);
    }

    /// "Load Tag Group": replace the node's tags with a group file's tags.
    fn ere_load(&self, doc: &DocRef, id: &str, at: &[f64]) {
        let rect = [at.first().copied().unwrap_or(0.0) as i32, (at.get(1).copied().unwrap_or(0.0) + at.get(3).copied().unwrap_or(0.0)) as i32, 1, 1];
        let (me, d, i) = (self.clone(), doc.clone(), id.to_owned());
        self.pick_group(doc, rect, move |file| {
            let Some(c) = me.client.borrow().clone() else { return };
            let (me, d, i) = (me.clone(), d.clone(), i.clone());
            glib::spawn_future_local(async move {
                let f = file.clone();
                match gio::spawn_blocking(move || c.get(&format!("/erenodes/get_tag_group?filename={}", api::percent_encode(&f)))).await {
                    Ok(Ok(v)) if v.is_array() => me.ere_commit(&d, &i, v),
                    Ok(Err(e)) => me.append_log(&format!("[erenodes] {file}: {e}")),
                    _ => {}
                }
            });
        });
    }

    /// Ask the server for the tag-group files and let the user pick one.
    fn pick_group(&self, doc: &DocRef, rect: [i32; 4], done: impl Fn(String) + 'static) {
        let Some(c) = self.client.borrow().clone() else { return };
        let area = doc.borrow().canvas.area.clone();
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || c.get("/erenodes/list_tag_groups?path=")).await {
                Ok(Ok(v)) => {
                    let files: Vec<String> = v.as_array().into_iter().flatten().filter(|e| e["type"] == "file").filter_map(|e| e["name"].as_str().map(str::to_owned)).collect();
                    crate::canvas::widgets::edit_combo(&area, rect, &files, "", None, move |sel| {
                        if let Some(f) = sel {
                            done(f);
                        }
                    });
                }
                Ok(Err(e)) => me.append_log(&format!("[erenodes] {e}")),
                Err(_) => {}
            }
        });
    }
}
