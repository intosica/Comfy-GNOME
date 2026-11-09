// SPDX-License-Identifier: GPL-3.0-only
//! Mask painting for Load Image ("Open in Mask Editor") and the Image BBox Layout node
//! (upload button, box + mask editor). The painter itself lives in `crate::maskedit`.

use super::*;
use crate::maskedit::{self, MaskSrc, Setup};
use serde_json::Value;
use std::collections::HashMap;

impl Workspace {
    /// `bbox:*` widget keys of Image BBox Layout. Returns false for other keys.
    pub(super) fn on_bbox(&self, doc: &DocRef, id: &str, key: &str, v: Value) -> bool {
        let Some(op) = key.strip_prefix("bbox:") else { return false };
        let Some(slots) = self.bbox_slots(doc, id) else { return true };
        let idx = |k: &str| slots.get(k).map(|s| s.idx);
        match op {
            "upload" => {
                let (me, d, id) = (self.clone(), doc.clone(), id.to_owned());
                let (image, mask) = (idx("image"), idx("mask_file"));
                self.pick_and_upload(move |value| {
                    // A new picture invalidates the old mask (different size / content).
                    let mut set = vec![(image, Value::from(value))];
                    set.push((mask, Value::from("")));
                    me.set_values(&d, &id, &set);
                });
            }
            "show_prompt" => {
                let on = v.as_bool().unwrap_or(false);
                self.sync_doc(doc);
                {
                    let mut d = doc.borrow_mut();
                    let Some(n) = graph_mut(&mut d).nodes.iter_mut().find(|n| n.id == id) else { return true };
                    n.properties["show_prompt"] = Value::Bool(on);
                }
                // Hidden prompt = no prompt (the web extension clears it too).
                let clear = if on { Vec::new() } else { vec![(idx("prompt"), Value::from(""))] };
                self.set_values(doc, id, &clear);
            }
            "edit" => self.edit_bbox(doc, id, &slots),
            _ => {}
        }
        true
    }

    pub(super) fn bbox_slots_pub(&self, doc: &DocRef, id: &str) -> bool {
        self.bbox_slots(doc, id).is_some()
    }

    fn bbox_slots(&self, doc: &DocRef, id: &str) -> Option<HashMap<String, prompt::WSlot>> {
        let d = doc.borrow();
        graph_view::bbox_slots(graph_ref(&d).node(id)?, &self.registry.borrow())
    }

    /// Write several `widgets_values` slots, then re-render once. `None` slots are skipped.
    fn set_values(&self, doc: &DocRef, id: &str, set: &[(Option<usize>, Value)]) {
        self.sync_doc(doc);
        {
            let mut d = doc.borrow_mut();
            let path = d.path.clone();
            for (i, v) in set {
                if let Some(i) = i {
                    graph_view::apply_widget(&mut d.wf, &path, id, &i.to_string(), v.clone(), "");
                }
            }
        }
        self.edited(doc);
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.tabs.root().and_downcast::<gtk::Window>()
    }

    /// Fetch a `/view` picture off the main thread; failures are logged and give `None`.
    fn fetch(&self, value: &str, then: impl FnOnce(Option<Vec<u8>>) + 'static) {
        let Some(client) = self.client.borrow().clone() else {
            self.append_log("[mask] not connected");
            return;
        };
        let (me, q) = (self.clone(), graph_view::image_query(value));
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || client.view_query(&q)).await {
                Ok(Ok(bytes)) => then(Some(bytes)),
                Ok(Err(e)) => {
                    me.append_log(&format!("[mask] {e}"));
                    then(None)
                }
                Err(_) => then(None),
            }
        });
    }

    fn upload_bg(&self, f: impl FnOnce(&api::Client) -> Result<String, String> + Send + 'static, then: impl FnOnce(String) + 'static) {
        let Some(client) = self.client.borrow().clone() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || f(&client)).await {
                Ok(Ok(v)) => {
                    me.append_log(&format!("[upload] {v}"));
                    then(v)
                }
                Ok(Err(e)) => me.append_log(&format!("[upload] {e}")),
                Err(_) => me.append_log("[upload] worker panicked"),
            }
        });
    }

    fn edit_bbox(&self, doc: &DocRef, id: &str, slots: &HashMap<String, prompt::WSlot>) {
        let s = |k: &str| slots.get(k).and_then(|s| s.value.as_str()).unwrap_or_default().to_owned();
        let image = s("image");
        if image.is_empty() {
            self.append_log("[mask] upload a picture first");
            return;
        }
        let boxes = maskedit::parse_boxes(&s("bbox_data"));
        let max_mp = slots.get("max_megapixels").and_then(|s| s.value.as_f64()).unwrap_or(1.0);
        let mask_file = s("mask_file");
        let (bbox_i, mask_i) = (slots.get("bbox_data").map(|s| s.idx), slots.get("mask_file").map(|s| s.idx));
        let (me, d, id) = (self.clone(), doc.clone(), id.to_owned());
        let me0 = self.clone();
        let mask_q = mask_file.clone();
        self.fetch(&image, move |img| {
            let Some(img) = img else { return };
            let open = move |mask: MaskSrc| {
                let setup = Setup { title: "Edit Boxes & Mask".into(), image: img, mask, mask_max: Some(1280), boxes: Some((boxes, max_mp)) };
                let (me2, d2, id2, mask_file2) = (me.clone(), d.clone(), id.clone(), mask_file.clone());
                let r = maskedit::open(me.parent_window().as_ref(), setup, move |out| {
                    let mut set = Vec::new();
                    if let Some(b) = out.boxes {
                        set.push((bbox_i, Value::from(maskedit::boxes_json(&b))));
                    }
                    match out.mask {
                        Some(m) if m.is_empty() => set.push((mask_i, Value::from(""))),
                        Some(m) => {
                            // Re-use the node's file name: it is overwritten in place.
                            let name = match mask_file2.rsplit('/').next().filter(|n| !n.is_empty()) {
                                Some(n) => n.split(" [").next().unwrap_or(n).to_owned(),
                                None => format!("midfr_mask_{:08x}.png", graph_view::rand_u64() as u32),
                            };
                            let png = m.png(false);
                            let (me3, d3, id3) = (me2.clone(), d2.clone(), id2.clone());
                            me2.upload_bg(move |c| c.upload_image(&name, &png), move |v: String| {
                                d3.borrow().canvas.forget_image(&graph_view::image_query(&v));
                                me3.set_values(&d3, &id3, &[(mask_i, Value::from(v))]);
                            });
                        }
                        None => {}
                    }
                    me2.set_values(&d2, &id2, &set);
                });
                if let Err(e) = r {
                    me.append_log(&format!("[mask] {e}"));
                }
            };
            if mask_q.is_empty() {
                open(MaskSrc::None);
            } else {
                // A missing mask file just starts empty.
                me0.fetch(&mask_q, move |png| open(png.map_or(MaskSrc::None, MaskSrc::Png)));
            }
        });
    }

    /// The node's `image_upload` combo (Load Image and friends): (slot index, value).
    pub(super) fn image_slot(&self, doc: &DocRef, id: &str) -> Option<(usize, String)> {
        let d = doc.borrow();
        let n = graph_ref(&d).node(id)?;
        if n.ty == "ImageBBoxLayout" {
            return None;
        }
        let reg = self.registry.borrow();
        prompt::widget_slots(n, reg.get(&n.ty)?)
            .into_iter()
            .find(|s| s.role == prompt::Role::Value && graph_view::truthy_opt(&s.input, "image_upload"))
            .and_then(|s| Some((s.idx, s.value.as_str().filter(|v| !v.is_empty())?.to_owned())))
    }

    /// Paint a mask into a Load Image picture's alpha, as the web "Open in MaskEditor" does:
    /// `/upload/mask` stores `clipspace/clipspace-mask-<ms>.png` and the node points at it.
    pub(super) fn open_mask_editor(&self, doc: &DocRef, id: &str) {
        let Some((idx, value)) = self.image_slot(doc, id) else { return };
        let (me, d, id) = (self.clone(), doc.clone(), id.to_owned());
        self.fetch(&value.clone(), move |img| {
            let Some(img) = img else { return };
            let setup = Setup { title: "Mask Editor".into(), image: img, mask: MaskSrc::ImageAlpha, mask_max: None, boxes: None };
            let (me2, d2, id2) = (me.clone(), d.clone(), id.clone());
            let r = maskedit::open(me.parent_window().as_ref(), setup, move |out| {
                let Some(m) = out.mask else { return };
                let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis());
                let (png, orig) = (m.png(true), value.clone());
                let (me3, d3, id3) = (me2.clone(), d2.clone(), id2.clone());
                me2.upload_bg(move |c| c.upload_mask(&format!("clipspace-mask-{ms}.png"), &png, &orig), move |v| {
                    me3.set_values(&d3, &id3, &[(Some(idx), Value::from(format!("{v} [input]")))]);
                });
            });
            if let Err(e) = r {
                me.append_log(&format!("[mask] {e}"));
            }
        });
    }
}
