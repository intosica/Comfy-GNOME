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
    /// Binary preview frame (JPEG/PNG bytes, header stripped).
    Preview(Vec<u8>),
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

#[derive(Debug, Clone, Default)]
pub struct Queue {
    pub running: Vec<QueueItem>,
    pub pending: Vec<QueueItem>,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    agent: ureq::Agent,
    pub client_id: String,
}

impl Client {
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

    /// `GET /view?filename=…&subfolder=…&type=…` — raw image bytes of an output/preview.
    pub fn view(&self, img: &OutputImage) -> Result<Vec<u8>, String> {
        let path = format!(
            "/view?filename={}&subfolder={}&type={}",
            percent_encode(&img.filename),
            percent_encode(&img.subfolder),
            percent_encode(&img.kind)
        );
        let r = self.agent.get(&format!("{}{path}", self.base)).call().map_err(err)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
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

    /// `POST /prompt` with an API-format prompt. Returns `prompt_id`.
    #[allow(dead_code)] // Phase 2.4
    pub fn queue_prompt(&self, prompt: Value, workflow: Option<Value>) -> Result<String, String> {
        let mut body = serde_json::json!({ "client_id": self.client_id, "prompt": prompt });
        if let Some(w) = workflow {
            body["extra_data"] = serde_json::json!({ "extra_pnginfo": { "workflow": w } });
        }
        let v = self.post("/prompt", body)?;
        v["prompt_id"].as_str().map(str::to_owned).ok_or_else(|| format!("unexpected /prompt reply: {v}"))
    }

    pub fn interrupt(&self) -> Result<(), String> {
        self.post("/interrupt", Value::Null).map(drop)
    }

    /// Delete pending queue entries by prompt id.
    pub fn delete_queued(&self, ids: &[String]) -> Result<(), String> {
        self.post("/queue", serde_json::json!({ "delete": ids })).map(drop)
    }

    /// Run the `/ws` reader on its own thread forever, reconnecting on failure.
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
/// Percent-encode everything but RFC 3986 unreserved characters (workflow files may
/// contain spaces, '#'? no, and non-ASCII).
fn percent_encode(s: &str) -> String {
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

/// Binary frames: big-endian u32 event type; 1 = PREVIEW_IMAGE (+ u32 image format), then bytes.
pub fn parse_binary(b: &[u8]) -> Option<WsEvent> {
    let kind = u32::from_be_bytes(b.get(..4)?.try_into().ok()?);
    match kind {
        1 => Some(WsEvent::Preview(b.get(8..)?.to_vec())),
        _ => None, // 2 = UNENCODED_PREVIEW_IMAGE, 3 = TEXT, 4 = PREVIEW_IMAGE_WITH_METADATA: later
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
        assert!(matches!(ev, Some(WsEvent::Preview(ref b)) if b == &[0xff, 0xd8]));
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
        let err = c.queue_prompt(serde_json::json!({"1": {"class_type": "NoSuchNodeXYZ", "inputs": {}}}), None);
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
