// SPDX-License-Identifier: GPL-3.0-only
//! ComfyUI REST + WebSocket client.
//!
//! Blocking I/O on worker threads (`ureq`, `tungstenite`); results reach the GTK main
//! loop through `async_channel` + `glib::spawn_future_local`. Nothing here touches GTK.
//! Endpoints mirror ComfyUI_frontend `src/scripts/api.ts`.

use serde_json::Value;
use std::time::Duration;

/// Events from the `/ws` stream (ComfyUI `send_sync` messages).
#[derive(Debug, Clone)]
pub enum WsEvent {
    Connected,
    /// Lost or failed; the worker retries every 2 s.
    Disconnected(String),
    /// `status`: queue length (+ our session id on first message).
    Status { queue_remaining: i64 },
    /// `progress`: sampler step of `node`.
    Progress { value: i64, max: i64, node: Option<String> },
    /// `executing`: `node == None` means the prompt finished.
    Executing { node: Option<String>, prompt_id: Option<String> },
    ExecutionStart { prompt_id: String },
    ExecutionError { message: String },
    /// Binary preview frame (JPEG/PNG bytes, header stripped) and the node the backend
    /// wants it shown on (`display_node_id`: subgraph/custom-sampler frames bubble to the host).
    Preview { bytes: Vec<u8>, display_node: Option<String> },
    /// `executed`: output images of a finished node (`filename/subfolder/type` → `/view`).
    Executed { node: Option<String>, images: Vec<OutputImage> },
    /// Anything else (custom-node events, executed, progress_state, …): raw type + data.
    Other { kind: String, data: Value },
}

/// One `/queue` entry: `[number, prompt_id, prompt, extra_data, outputs]`.
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub number: i64,
    pub prompt_id: String,
    pub nodes: usize,
    /// Distinct class_types, first few (what the item will run).
    pub classes: String,
}

/// One entry of `executed.output.images`.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputImage {
    pub filename: String,
    pub subfolder: String,
    /// "output" or "temp".
    pub kind: String,
}

impl OutputImage {
    /// Query string for `GET /view`; also the canvas image-cache key.
    pub fn query(&self) -> String {
        format!(
            "filename={}&subfolder={}&type={}",
            percent_encode(&self.filename),
            percent_encode(&self.subfolder),
            percent_encode(&self.kind)
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    pub running: Vec<QueueItem>,
    pub pending: Vec<QueueItem>,
}

/// One finished run from `GET /history` — what the queue sidebar keeps under "Completed".
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub prompt_id: String,
    pub number: Option<i64>,
    pub images: Vec<OutputImage>,
    /// Epoch milliseconds of `execution_start` / `execution_success` (or `execution_error`).
    pub started: Option<i64>,
    pub finished: Option<i64>,
}

impl HistoryEntry {
    /// Wall time the prompt ran for, in seconds (the number the web UI shows under each row).
    pub fn duration(&self) -> Option<f64> {
        Some((self.finished? - self.started?) as f64 / 1000.0).filter(|s| *s >= 0.0)
    }

    /// The `execution_start`/`execution_success` timestamps exist in `status.messages`
    /// (`[[kind, {prompt_id, timestamp}], …]`); outputs are one `images` array per node.
    pub fn parse(prompt_id: &str, v: &Value) -> Self {
        let images = v["outputs"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(_, out)| out["images"].as_array().into_iter().flatten())
            .map(|img| OutputImage {
                filename: img["filename"].as_str().unwrap_or_default().to_string(),
                subfolder: img["subfolder"].as_str().unwrap_or_default().to_string(),
                kind: img["type"].as_str().unwrap_or("output").to_string(),
            })
            .collect();
        let stamp = |kind: &str| {
            v["status"]["messages"]
                .as_array()
                .and_then(|m| m.iter().find(|x| x[0] == kind))
                .and_then(|x| x[1]["timestamp"].as_i64())
        };
        Self {
            prompt_id: prompt_id.to_string(),
            number: v["prompt"][0].as_i64(),
            images,
            started: stamp("execution_start"),
            finished: stamp("execution_success").or_else(|| stamp("execution_error")),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    base: String,
    agent: ureq::Agent,
    pub client_id: String,
}

impl Client {
    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn new(host: &str, port: u16) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(60))
            .build();
        Self { base: format!("http://{host}:{port}"), agent, client_id: client_id() }
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.agent.get(&format!("{}{path}", self.base)).call().map_err(err)?.into_json().map_err(|e| e.to_string())
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let r = self.agent.post(&format!("{}{path}", self.base)).send_json(body).map_err(err)?;
        // Some endpoints (/interrupt, /queue) answer with an empty body.
        let text = r.into_string().map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// `GET /view?<query>` — raw image bytes of an output/preview (`OutputImage::query`).
    pub fn view_query(&self, query: &str) -> Result<Vec<u8>, String> {
        // Keys starting with '/' are full server paths (EreNodes gallery tiles).
        let url = if query.starts_with('/') { format!("{}{query}", self.base) } else { format!("{}/view?{query}", self.base) };
        let r = self.agent.get(&url).call().map_err(err)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// `POST /upload/image` (multipart): copy `bytes` into ComfyUI's input folder.
    /// Returns the value for a LoadImage widget (`subfolder/name`).
    pub fn upload_image(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        self.upload("/upload/image", name, bytes, &[("overwrite", "true")])
    }

    /// `POST /upload/mask`: the backend copies `png`'s alpha into the picture `original`
    /// (a LoadImage value) and saves the result as `clipspace/<name>` in the input folder.
    pub fn upload_mask(&self, name: &str, png: &[u8], original: &str) -> Result<String, String> {
        let (path, kind) = match original.rsplit_once(" [") {
            Some((p, k)) if k.ends_with(']') => (p, &k[..k.len() - 1]),
            _ => (original, "input"),
        };
        let (sub, file) = path.rsplit_once('/').unwrap_or(("", path));
        let r = serde_json::json!({"filename": file, "subfolder": sub, "type": kind}).to_string();
        self.upload("/upload/mask", name, png, &[("original_ref", &r), ("type", "input"), ("subfolder", "clipspace"), ("overwrite", "true")])
    }

    fn upload(&self, path: &str, name: &str, bytes: &[u8], fields: &[(&str, &str)]) -> Result<String, String> {
        let b = "----comfygnome";
        let mut body = format!("--{b}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n", name.replace('"', "_")).into_bytes();
        body.extend_from_slice(bytes);
        for (k, v) in fields {
            body.extend_from_slice(format!("\r\n--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}").as_bytes());
        }
        body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
        let r = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Content-Type", &format!("multipart/form-data; boundary={b}"))
            .send_bytes(&body)
            .map_err(err)?;
        let v: Value = r.into_json().map_err(|e| e.to_string())?;
        let name = v["name"].as_str().ok_or("upload: no name in reply")?;
        Ok(match v["subfolder"].as_str().filter(|s| !s.is_empty()) {
            Some(sub) => format!("{sub}/{name}"),
            None => name.to_owned(),
        })
    }

    /// `GET /queue`.
    pub fn queue(&self) -> Result<Queue, String> {
        let v = self.get("/queue")?;
        let list = |key: &str| -> Vec<QueueItem> {
            let entries = v[key].as_array().cloned().unwrap_or_default();
            entries
                .iter()
                .filter_map(|e| {
                    let prompt = &e[2];
                    let mut seen: Vec<&str> = Vec::new();
                    for n in prompt.as_object().into_iter().flatten() {
                        if let Some(t) = n.1["class_type"].as_str() {
                            if !seen.contains(&t) {
                                seen.push(t);
                            }
                        }
                    }
                    Some(QueueItem {
                        number: e[0].as_i64().unwrap_or(0),
                        prompt_id: e[1].as_str().unwrap_or_default().to_owned(),
                        nodes: prompt.as_object().map_or(0, |o| o.len()),
                        classes: seen.iter().take(3).copied().collect::<Vec<_>>().join(", "),
                    })
                })
                .collect()
        };
        Ok(Queue { running: list("queue_running"), pending: list("queue_pending") })
    }

    /// `GET /api/userdata?dir=workflows&recurse=true&split=false&full_info=true`.
    pub fn workflows(&self) -> Result<Vec<String>, String> {
        let v = self.get("/api/userdata?dir=workflows&recurse=true&split=false&full_info=true")?;
        Ok(v.as_array().into_iter().flatten().filter_map(|e| e["path"].as_str().map(str::to_owned)).collect())
    }

    /// `GET /api/userdata/workflows/<path>`: the saved workflow JSON.
    pub fn workflow_json(&self, file: &str) -> Result<String, String> {
        // The server route is a single `{file}` segment: "workflows/x.json" must be
        // percent-encoded whole (a literal '/' after /userdata/ is a 404).
        let url = format!("{}/api/userdata/{}", self.base, percent_encode(&format!("workflows/{file}")));
        self.agent.get(&url).call().map_err(err)?.into_string().map_err(|e| e.to_string())
    }

    /// `POST /api/userdata/workflows/<path>` with the raw JSON body (ComfyUI writes it
    /// atomically; the route reads the body verbatim, not as a JSON field).
    pub fn save_workflow(&self, file: &str, body: &[u8]) -> Result<(), String> {
        let url = format!("{}/api/userdata/{}", self.base, percent_encode(&format!("workflows/{file}")));
        self.agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_bytes(body)
            .map(drop)
            .map_err(err)
    }

    /// `POST /api/userdata/workflows%2F<src>/move/workflows%2F<dst>` (fails if `dst` exists).
    pub fn move_workflow(&self, src: &str, dst: &str) -> Result<(), String> {
        let url = format!(
            "{}/api/userdata/{}/move/{}",
            self.base,
            percent_encode(&format!("workflows/{src}")),
            percent_encode(&format!("workflows/{dst}"))
        );
        self.agent.post(&url).send_bytes(&[]).map(drop).map_err(err)
    }

    /// `POST /prompt` with an API-format prompt. Returns `prompt_id`.
    #[allow(dead_code)] // Phase 2.4
    pub fn queue_prompt(
        &self,
        prompt: Value,
        workflow: Option<Value>,
        front: bool,
        preview_method: Option<&str>,
    ) -> Result<String, String> {
        let mut body = serde_json::json!({ "client_id": self.client_id, "prompt": prompt });
        if front {
            body["front"] = true.into();
        }
        // `extra_data.preview_method` is what the server's latent_preview reads; the
        // frontend omits it for "default" so the CLI setting wins.
        let mut extra = serde_json::Map::new();
        if let Some(w) = workflow {
            extra.insert("extra_pnginfo".into(), serde_json::json!({ "workflow": w }));
        }
        if let Some(p) = preview_method.filter(|p| !p.is_empty() && *p != "default") {
            extra.insert("preview_method".into(), Value::String(p.to_owned()));
        }
        if !extra.is_empty() {
            body["extra_data"] = Value::Object(extra);
        }
        let v = self.post("/prompt", body)?;
        v["prompt_id"].as_str().map(str::to_owned).ok_or_else(|| format!("unexpected /prompt reply: {v}"))
    }

    pub fn interrupt(&self) -> Result<(), String> {
        self.post("/interrupt", serde_json::json!({})).map(drop)
    }

    /// Delete pending queue entries by prompt id.
    pub fn delete_queued(&self, ids: &[String]) -> Result<(), String> {
        self.post("/queue", serde_json::json!({ "delete": ids })).map(drop)
    }

    /// Drop every *pending* entry (the running one keeps going) — the web UI's "Clear queue".
    pub fn clear_queue(&self) -> Result<(), String> {
        self.post("/queue", serde_json::json!({ "clear": true })).map(drop)
    }

    /// The last `max_items` finished runs, oldest first.
    pub fn history(&self, max_items: usize) -> Result<Vec<HistoryEntry>, String> {
        let v = self.get(&format!("/history?max_items={max_items}"))?;
        let mut out: Vec<HistoryEntry> =
            v.as_object().into_iter().flatten().map(|(id, e)| HistoryEntry::parse(id, e)).collect();
        // The map's order is not guaranteed: sort by number, which is the queue order.
        out.sort_by_key(|e| e.number.unwrap_or(0));
        Ok(out)
    }

    /// The editor workflow a job was queued from (`prompt[3].extra_pnginfo.workflow`), for a
    /// prompt that is still queued or already finished. `None` when the client sent none.
    pub fn job_workflow(&self, prompt_id: &str) -> Result<Option<Value>, String> {
        // Both shapes hold `[number, id, prompt, extra_data, outputs]`: history under
        // `entry.prompt`, `/queue` as the entry itself.
        let extract = |e: &Value| e[3]["extra_pnginfo"]["workflow"].as_object().cloned();
        if let Some(e) = self.get(&format!("/history/{prompt_id}"))?.get(prompt_id) {
            if let Some(w) = extract(&e["prompt"]) {
                return Ok(Some(w.into()));
            }
        }
        let q = self.get("/queue")?;
        for key in ["queue_running", "queue_pending"] {
            for e in q[key].as_array().into_iter().flatten() {
                if e[1].as_str() == Some(prompt_id) {
                    if let Some(w) = extract(e) {
                        return Ok(Some(w.into()));
                    }
                }
            }
        }
        Ok(None)
    }

    /// `GET /history/<prompt_id>` — `None` while the entry is not there yet.
    pub fn history_entry(&self, prompt_id: &str) -> Result<Option<HistoryEntry>, String> {
        let v = self.get(&format!("/history/{prompt_id}"))?;
        Ok(v.get(prompt_id).map(|e| HistoryEntry::parse(prompt_id, e)))
    }

    /// Run the `/ws` reader on its own thread forever, reconnecting on failure.
    /// Every setting the backend reports (`GET /api/settings`), e.g. `pysssss.SnapToGrid`.
    /// Extensions register theirs at import, so this is how their panels reach us.
    pub fn settings(&self) -> Result<std::collections::BTreeMap<String, Value>, String> {
        let v = self.get("/api/settings")?;
        Ok(v.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default())
    }

    /// Persist one setting (`POST /api/settings/{key}`); the reply body is empty.
    pub fn set_setting(&self, key: &str, value: Value) -> Result<(), String> {
        self.post(&format!("/api/settings/{}", percent_encode(key)), value)?;
        Ok(())
    }

    pub fn spawn_ws(&self, tx: async_channel::Sender<WsEvent>) {
        let url = format!("{}/ws?clientId={}", self.base.replacen("http", "ws", 1), self.client_id);
        std::thread::Builder::new()
            .name("comfy-ws".into())
            .spawn(move || loop {
                let reason = match ws_session(&url, &tx) {
                    Ok(()) => "closed".to_string(),
                    Err(e) => e,
                };
                if tx.send_blocking(WsEvent::Disconnected(reason)).is_err() {
                    return; // UI gone
                }
                std::thread::sleep(Duration::from_secs(2));
            })
            .expect("spawn ws thread");
    }
}

fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => format!("HTTP {code}: {}", r.into_string().unwrap_or_default()),
        e => e.to_string(),
    }
}

/// Random-enough session id (ComfyUI only needs it unique per client).
/// Lora Manager autocomplete settings, read from the ComfyUI settings store the
/// web extension writes to. Defaults match `comfyui-lora-manager/web/comfyui/settings.js`.
#[derive(Debug, Clone)]
pub struct LmPrefs {
    pub tags: bool,
    pub append_comma: bool,
    pub space_underscore: bool,
    pub accept_tab: bool,
    pub accept_enter: bool,
}

impl Default for LmPrefs {
    fn default() -> Self {
        Self { tags: true, append_comma: true, space_underscore: false, accept_tab: true, accept_enter: true }
    }
}

/// Read the Lora Manager autocomplete settings. Any failure keeps the defaults.
pub fn lm_prefs(base: &str) -> LmPrefs {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(5))
        .build();
    let Ok(resp) = agent.get(&format!("{base}/api/settings")).call() else {
        return LmPrefs::default();
    };
    let Ok(v) = resp.into_json::<Value>() else {
        return LmPrefs::default();
    };
    let get = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    let bool_at = |k: &str, d: bool| get(k).as_bool().unwrap_or(d);
    let accept = get("loramanager.autocomplete_accept_key").as_str().unwrap_or("both").to_ascii_lowercase();
    LmPrefs {
        tags: bool_at("loramanager.prompt_tag_autocomplete", true),
        append_comma: bool_at("loramanager.autocomplete_append_comma", true),
        space_underscore: bool_at("loramanager.tag_space_replacement", false),
        accept_tab: accept.contains("tab") || accept == "both",
        accept_enter: accept.contains("enter") || accept == "both",
    }
}

/// `(text, detail)` suggestions for the prompt editor. Lora queries use Lora Manager's
/// relative model paths; tag queries use its custom-word FTS index.
pub fn lm_suggest(base: &str, query: &str, lora: bool) -> Vec<(String, String)> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(5))
        .build();
    let q = percent_encode(query);
    let path = if lora {
        format!("/api/lm/loras/relative-paths?search={q}&limit=15")
    } else {
        format!("/api/lm/custom-words/search?search={q}&enriched=true&limit=20")
    };
    let Ok(resp) = agent.get(&format!("{base}{path}")).call() else {
        return Vec::new();
    };
    let Ok(v) = resp.into_json::<Value>() else {
        return Vec::new();
    };
    let key = if lora { "relative_paths" } else { "words" };
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|it| match it {
                    Value::String(s) => Some((s.clone(), String::new())),
                    Value::Object(o) => {
                        let name = o.get("tag_name").or_else(|| o.get("name")).and_then(Value::as_str)?;
                        let cat = o.get("category").and_then(Value::as_str).unwrap_or("");
                        let count = o.get("post_count").and_then(Value::as_i64).unwrap_or(0);
                        let detail = match (cat.is_empty(), count) {
                            (false, 0) => cat.to_owned(),
                            (true, 0) => String::new(),
                            (false, n) => format!("{cat} · {n}"),
                            (true, n) => n.to_string(),
                        };
                        Some((name.to_owned(), detail))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Percent-encode everything but RFC 3986 unreserved characters (workflow files may
/// contain spaces, '#'? no, and non-ASCII).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn client_id() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("comfy-gnome-{:x}{:x}", t.as_nanos(), std::process::id())
}

fn ws_session(url: &str, tx: &async_channel::Sender<WsEvent>) -> Result<(), String> {
    let (mut ws, _) = tungstenite::connect(url).map_err(|e| e.to_string())?;
    // This ComfyUI forks preview frames behind a feature-flag handshake that must be the
    // first client message; without it `progress.py` sends no live previews at all.
    ws.send(tungstenite::Message::Text(
        r#"{"type":"feature_flags","data":{"supports_preview_metadata":true}}"#.to_owned().into(),
    ))
    .map_err(|e| e.to_string())?;
    tx.send_blocking(WsEvent::Connected).map_err(|e| e.to_string())?;
    loop {
        let ev = match ws.read().map_err(|e| e.to_string())? {
            tungstenite::Message::Text(t) => parse_text(&t),
            tungstenite::Message::Binary(b) => parse_binary(&b),
            tungstenite::Message::Close(_) => return Ok(()),
            _ => None,
        };
        if let Some(ev) = ev {
            tx.send_blocking(ev).map_err(|e| e.to_string())?;
        }
    }
}

/// `progress_state` carries every tracked node; the bar wants the busiest running one.
fn running_progress(d: &Value) -> Option<(i64, i64, String)> {
    d["nodes"]
        .as_object()?
        .iter()
        .filter(|(_, n)| n["state"].as_str() == Some("running"))
        .map(|(id, n)| (n["value"].as_i64().unwrap_or(0), n["max"].as_i64().unwrap_or(0), id.clone()))
        .filter(|(_, max, _)| *max > 0)
        .max_by(|a, b| (a.0 as f64 / a.1 as f64).total_cmp(&(b.0 as f64 / b.1 as f64)))
}

pub fn parse_text(t: &str) -> Option<WsEvent> {
    let v: Value = serde_json::from_str(t).ok()?;
    let kind = v["type"].as_str()?.to_owned();
    let d = &v["data"];
    let s = |k: &str| d[k].as_str().map(str::to_owned);
    Some(match kind.as_str() {
        "status" => WsEvent::Status {
            queue_remaining: d["status"]["exec_info"]["queue_remaining"].as_i64().unwrap_or(0),
        },
        "progress" => WsEvent::Progress {
            value: d["value"].as_i64().unwrap_or(0),
            max: d["max"].as_i64().unwrap_or(1),
            node: s("node"),
        },
        // Newer backends send one combined message instead of per-node `progress`.
        "progress_state" => {
            let (value, max, node) = running_progress(d)?;
            WsEvent::Progress { value, max, node: Some(node) }
        }
        "executing" => WsEvent::Executing { node: s("node"), prompt_id: s("prompt_id") },
        "executed" => WsEvent::Executed {
            node: s("node"),
            images: d["output"]["images"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| OutputImage {
                    filename: i["filename"].as_str().unwrap_or_default().to_owned(),
                    subfolder: i["subfolder"].as_str().unwrap_or_default().to_owned(),
                    kind: i["type"].as_str().unwrap_or("output").to_owned(),
                })
                .collect(),
        },
        "execution_start" => WsEvent::ExecutionStart { prompt_id: s("prompt_id").unwrap_or_default() },
        "execution_error" => WsEvent::ExecutionError {
            message: format!(
                "{} (node {}): {}",
                s("node_type").unwrap_or_default(),
                s("node_id").unwrap_or_default(),
                s("exception_message").unwrap_or_default().trim()
            ),
        },
        _ => WsEvent::Other { kind, data: d.clone() },
    })
}

/// Binary frames: big-endian u32 event type; 1 = PREVIEW_IMAGE (+ u32 image format),
/// 4 = PREVIEW_IMAGE_WITH_METADATA (+ u32 JSON length, JSON, image bytes).
pub fn parse_binary(b: &[u8]) -> Option<WsEvent> {
    let kind = u32::from_be_bytes(b.get(..4)?.try_into().ok()?);
    match kind {
        1 => Some(WsEvent::Preview { bytes: b.get(8..)?.to_vec(), display_node: None }),
        4 => {
            let len = u32::from_be_bytes(b.get(4..8)?.try_into().ok()?) as usize;
            let meta = serde_json::from_slice::<Value>(b.get(8..8 + len)?).ok();
            let display_node = meta
                .as_ref()
                .and_then(|m| m.get("display_node_id").or_else(|| m.get("node_id")))
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                });
            Some(WsEvent::Preview { bytes: b.get(8 + len..)?.to_vec(), display_node })
        }
        _ => None, // 2 = UNENCODED_PREVIEW_IMAGE (converted to 1 by the server), 3 = TEXT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ws_messages() {
        let ev = parse_text(r#"{"type":"progress","data":{"value":3,"max":20,"node":"5"}}"#);
        assert!(matches!(ev, Some(WsEvent::Progress { value: 3, max: 20, node: Some(ref n) }) if n == "5"));
        let ev = parse_text(r#"{"type":"status","data":{"status":{"exec_info":{"queue_remaining":2}}}}"#);
        assert!(matches!(ev, Some(WsEvent::Status { queue_remaining: 2 })));
        let ev = parse_text(r#"{"type":"executing","data":{"node":null,"prompt_id":"x"}}"#);
        assert!(matches!(ev, Some(WsEvent::Executing { node: None, .. })));
        let ev = parse_binary(&[0, 0, 0, 1, 0, 0, 0, 1, 0xff, 0xd8]);
        assert!(matches!(ev, Some(WsEvent::Preview { ref bytes, display_node: None }) if bytes == &[0xff, 0xd8]));
        // PREVIEW_IMAGE_WITH_METADATA in this fork: event, json length, json, image.
        let meta = br#"{"display_node_id":"75","node_id":"75:73"}"#;
        let mut frame = vec![0, 0, 0, 4];
        frame.extend((meta.len() as u32).to_be_bytes());
        frame.extend(meta);
        frame.extend([0xff, 0xd8]);
        let ev = parse_binary(&frame);
        assert!(matches!(ev, Some(WsEvent::Preview { ref bytes, display_node: Some(ref n) }) if bytes == &[0xff, 0xd8] && n == "75"));
        let ev = parse_text(
            r#"{"type":"progress_state","data":{"nodes":{"7":{"state":"running","value":5,"max":10},"8":{"state":"finished","value":9,"max":10}}}}"#,
        );
        assert!(matches!(ev, Some(WsEvent::Progress { value: 5, max: 10, node: Some(ref n) }) if n == "7"));
    }

    #[test]
    fn parses_history_entry() {
        // Trimmed from a live `GET /history?max_items=1` response.
        let v: Value = serde_json::from_str(
            r#"{"prompt":[0,"975a8158-d73b-4e02-8a2f-adbdfc12652c",{},{},"/tmp/out"],
                 "outputs":{"146":{"images":[]},"147":{"images":[{"filename":"ComfyUI_temp_ssrkp_00001_.png",
                   "subfolder":"","type":"temp","metadata":{"kind":"image","width":1104,"height":1424}}]}},
                 "status":{"status_str":"success","completed":true,"messages":[
                   ["execution_start",{"prompt_id":"975a8158","timestamp":1794090983888}],
                   ["execution_cached",{"nodes":[],"prompt_id":"975a8158","timestamp":1794090983906}],
                   ["execution_success",{"prompt_id":"975a8158","timestamp":1794091088573}]]}}"#,
        )
        .unwrap();
        let e = HistoryEntry::parse("975a8158", &v);
        assert_eq!(e.number, Some(0));
        assert_eq!(e.images.len(), 1);
        assert_eq!(e.images[0].filename, "ComfyUI_temp_ssrkp_00001_.png");
        assert_eq!(e.images[0].query(), "filename=ComfyUI_temp_ssrkp_00001_.png&subfolder=&type=temp");
        assert!((e.duration().unwrap() - 104.685).abs() < 1e-6);
    }

    #[test]
    fn encodes_workflow_paths() {
        assert_eq!(percent_encode("sub dir/a+b.json"), "sub%20dir%2Fa%2Bb.json");
    }
}

/// Live check against a running ComfyUI: `cargo test -- --ignored live`.
#[cfg(test)]
mod live {
    /// Posts a prompt whose only node type does not exist: the server must reject it,
    /// so nothing is queued (safe against a backend that is doing real work).
    #[test]
    #[ignore]
    fn live_rejects_unknown_node() {
        let c = super::Client::new("127.0.0.1", 8188);
        let before = c.get("/queue").unwrap();
        let err = c.queue_prompt(serde_json::json!({"1": {"class_type": "NoSuchNodeXYZ", "inputs": {}}}), None, false, None);
        assert!(err.unwrap_err().contains("400"), "expected 400");
        assert_eq!(c.get("/queue").unwrap(), before, "queue unchanged");
    }

    #[test]
    #[ignore]
    fn live_object_info_and_ws() {
        let c = super::Client::new("127.0.0.1", 8188);
        let reg = crate::registry::Registry::from_object_info(&c.get("/object_info").unwrap());
        assert!(reg.get("KSampler").is_some());
        eprintln!("{} node types", reg.nodes.len());
        let (tx, rx) = async_channel::unbounded();
        c.spawn_ws(tx);
        assert!(matches!(rx.recv_blocking().unwrap(), super::WsEvent::Connected));
        let ev = rx.recv_blocking().unwrap();
        eprintln!("first event: {ev:?}");
        assert!(matches!(ev, super::WsEvent::Status { .. }));
    }
}
