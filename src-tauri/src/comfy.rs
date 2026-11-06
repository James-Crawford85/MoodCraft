//! ComfyUI client: HTTP queueing, a long-lived WebSocket listener, and the
//! per-card generation job runner. All network I/O lives here so the webview
//! never talks to ComfyUI directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast};

/// Workflow templates (ComfyUI API format) with `{{placeholder}}` values.
const T2I_TEMPLATE: &str = include_str!("../workflows/t2i_flux3_klein.json");
const EDIT_TEMPLATE: &str = include_str!("../workflows/edit_qwen_2509.json");

/// Event sink. In the app it forwards to Tauri's `emit`; the smoke test prints instead.
#[derive(Clone)]
pub struct Sink(pub Arc<dyn Fn(&str, Value) + Send + Sync>);

impl Sink {
    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> Result<(), ()> {
        (self.0)(event, serde_json::to_value(payload).unwrap_or(Value::Null));
        Ok(())
    }
}

pub const DEFAULT_URL: &str = "http://127.0.0.1:8188";

/// A decoded message from ComfyUI's `/ws` stream.
#[derive(Clone, Debug)]
pub enum WsEvent {
    Json { kind: String, data: Value },
    /// Live latent preview (JPEG/PNG bytes). `prompt_id` is only known when the
    /// server sends the metadata variant.
    Preview { prompt_id: Option<String>, mime: &'static str, bytes: Arc<Vec<u8>> },
}

pub struct Comfy {
    pub base_url: RwLock<String>,
    pub http: reqwest::Client,
    pub client_id: String,
    pub events: broadcast::Sender<WsEvent>,
    pub connected: RwLock<bool>,
    /// card_id -> prompt_id for cancellation.
    pub jobs: RwLock<HashMap<String, String>>,
    /// prompt_id currently executing on the server (from `executing` events).
    pub running_prompt: RwLock<Option<String>>,
}

impl Comfy {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Comfy {
            base_url: RwLock::new(DEFAULT_URL.to_string()),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .build()
                .expect("http client"),
            client_id: uuid::Uuid::new_v4().to_string(),
            events: tx,
            connected: RwLock::new(false),
            jobs: RwLock::new(HashMap::new()),
            running_prompt: RwLock::new(None),
        }
    }

    pub async fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.read().await.trim_end_matches('/'), path)
    }

    async fn get_json(&self, path: &str) -> Result<Value, String> {
        let resp = self
            .http
            .get(self.url(path).await)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| unreachable_msg(&e))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| format!("Bad JSON from ComfyUI: {e}"))?;
        if !status.is_success() {
            return Err(format!("ComfyUI {status}: {v}"));
        }
        Ok(v)
    }
}

fn unreachable_msg(e: &reqwest::Error) -> String {
    if e.is_connect() || e.is_timeout() {
        "Cannot reach ComfyUI. Is the Comfy Desktop app running?".to_string()
    } else {
        format!("ComfyUI request failed: {e}")
    }
}

// ---------------------------------------------------------------------------
// WebSocket listener
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusPayload {
    connected: bool,
    url: String,
    queue_remaining: Option<i64>,
}

/// Runs forever: connects to `/ws`, decodes messages, fans them out on the
/// broadcast channel, and reconnects with a short backoff.
pub async fn ws_loop(app: Sink, comfy: Arc<Comfy>) {
    loop {
        let http = comfy.base_url.read().await.clone();
        let ws_url = format!(
            "{}/ws?clientId={}",
            http.trim_end_matches('/').replacen("http", "ws", 1),
            comfy.client_id
        );
        match tokio_tungstenite::connect_async(ws_url.as_str()).await {
            Ok((stream, _)) => {
                *comfy.connected.write().await = true;
                let _ = app.emit("comfy://status", StatusPayload { connected: true, url: http.clone(), queue_remaining: None });
                let (_, mut read) = stream.split();
                while let Some(msg) = read.next().await {
                    let Ok(msg) = msg else { break };
                    match msg {
                        tokio_tungstenite::tungstenite::Message::Text(t) => {
                            let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                            let kind = v["type"].as_str().unwrap_or("").to_string();
                            let data = v["data"].clone();
                            if kind == "executing" {
                                let pid = data["prompt_id"].as_str().map(str::to_string);
                                *comfy.running_prompt.write().await =
                                    if data["node"].is_null() { None } else { pid };
                            }
                            if kind == "status" {
                                let q = data["status"]["exec_info"]["queue_remaining"].as_i64();
                                let _ = app.emit("comfy://status", StatusPayload { connected: true, url: http.clone(), queue_remaining: q });
                            }
                            let _ = comfy.events.send(WsEvent::Json { kind, data });
                        }
                        tokio_tungstenite::tungstenite::Message::Binary(b) => {
                            if let Some(ev) = decode_preview(&b) {
                                let _ = comfy.events.send(ev);
                            }
                        }
                        tokio_tungstenite::tungstenite::Message::Close(_) => break,
                        _ => {}
                    }
                    // URL changed from settings: reconnect to the new server.
                    if *comfy.base_url.read().await != http {
                        break;
                    }
                }
                *comfy.connected.write().await = false;
                *comfy.running_prompt.write().await = None;
                let _ = app.emit("comfy://status", StatusPayload { connected: false, url: http, queue_remaining: None });
            }
            Err(_) => {
                if *comfy.connected.read().await {
                    *comfy.connected.write().await = false;
                }
                let _ = app.emit("comfy://status", StatusPayload { connected: false, url: http, queue_remaining: None });
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Binary frames: `[u32 event][payload]`. Event 1 = `[u32 fmt][image]`,
/// event 4 = `[u32 meta_len][json meta][image]`.
fn decode_preview(b: &[u8]) -> Option<WsEvent> {
    match be_u32(b, 0)? {
        1 => {
            let mime = if be_u32(b, 4)? == 2 { "image/png" } else { "image/jpeg" };
            Some(WsEvent::Preview { prompt_id: None, mime, bytes: Arc::new(b.get(8..)?.to_vec()) })
        }
        4 => {
            let len = be_u32(b, 4)? as usize;
            let meta: Value = serde_json::from_slice(b.get(8..8 + len)?).ok()?;
            let mime = if meta["image_type"] == "image/png" { "image/png" } else { "image/jpeg" };
            Some(WsEvent::Preview {
                prompt_id: meta["prompt_id"].as_str().map(str::to_string),
                mime,
                bytes: Arc::new(b.get(8 + len..)?.to_vec()),
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Workflow templating
// ---------------------------------------------------------------------------

/// Replace every string value that is exactly `{{key}}` with `vars[key]`
/// (keeping the JSON type of the replacement, so numbers stay numbers).
fn substitute(v: &mut Value, vars: &HashMap<&str, Value>) {
    match v {
        Value::String(s) => {
            if let Some(key) = s.strip_prefix("{{").and_then(|r| r.strip_suffix("}}")) {
                if let Some(rep) = vars.get(key) {
                    *v = rep.clone();
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| substitute(x, vars)),
        Value::Object(o) => o.values_mut().for_each(|x| substitute(x, vars)),
        _ => {}
    }
}

/// Load a template, preferring a user override in `<app_config>/workflows/`.
fn load_template(override_dir: Option<&Path>, name: &str, builtin: &str) -> Result<Value, String> {
    if let Some(dir) = override_dir {
        let p = dir.join(name);
        if let Ok(s) = std::fs::read_to_string(&p) {
            return serde_json::from_str(&s).map_err(|e| format!("Bad workflow override {}: {e}", p.display()));
        }
    }
    serde_json::from_str(builtin).map_err(|e| format!("Bad builtin workflow {name}: {e}"))
}

// ---------------------------------------------------------------------------
// Generation jobs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GenRequest {
    pub card_id: String,
    /// "t2i" or "edit"
    pub kind: String,
    pub prompt: String,
    #[serde(default)]
    pub negative: String,
    pub seed: u64,
    #[serde(default = "d_size")]
    pub width: u32,
    #[serde(default = "d_size")]
    pub height: u32,
    pub steps: u32,
    pub cfg: f64,
    /// Board-relative asset path of the source image (edit only).
    pub source: Option<String>,
    pub board_dir: String,
}
fn d_size() -> u32 {
    768
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    card_id: String,
    stage: &'static str,
    value: Option<u64>,
    max: Option<u64>,
    node: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PreviewPayload {
    card_id: String,
    data_url: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DonePayload {
    card_id: String,
    asset: String,
    width: u32,
    height: u32,
    seed: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload {
    card_id: String,
    message: String,
    oom: bool,
    cancelled: bool,
}

enum Outcome {
    Success,
    Error(String),
    Interrupted,
}

/// Entry point used by the `generate` command. Never panics; every failure is
/// reported to the card via a `gen://error` event.
pub async fn run_job(app: Sink, comfy: Arc<Comfy>, req: GenRequest, override_dir: Option<PathBuf>) {
    let card_id = req.card_id.clone();
    let res = run_job_inner(&app, &comfy, &req, override_dir.as_deref()).await;
    comfy.jobs.write().await.remove(&card_id);
    match res {
        Ok(done) => {
            let _ = app.emit("gen://done", done);
        }
        Err((message, cancelled)) => {
            let oom = is_oom(&message);
            let message = if oom {
                "GPU out of memory. ComfyUI unloaded its models; close other GPU apps (Blender viewport, games) or lower the size, then retry.".to_string()
            } else {
                message
            };
            let _ = app.emit("gen://error", ErrorPayload { card_id, message, oom, cancelled });
        }
    }
}

fn is_oom(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("out of memory") || m.contains("allocation on device") || m.contains("outofmemory")
}

fn progress(app: &Sink, card_id: &str, stage: &'static str, value: Option<u64>, max: Option<u64>, node: Option<String>) {
    let _ = app.emit("gen://progress", ProgressPayload { card_id: card_id.to_string(), stage, value, max, node });
}

async fn run_job_inner(
    app: &Sink,
    comfy: &Arc<Comfy>,
    req: &GenRequest,
    override_dir: Option<&Path>,
) -> Result<DonePayload, (String, bool)> {
    let e = |m: String| (m, false);
    let board_dir = PathBuf::from(&req.board_dir);

    progress(app, &req.card_id, "preparing", None, None, None);

    let mut vars: HashMap<&str, Value> = HashMap::new();
    vars.insert("prompt", json!(req.prompt));
    vars.insert("negative", json!(req.negative));
    vars.insert("seed", json!(req.seed));
    vars.insert("width", json!(req.width.clamp(64, 2048) / 16 * 16));
    vars.insert("height", json!(req.height.clamp(64, 2048) / 16 * 16));
    vars.insert("steps", json!(req.steps.clamp(1, 150)));
    vars.insert("cfg", json!(req.cfg));

    let mut workflow = match req.kind.as_str() {
        "t2i" => load_template(override_dir, "t2i.json", T2I_TEMPLATE).map_err(e)?,
        "edit" => {
            let src = req.source.as_ref().ok_or_else(|| e("Edit needs a source image".into()))?;
            let src_path = crate::board::resolve_asset(&board_dir, src).map_err(e)?;
            let name = upload_image(comfy, &src_path).await.map_err(e)?;
            vars.insert("image", json!(name));
            load_template(override_dir, "edit.json", EDIT_TEMPLATE).map_err(e)?
        }
        other => return Err(e(format!("Unknown generation kind '{other}'"))),
    };
    substitute(&mut workflow, &vars);

    // Subscribe before queueing so no event can slip past us.
    let mut rx = comfy.events.subscribe();
    let prompt_id = queue_prompt(comfy, &workflow).await.map_err(e)?;
    comfy.jobs.write().await.insert(req.card_id.clone(), prompt_id.clone());
    progress(app, &req.card_id, "queued", None, None, None);

    let outcome = wait_for(app, comfy, &mut rx, &req.card_id, &prompt_id).await;
    match outcome {
        Outcome::Success => {}
        Outcome::Interrupted => return Err(("Cancelled".into(), true)),
        Outcome::Error(m) => return Err(e(m)),
    }

    progress(app, &req.card_id, "downloading", None, None, None);
    let hist = comfy.get_json(&format!("/history/{prompt_id}")).await.map_err(e)?;
    let img = first_output_image(&hist[&prompt_id])
        .ok_or_else(|| e("ComfyUI finished but returned no image".into()))?;
    let bytes = fetch_view(comfy, &img).await.map_err(e)?;
    let (asset, w, h) = crate::board::write_asset(&board_dir, &bytes, "png").map_err(e)?;
    Ok(DonePayload { card_id: req.card_id.clone(), asset, width: w, height: h, seed: req.seed })
}

async fn upload_image(comfy: &Comfy, path: &Path) -> Result<String, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("Read {}: {e}", path.display()))?;
    let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("source.png").to_string();
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(fname)
        .mime_str("application/octet-stream")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .part("image", part)
        .text("type", "input")
        .text("subfolder", "boardcreate")
        .text("overwrite", "true");
    let resp = comfy
        .http
        .post(comfy.url("/upload/image").await)
        .multipart(form)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| unreachable_msg(&e))?;
    if !resp.status().is_success() {
        return Err(format!("Upload failed: {} {}", resp.status(), resp.text().await.unwrap_or_default()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let name = v["name"].as_str().ok_or("Upload response missing name")?;
    let sub = v["subfolder"].as_str().unwrap_or("");
    Ok(if sub.is_empty() { name.to_string() } else { format!("{sub}/{name}") })
}

async fn queue_prompt(comfy: &Comfy, workflow: &Value) -> Result<String, String> {
    let body = json!({ "prompt": workflow, "client_id": comfy.client_id });
    let resp = comfy
        .http
        .post(comfy.url("/prompt").await)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| unreachable_msg(&e))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() || v.get("error").is_some() {
        return Err(format_validation_error(&v));
    }
    v["prompt_id"].as_str().map(str::to_string).ok_or_else(|| format!("Unexpected /prompt response: {v}"))
}

/// Turn a `/prompt` 400 body into a short readable message.
fn format_validation_error(v: &Value) -> String {
    let mut parts = vec![];
    if let Some(m) = v["error"]["message"].as_str() {
        parts.push(m.to_string());
    }
    if let Some(nodes) = v["node_errors"].as_object() {
        for (id, ne) in nodes {
            let class = ne["class_type"].as_str().unwrap_or("?");
            for err in ne["errors"].as_array().into_iter().flatten() {
                let msg = err["message"].as_str().unwrap_or("");
                let det = err["details"].as_str().unwrap_or("");
                parts.push(format!("[{class} #{id}] {msg}: {det}"));
            }
        }
    }
    if parts.is_empty() { format!("ComfyUI rejected the workflow: {v}") } else { parts.join("\n") }
}

/// Wait for our prompt to finish, driven by WebSocket events with a
/// `/history` poll as a fallback (covers WS drops and missed events).
async fn wait_for(
    app: &Sink,
    comfy: &Arc<Comfy>,
    rx: &mut broadcast::Receiver<WsEvent>,
    card_id: &str,
    prompt_id: &str,
) -> Outcome {
    let mut poll = tokio::time::interval(Duration::from_secs(2));
    poll.tick().await;
    let mut started = false;
    let mut last_preview = std::time::Instant::now() - Duration::from_secs(1);
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Outcome::Error("Event channel closed".into()),
                };
                match ev {
                    WsEvent::Json { kind, data } => {
                        if data["prompt_id"].as_str() != Some(prompt_id) { continue; }
                        match kind.as_str() {
                            "execution_start" => { started = true; progress(app, card_id, "running", None, None, None); }
                            "executing" => {
                                if data["node"].is_null() { return Outcome::Success; }
                                started = true;
                                progress(app, card_id, "running", None, None, data["node"].as_str().map(str::to_string));
                            }
                            "progress" => {
                                progress(app, card_id, "sampling", data["value"].as_u64(), data["max"].as_u64(), data["node"].as_str().map(str::to_string));
                            }
                            "execution_success" => return Outcome::Success,
                            "execution_error" => return Outcome::Error(format_exec_error(&data)),
                            "execution_interrupted" => return Outcome::Interrupted,
                            _ => {}
                        }
                    }
                    WsEvent::Preview { prompt_id: pid, mime, bytes } => {
                        let ours = match pid {
                            Some(p) => p == prompt_id,
                            None => started && comfy.running_prompt.read().await.as_deref() == Some(prompt_id),
                        };
                        // Throttle previews to ~4/s to keep the IPC channel light.
                        if ours && last_preview.elapsed() > Duration::from_millis(250) {
                            last_preview = std::time::Instant::now();
                            let data_url = format!("data:{mime};base64,{}", b64(&bytes));
                            let _ = app.emit("gen://preview", PreviewPayload { card_id: card_id.to_string(), data_url });
                        }
                    }
                }
            }
            _ = poll.tick() => {
                if let Ok(h) = comfy.get_json(&format!("/history/{prompt_id}")).await {
                    let entry = &h[prompt_id];
                    if !entry.is_null() {
                        let st = &entry["status"];
                        if st["status_str"] == "error" {
                            let msg = st["messages"].as_array().into_iter().flatten()
                                .find(|m| m[0] == "execution_error")
                                .map(|m| format_exec_error(&m[1]))
                                .unwrap_or_else(|| "ComfyUI reported an error".into());
                            return Outcome::Error(msg);
                        }
                        let interrupted = st["messages"].as_array().into_iter().flatten().any(|m| m[0] == "execution_interrupted");
                        if interrupted { return Outcome::Interrupted; }
                        if st["completed"] == true || first_output_image(entry).is_some() {
                            return Outcome::Success;
                        }
                    }
                }
            }
        }
    }
}

fn format_exec_error(d: &Value) -> String {
    let class = d["node_type"].as_str().unwrap_or("node");
    let msg = d["exception_message"].as_str().unwrap_or("unknown error").trim();
    format!("{class}: {msg}")
}

#[derive(Debug)]
struct OutImage {
    filename: String,
    subfolder: String,
    kind: String,
}

fn first_output_image(entry: &Value) -> Option<OutImage> {
    for out in entry["outputs"].as_object()?.values() {
        if let Some(img) = out["images"].as_array().and_then(|a| a.first()) {
            return Some(OutImage {
                filename: img["filename"].as_str()?.to_string(),
                subfolder: img["subfolder"].as_str().unwrap_or("").to_string(),
                kind: img["type"].as_str().unwrap_or("output").to_string(),
            });
        }
    }
    None
}

async fn fetch_view(comfy: &Comfy, img: &OutImage) -> Result<Vec<u8>, String> {
    let resp = comfy
        .http
        .get(comfy.url("/view").await)
        .query(&[("filename", &img.filename), ("subfolder", &img.subfolder), ("type", &img.kind)])
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| unreachable_msg(&e))?;
    if !resp.status().is_success() {
        return Err(format!("/view failed: {}", resp.status()));
    }
    Ok(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

pub async fn cancel(comfy: &Comfy, card_id: &str) -> Result<(), String> {
    let Some(pid) = comfy.jobs.read().await.get(card_id).cloned() else { return Ok(()) };
    // Dequeue if pending, interrupt only if it is *our* prompt that is running.
    let _ = comfy.http.post(comfy.url("/queue").await).json(&json!({ "delete": [pid] })).send().await;
    let _ = comfy.http.post(comfy.url("/interrupt").await).json(&json!({ "prompt_id": pid })).send().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Health check: verify the nodes and models our templates need are present.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub connected: bool,
    pub problems: Vec<String>,
    pub device: Option<String>,
    pub vram_free_mb: Option<u64>,
}

pub async fn check(comfy: &Comfy, override_dir: Option<&Path>) -> CheckReport {
    let mut problems = vec![];
    let stats = match comfy.get_json("/system_stats").await {
        Ok(s) => s,
        Err(m) => return CheckReport { connected: false, problems: vec![m], device: None, vram_free_mb: None },
    };
    let dev = &stats["devices"][0];
    let device = dev["name"].as_str().map(str::to_string);
    let vram_free_mb = dev["vram_free"].as_u64().map(|b| b / 1024 / 1024);

    let info = match comfy.get_json("/object_info").await {
        Ok(i) => i,
        Err(m) => return CheckReport { connected: true, problems: vec![m], device, vram_free_mb },
    };
    for (label, name, builtin) in [("Text-to-image", "t2i.json", T2I_TEMPLATE), ("Edit", "edit.json", EDIT_TEMPLATE)] {
        let wf = match load_template(override_dir, name, builtin) {
            Ok(w) => w,
            Err(m) => { problems.push(m); continue; }
        };
        for (id, node) in wf.as_object().into_iter().flatten() {
            let class = node["class_type"].as_str().unwrap_or("");
            let Some(def) = info.get(class) else {
                problems.push(format!("{label}: node '{class}' (#{id}) is not installed"));
                continue;
            };
            // Check combo (file list) inputs such as unet_name / clip_name / vae_name.
            for (key, val) in node["inputs"].as_object().into_iter().flatten() {
                let Some(s) = val.as_str() else { continue };
                if s.starts_with("{{") { continue; }
                let spec = def["input"]["required"].get(key).or_else(|| def["input"]["optional"].get(key));
                let Some(spec) = spec else { continue };
                if let Some(opts) = spec[0].as_array() {
                    if !opts.iter().any(|o| o == s) {
                        problems.push(format!("{label}: '{s}' not available for {class}.{key}"));
                    }
                }
            }
        }
    }
    CheckReport { connected: true, problems, device, vram_free_mb }
}

// Minimal base64 encoder (avoids an extra dependency for previews).
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_typed_values() {
        let mut v: Value = serde_json::from_str(T2I_TEMPLATE).unwrap();
        let mut vars = HashMap::new();
        vars.insert("prompt", json!("a fox"));
        vars.insert("seed", json!(42u64));
        vars.insert("width", json!(512));
        substitute(&mut v, &vars);
        assert_eq!(v["4"]["inputs"]["text"], "a fox");
        assert_eq!(v["7"]["inputs"]["seed"], 42);
        assert_eq!(v["6"]["inputs"]["width"], 512);
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(b64(b"hello"), "aGVsbG8=");
        assert_eq!(b64(b"hi!"), "aGkh");
    }

    #[test]
    fn detects_oom() {
        assert!(is_oom("KSampler: Allocation on device \nThis error means you ran out of memory on your GPU."));
        assert!(!is_oom("Value not in list"));
    }
}
