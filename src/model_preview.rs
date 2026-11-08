//! Preview images for model files (LoRAs, checkpoints, embeddings, …), shown on hover in every
//! combo list. Generic like the web UI: no per-node code — any option that names a file in one of
//! the backend's model folders gets a preview, from whichever provider has one:
//!   1. LoRA Manager (`/api/lm/{loras|checkpoints|embeddings}/preview-url`) when installed,
//!   2. ComfyUI core (`/experiment/models/preview/{folder}/{path_index}/{name}`: sibling
//!      `name.png` / `name.preview.png` or the safetensors cover image).
//! Blocking; call from `gio::spawn_blocking`.

use crate::api::{percent_encode, Client};
use std::collections::HashMap;

/// A model file the backend knows: folder name (`loras`), path index, relative name.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub folder: String,
    pub index: i64,
    pub name: String,
}

/// Option text → model file. Keyed by the relative name *and* the bare stem, since some widgets
/// (LoRA Manager tags, `<lora:x>`) show names without folder or extension.
#[derive(Default)]
pub struct Index(HashMap<String, Entry>);

pub fn stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.rsplit_once('.').map_or(base, |(s, _)| s)
}

impl Index {
    /// `GET /experiment/models` + one `GET /experiment/models/{folder}` per folder (~2 s on a big
    /// install). Earlier folders win on name clashes (`loras` before `lora_gguf`).
    pub fn load(client: &Client) -> Self {
        let mut map = HashMap::new();
        let folders = client.get("/experiment/models").unwrap_or_default();
        for f in folders.as_array().into_iter().flatten() {
            let Some(folder) = f["name"].as_str() else { continue };
            let Ok(files) = client.get(&format!("/experiment/models/{}", percent_encode(folder))) else { continue };
            for m in files.as_array().into_iter().flatten() {
                let Some(name) = m["name"].as_str() else { continue };
                let e = Entry { folder: folder.to_owned(), index: m["pathIndex"].as_i64().unwrap_or(0), name: name.to_owned() };
                map.entry(stem(name).to_owned()).or_insert_with(|| e.clone());
                map.entry(name.to_owned()).or_insert(e);
            }
        }
        Index(map)
    }

    pub fn get(&self, option: &str) -> Option<&Entry> {
        self.0.get(option).or_else(|| self.0.get(stem(option)))
    }
}

/// LoRA Manager's model-type prefix for a ComfyUI folder.
fn lm_prefix(folder: &str) -> Option<&'static str> {
    match folder {
        "loras" | "lora_gguf" => Some("loras"),
        "checkpoints" | "diffusion_models" | "unet" | "unet_gguf" => Some("checkpoints"),
        "embeddings" => Some("embeddings"),
        _ => None,
    }
}

fn is_video(url: &str) -> bool {
    let u = url.to_lowercase(); // LoRA Manager puts the file path in the query string
    [".mp4", ".webm", ".mov", ".mkv"].iter().any(|x| u.ends_with(x))
}

/// Image bytes for `e`, or `None` when no provider has one.
pub fn fetch(client: &Client, e: &Entry) -> Option<Vec<u8>> {
    if let Some(prefix) = lm_prefix(&e.folder) {
        let q = format!("/api/lm/{prefix}/preview-url?name={}&license_flags=true", percent_encode(stem(&e.name)));
        if let Ok(v) = client.get(&q) {
            if let Some(url) = v["preview_url"].as_str().filter(|u| v["success"] == true && !u.is_empty() && !is_video(u)) {
                if let Some(b) = client.view_query(url).ok().filter(|b| !b.is_empty()) {
                    return Some(b);
                }
            }
        }
    }
    let path = e.name.split('/').map(percent_encode).collect::<Vec<_>>().join("/");
    client.view_query(&format!("/experiment/models/preview/{}/{}/{path}", percent_encode(&e.folder), e.index)).ok().filter(|b| !b.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_names_and_stems() {
        let e = |f: &str, n: &str| Entry { folder: f.into(), index: 0, name: n.into() };
        let mut ix = Index::default();
        for x in [e("loras", "SDXL/a.safetensors"), e("lora_gguf", "SDXL/a.safetensors")] {
            ix.0.entry(stem(&x.name).to_owned()).or_insert_with(|| x.clone());
            ix.0.entry(x.name.clone()).or_insert(x);
        }
        assert_eq!(ix.get("SDXL/a.safetensors").unwrap().folder, "loras");
        assert_eq!(ix.get("a").unwrap().name, "SDXL/a.safetensors");
        assert_eq!(ix.get("other/a.ckpt").unwrap().name, "SDXL/a.safetensors"); // stem fallback
        assert!(ix.get("Select the LoRA to add to the text").is_none());
        assert!(is_video("/api/lm/previews?path=x.mp4") && !is_video("/x.jpeg"));
    }
}
