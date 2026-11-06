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

use crate::models::ModelConfig;

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
    /// Active model family + files (see models.rs).
    pub models: RwLock<ModelConfig>,
    /// `<app_config>/workflows/` for user template overrides.
    pub override_dir: Option<PathBuf>,
}

impl Comfy {
    pub fn new(models: ModelConfig, override_dir: Option<PathBuf>) -> Self {
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
            models: RwLock::new(models),
            override_dir,
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
    let mut warmed = false;
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
                if !warmed {
                    warmed = true;
                    let c = comfy.clone();
                    tokio::spawn(async move {
                        if let Err(e) = warmup(&c).await {
                            eprintln!("warmup skipped: {e}");
                        }
                    });
                }
                let _ = app.emit("comfy://status", StatusPayload { connected: true, url: http.clone(), queue_remaining: None });
                let (_, mut read) = stream.split();
                // Also wake every second so a URL change in settings takes
                // effect even when the old server is quiet.
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    let msg = tokio::select! {
                        m = read.next() => m,
                        _ = tick.tick() => {
                            if *comfy.base_url.read().await != http { break; }
                            continue;
                        }
                    };
                    let Some(Ok(msg)) = msg else { break };
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

/// Replace `{{key}}` placeholders with `vars[key]`. A string that is exactly
/// one placeholder takes the replacement's JSON type (numbers stay numbers);
/// placeholders embedded in longer strings are replaced as text.
fn substitute(v: &mut Value, vars: &HashMap<&str, Value>) {
    match v {
        Value::String(s) => {
            if let Some(key) = s.strip_prefix("{{").and_then(|r| r.strip_suffix("}}")) {
                if let Some(rep) = vars.get(key) {
                    *v = rep.clone();
                    return;
                }
            }
            if s.contains("{{") {
                let mut out = s.clone();
                for (k, rep) in vars {
                    let text = match rep { Value::String(t) => t.clone(), other => other.to_string() };
                    out = out.replace(&format!("{{{{{k}}}}}"), &text);
                }
                *s = out;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| substitute(x, vars)),
        Value::Object(o) => o.values_mut().for_each(|x| substitute(x, vars)),
        _ => {}
    }
}

/// Build a workflow for `kind` from the selected family's template, with the
/// model selection and family sampling defaults filled in plus `vars`.
fn build_workflow(cfg: &ModelConfig, kind: &str, override_dir: Option<&Path>, vars: &mut HashMap<&str, Value>) -> Result<Value, String> {
    let fam = cfg.family()?;
    let mut wf = fam.template(kind, override_dir)?;
    for (k, v) in cfg.vars()? {
        vars.insert(k, v);
    }
    substitute(&mut wf, vars);
    Ok(wf)
}

/// A tiny 256² text-to-image that only writes a temp preview (used for the
/// launch warm-up and the settings "Test" button).
fn preview_workflow(cfg: &ModelConfig, override_dir: Option<&Path>, steps: u32) -> Result<Value, String> {
    let fam = cfg.family()?;
    let mut vars: HashMap<&str, Value> = HashMap::new();
    for (k, v) in [("prompt", json!("a small red apple on a wooden table")), ("negative", json!("")), ("seed", json!(0)),
                   ("width", json!(256)), ("height", json!(256)), ("steps", json!(steps)), ("cfg", json!(fam.defaults.cfg)),
                   ("prefix", json!("board/_warmup"))] {
        vars.insert(k, v);
    }
    let mut wf = build_workflow(cfg, "t2i", override_dir, &mut vars)?;
    for node in wf.as_object_mut().into_iter().flatten().map(|(_, n)| n) {
        if node["class_type"] == "SaveImage" {
            node["class_type"] = json!("PreviewImage");
            if let Some(i) = node["inputs"].as_object_mut() {
                i.remove("filename_prefix");
            }
        }
    }
    Ok(wf)
}

// ---------------------------------------------------------------------------
// Generation jobs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GenRequest {
    pub card_id: String,
    /// "t2i", "edit" (reference-image edit) or "variation" (img2img)
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
    /// Re-attach to a prompt queued earlier (e.g. before an app restart)
    /// instead of queueing a new one.
    #[serde(default)]
    pub resume_prompt_id: Option<String>,
    /// Variation strength (img2img denoise); ignored by other kinds.
    #[serde(default)]
    pub denoise: Option<f64>,
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
    prompt_id: Option<String>,
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
pub async fn run_job(app: Sink, comfy: Arc<Comfy>, req: GenRequest) {
    let card_id = req.card_id.clone();
    let res = run_job_inner(&app, &comfy, &req).await;
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
    let _ = app.emit("gen://progress", ProgressPayload { card_id: card_id.to_string(), stage, value, max, node, prompt_id: None });
}

async fn run_job_inner(
    app: &Sink,
    comfy: &Arc<Comfy>,
    req: &GenRequest,
) -> Result<DonePayload, (String, bool)> {
    let e = |m: String| (m, false);
    let board_dir = PathBuf::from(&req.board_dir);

    // Subscribe before queueing so no event can slip past us.
    let mut rx = comfy.events.subscribe();
    let prompt_id = match &req.resume_prompt_id {
        Some(pid) => {
            if !prompt_known(comfy, pid).await {
                return Err(e("This job is no longer known to ComfyUI (server restarted?). Retry?".into()));
            }
            pid.clone()
        }
        None => queue_new(app, comfy, req, &board_dir).await.map_err(e)?,
    };
    comfy.jobs.write().await.insert(req.card_id.clone(), prompt_id.clone());
    let _ = app.emit("gen://progress", ProgressPayload {
        card_id: req.card_id.clone(), stage: "queued", value: None, max: None, node: None, prompt_id: Some(prompt_id.clone()),
    });

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

/// Is this prompt still pending/running, or already in history?
async fn prompt_known(comfy: &Comfy, pid: &str) -> bool {
    if let Ok(h) = comfy.get_json(&format!("/history/{pid}")).await {
        if !h[pid].is_null() {
            return true;
        }
    }
    if let Ok(q) = comfy.get_json("/queue").await {
        for key in ["queue_running", "queue_pending"] {
            if q[key].as_array().into_iter().flatten().any(|item| item[1] == pid) {
                return true;
            }
        }
    }
    false
}

/// Build the workflow from its template, upload any source image, queue it.
async fn queue_new(app: &Sink, comfy: &Comfy, req: &GenRequest, board_dir: &Path) -> Result<String, String> {
    let e = |m: String| m;
    progress(app, &req.card_id, "preparing", None, None, None);

    let mut vars: HashMap<&str, Value> = HashMap::new();
    vars.insert("prompt", json!(req.prompt));
    vars.insert("negative", json!(req.negative));
    vars.insert("seed", json!(req.seed));
    vars.insert("width", json!(req.width.clamp(64, 2048) / 16 * 16));
    vars.insert("height", json!(req.height.clamp(64, 2048) / 16 * 16));
    vars.insert("steps", json!(req.steps.clamp(1, 150)));
    vars.insert("cfg", json!(req.cfg));
    // Edit output size: the reference image is resized to this many megapixels.
    vars.insert("denoise", json!(req.denoise.unwrap_or(0.7).clamp(0.05, 1.0)));
    vars.insert("megapixels", json!(((req.width as f64 * req.height as f64) / 1_048_576.0).clamp(0.1, 2.0)));
    // Inputs/outputs are shared with the user's other ComfyUI work, so keep
    // ours under board/<board name>/.
    vars.insert("prefix", json!(format!("board/{}", board_slug(board_dir))));

    let cfg = comfy.models.read().await.clone();
    let fam = cfg.family()?;
    match req.kind.as_str() {
        "t2i" => {}
        kind @ ("edit" | "variation") => {
            if !fam.supports(kind) {
                return Err(format!("The {} model family doesn't support {kind}. Pick another family in Settings > Models.", fam.label));
            }
            let src = req.source.as_ref().ok_or_else(|| e("Edit needs a source image".into()))?;
            let src_path = crate::board::resolve_asset(board_dir, src).map_err(e)?;
            let name = upload_image(comfy, &src_path).await.map_err(e)?;
            vars.insert("image", json!(name));
        }
        other => return Err(e(format!("Unknown generation kind '{other}'"))),
    }
    let workflow = build_workflow(&cfg, &req.kind, comfy.override_dir.as_deref(), &mut vars)?;
    queue_prompt(comfy, &workflow).await
}

/// Load the selected models in the background so the user's first image is
/// fast: a 1-step 256² text-to-image whose result only goes to ComfyUI's temp
/// folder. Skipped when ComfyUI is busy with anything else.
pub async fn warmup(comfy: &Comfy) -> Result<(), String> {
    let q = comfy.get_json("/queue").await?;
    let busy = |k: &str| q[k].as_array().map(|a| !a.is_empty()).unwrap_or(true);
    if busy("queue_running") || busy("queue_pending") {
        return Ok(());
    }
    let cfg = comfy.models.read().await.clone();
    let wf = preview_workflow(&cfg, comfy.override_dir.as_deref(), 1)?;
    queue_prompt(comfy, &wf).await.map(|_| ())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTest {
    pub seconds: f64,
}

/// Settings "Test": render a few-step 256² image with `cfg` (not yet saved)
/// and wait for it, so bad combinations surface in settings.
pub async fn test_models(comfy: &Arc<Comfy>, cfg: &ModelConfig) -> Result<ModelTest, String> {
    let fam = cfg.family()?;
    let wf = preview_workflow(cfg, comfy.override_dir.as_deref(), fam.defaults.test_steps)?;
    let t = std::time::Instant::now();
    let mut rx = comfy.events.subscribe();
    let pid = queue_prompt(comfy, &wf).await?;
    let quiet = Sink(Arc::new(|_, _| {}));
    let outcome = tokio::time::timeout(Duration::from_secs(900), wait_for(&quiet, comfy, &mut rx, "model-test", &pid))
        .await
        .map_err(|_| "Test timed out after 15 minutes".to_string())?;
    match outcome {
        Outcome::Success => Ok(ModelTest { seconds: t.elapsed().as_secs_f64() }),
        Outcome::Interrupted => Err("Test was cancelled in ComfyUI".into()),
        Outcome::Error(m) if is_oom(&m) => Err(format!("GPU out of memory: {m}")),
        Outcome::Error(m) => Err(m),
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelLists {
    pub diffusion_models: Vec<String>,
    pub text_encoders: Vec<String>,
    pub vae: Vec<String>,
    pub checkpoints: Vec<String>,
    pub clip_types: Vec<String>,
}

/// Installed model files per folder, plus CLIPLoader's `type` options.
pub async fn list_models(comfy: &Comfy) -> Result<ModelLists, String> {
    let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>();
    let mut out = ModelLists::default();
    for (folder, slot) in [
        ("diffusion_models", &mut out.diffusion_models),
        ("text_encoders", &mut out.text_encoders),
        ("vae", &mut out.vae),
        ("checkpoints", &mut out.checkpoints),
    ] {
        *slot = strings(&comfy.get_json(&format!("/models/{folder}")).await?);
    }
    let clip = comfy.get_json("/object_info/CLIPLoader").await?;
    out.clip_types = strings(&clip["CLIPLoader"]["input"]["required"]["type"][0]);
    Ok(out)
}

/// Board folder name reduced to a safe ComfyUI subfolder name.
fn board_slug(board_dir: &Path) -> String {
    let name = board_dir.file_name().and_then(|n| n.to_str()).unwrap_or("board");
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if slug.trim_matches('_').is_empty() { "board".into() } else { slug }
}

/// Ask ComfyUI to unload models and release VRAM (best effort).
pub async fn free_memory(comfy: &Comfy, timeout: Duration) -> Result<(), String> {
    let resp = comfy
        .http
        .post(comfy.url("/free").await)
        .json(&json!({ "unload_models": true, "free_memory": true }))
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| unreachable_msg(&e))?;
    if resp.status().is_success() { Ok(()) } else { Err(format!("/free failed: {}", resp.status())) }
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
        .text("subfolder", "board")
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

pub async fn check(comfy: &Comfy) -> CheckReport {
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
    let cfg = comfy.models.read().await.clone();
    let fam = match cfg.family() {
        Ok(f) => f,
        Err(m) => return CheckReport { connected: true, problems: vec![m], device, vram_free_mb },
    };
    for kind in fam.kinds() {
        let label = match kind { "t2i" => "Text-to-image", "edit" => "Edit", _ => "Variation" };
        // Model names are filled in so they get validated against ComfyUI's file lists.
        let wf = match build_workflow(&cfg, kind, comfy.override_dir.as_deref(), &mut HashMap::new()) {
            Ok(w) => w,
            Err(m) => { problems.push(format!("{label}: {m}")); continue; }
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
                if s.contains("{{") { continue; }
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
        let fam = crate::models::family("flux_klein").unwrap();
        let mut v: Value = fam.template("t2i", None).unwrap();
        let mut vars = HashMap::new();
        vars.insert("prompt", json!("a fox"));
        vars.insert("seed", json!(42u64));
        vars.insert("width", json!(512));
        substitute(&mut v, &vars);
        assert_eq!(v["4"]["inputs"]["text"], "a fox");
        assert_eq!(v["7"]["inputs"]["seed"], 42);
        assert_eq!(v["6"]["inputs"]["width"], 512);

        let mut vars = HashMap::new();
        vars.insert("prefix", json!("board/my_board"));
        let v = build_workflow(&ModelConfig::default(), "edit", None, &mut vars).unwrap();
        assert_eq!(v["9"]["inputs"]["filename_prefix"], "board/my_board/edit");
        assert_eq!(v["1"]["inputs"]["unet_name"], "flux3-image-klein-base-int8-convrot.safetensors");
        assert_eq!(v["2"]["inputs"]["type"], "flux3");
        assert_eq!(v["7"]["inputs"]["sampler_name"], "euler");
    }

    #[test]
    fn preview_workflow_writes_no_files() {
        let cfg = ModelConfig { family: "checkpoint".into(), ckpt: "sd_xl_base_1.0.safetensors".into(), ..Default::default() };
        let wf = preview_workflow(&cfg, None, 4).unwrap();
        assert_eq!(wf["9"]["class_type"], "PreviewImage");
        assert_eq!(wf["1"]["inputs"]["ckpt_name"], "sd_xl_base_1.0.safetensors");
        assert_eq!(wf["7"]["inputs"]["scheduler"], "karras");
        assert!(!serde_json::to_string(&wf).unwrap().contains("{{"));
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(b64(b"hello"), "aGVsbG8=");
        assert_eq!(b64(b"hi!"), "aGkh");
    }

    #[test]
    fn board_slugs() {
        assert_eq!(board_slug(Path::new("E:/Boards/My Board!")), "My_Board_");
        assert_eq!(board_slug(Path::new("C:/x/untitled-17")), "untitled-17");
    }

    #[test]
    fn detects_oom() {
        assert!(is_oom("KSampler: Allocation on device \nThis error means you ran out of memory on your GPU."));
        assert!(!is_oom("Value not in list"));
    }
}
