pub mod board;
pub mod comfy;
pub mod models;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use comfy::{CheckReport, Comfy, GenRequest, ModelLists, ModelTest, Sink};
use models::{FAMILIES, Family, ModelConfig};

fn sink(app: &AppHandle) -> Sink {
    let app = app.clone();
    Sink(Arc::new(move |event, payload| {
        let _ = app.emit(event, payload);
    }))
}

struct AppState {
    comfy: Arc<Comfy>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedAsset {
    asset: String,
    width: u32,
    height: u32,
}

/// `<app_config>/workflows/<family>/{t2i,edit,variation}.json` override the
/// built-in templates without rebuilding.
fn override_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("workflows"))
}

/// Let the webview load images from this board folder via the asset protocol.
fn allow_board(app: &AppHandle, dir: &std::path::Path) -> Result<(), String> {
    app.asset_protocol_scope().allow_directory(dir, true).map_err(|e| e.to_string())
}

// --- ComfyUI -----------------------------------------------------------------

#[tauri::command]
async fn comfy_check(state: State<'_, AppState>) -> Result<CheckReport, String> {
    Ok(comfy::check(&state.comfy).await)
}

// --- Model setup ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSetup {
    config: ModelConfig,
    families: &'static [Family],
}

#[tauri::command]
async fn models_get(state: State<'_, AppState>) -> Result<ModelSetup, String> {
    Ok(ModelSetup { config: state.comfy.models.read().await.clone(), families: FAMILIES })
}

/// Installed files from ComfyUI (fails when ComfyUI is offline).
#[tauri::command]
async fn models_list(state: State<'_, AppState>) -> Result<ModelLists, String> {
    comfy::list_models(&state.comfy).await
}

/// Save and activate a selection. Nothing is freed or loaded here: the next
/// generation loads the new models.
#[tauri::command]
async fn models_set(app: AppHandle, config: ModelConfig, state: State<'_, AppState>) -> Result<(), String> {
    config.family()?;
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    models::save_config(&dir, &config)?;
    *state.comfy.models.write().await = config;
    Ok(())
}

/// Render a tiny test image with a (possibly unsaved) selection.
#[tauri::command]
async fn models_test(config: ModelConfig, state: State<'_, AppState>) -> Result<ModelTest, String> {
    comfy::test_models(&state.comfy, &config).await
}

#[tauri::command]
async fn comfy_status(state: State<'_, AppState>) -> Result<(bool, String), String> {
    Ok((*state.comfy.connected.read().await, state.comfy.base_url.read().await.clone()))
}

#[tauri::command]
async fn comfy_set_url(url: String, state: State<'_, AppState>) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL must start with http:// or https://".into());
    }
    *state.comfy.base_url.write().await = url;
    Ok(())
}

/// Queue a generation. Returns immediately; progress/result arrive as
/// `gen://progress`, `gen://preview`, `gen://done`, `gen://error` events.
#[tauri::command]
async fn generate(app: AppHandle, req: GenRequest, state: State<'_, AppState>) -> Result<(), String> {
    let comfy = state.comfy.clone();
    // A webview reload re-requests pending cards; if this process is still
    // tracking the job, the running task will deliver the result.
    if req.resume_prompt_id.is_some() && comfy.jobs.read().await.contains_key(&req.card_id) {
        return Ok(());
    }
    tauri::async_runtime::spawn(comfy::run_job(sink(&app), comfy, req));
    Ok(())
}

/// Unload ComfyUI's models so the GPU is free for Blender etc.
#[tauri::command]
async fn comfy_free(state: State<'_, AppState>) -> Result<(), String> {
    comfy::free_memory(&state.comfy, std::time::Duration::from_secs(5)).await
}

#[tauri::command]
async fn cancel_generation(card_id: String, state: State<'_, AppState>) -> Result<(), String> {
    comfy::cancel(&state.comfy, &card_id).await
}

// --- Boards ------------------------------------------------------------------

/// Create a fresh scratch board under app data and return its folder.
#[tauri::command]
fn board_new(app: AppHandle) -> Result<String, String> {
    let base = app.path().app_data_dir().map_err(|e| e.to_string())?.join("boards");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = base.join(format!("untitled-{stamp}"));
    std::fs::create_dir_all(dir.join(board::ASSETS_DIR)).map_err(|e| e.to_string())?;
    allow_board(&app, &dir)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Open a board folder; returns the board.json contents.
#[tauri::command]
fn board_open(app: AppHandle, dir: String) -> Result<String, String> {
    let dir = PathBuf::from(dir);
    let json = board::load(&dir)?;
    allow_board(&app, &dir)?;
    Ok(json)
}

#[tauri::command]
async fn board_save(dir: String, json: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || board::save(&PathBuf::from(dir), &json))
        .await
        .map_err(|e| e.to_string())?
}

/// Download an image from a web address (pasted or dropped) into the board.
/// Only http(s); at most 40 MB; the bytes must be a png/jpg/webp/gif/bmp.
#[tauri::command]
async fn import_url(board_dir: String, url: String) -> Result<ImportedAsset, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| format!("Not a web address: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https links can be imported.".into());
    }
    const MAX: usize = 40 * 1024 * 1024;
    let client = reqwest::Client::builder()
        .user_agent("MoodCraft/0.1 (image import)")
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(parsed.clone()).send().await.map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: {} from {}", resp.status(), parsed.host_str().unwrap_or("?")));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX) {
        return Err("That image is larger than 40 MB.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Download failed: {e}"))? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX {
            return Err("That image is larger than 40 MB.".into());
        }
    }
    let ext = board::sniff_image_ext(&bytes).ok_or("That link isn't an image (png, jpg, webp, gif or bmp).")?;
    let dir = PathBuf::from(board_dir);
    tauri::async_runtime::spawn_blocking(move || board::write_asset(&dir, &bytes, ext))
        .await
        .map_err(|e| e.to_string())?
        .map(|(asset, width, height)| ImportedAsset { asset, width, height })
}

/// Write a self-contained HTML deck next to the board folder and open it.
/// `html` refers to images as `__ASSET_<n>__`, replaced by `assets[n]`
/// (board-relative) embedded as data URIs. Returns the file path.
#[tauri::command]
async fn export_deck(app: AppHandle, board_dir: String, name: String, html: String, assets: Vec<String>) -> Result<String, String> {
    let dir = PathBuf::from(&board_dir);
    if !dir.is_absolute() {
        return Err("Board folder unknown".into());
    }
    let out_dir = dir.parent().map(PathBuf::from).unwrap_or_else(|| dir.clone());
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '_' })
        .collect::<String>()
        .trim()
        .chars()
        .take(80)
        .collect();
    let path = out_dir.join(format!("{} - deck.html", if safe.is_empty() { "Board" } else { &safe }));
    let path2 = path.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut html = html;
        // Replace from the highest index down so __ASSET_1__ never matches inside __ASSET_10__.
        for (i, rel) in assets.iter().enumerate().rev() {
            let p = board::resolve_asset(&dir, rel)?;
            let bytes = std::fs::read(&p).map_err(|e| format!("Read {}: {e}", p.display()))?;
            let mime = match board::sniff_image_ext(&bytes) {
                Some("jpg") => "image/jpeg",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                Some("bmp") => "image/bmp",
                _ => "image/png",
            };
            html = html.replace(&format!("__ASSET_{i}__"), &format!("data:{mime};base64,{}", comfy::b64(&bytes)));
        }
        std::fs::write(&path2, html).map_err(|e| format!("Write {}: {e}", path2.display()))
    })
    .await
    .map_err(|e| e.to_string())??;
    let shown = path.to_string_lossy().into_owned();
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_path(&shown, None::<&str>).map_err(|e| format!("Saved {shown}, but couldn't open it: {e}"))?;
    }
    Ok(shown)
}

/// Create an empty subboard inside `parent`; returns its board-relative folder.
#[tauri::command]
fn subboard_create(parent: String, title: String) -> Result<String, String> {
    board::create_subboard(&PathBuf::from(parent), &title)
}

/// Save the board into a new folder, copying its assets along.
#[tauri::command]
async fn board_save_as(app: AppHandle, from: String, to: String, json: String) -> Result<(), String> {
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));
    if !to.is_absolute() || !from.is_absolute() {
        return Err("Pick a full folder path to save the board into.".into());
    }
    let to2 = to.clone();
    tauri::async_runtime::spawn_blocking(move || {
        board::copy_assets(&from, &to)?;
        board::save(&to, &json)
    })
    .await
    .map_err(|e| e.to_string())??;
    allow_board(&app, &to2)
}

#[tauri::command]
async fn import_image(board_dir: String, path: String) -> Result<ImportedAsset, String> {
    tauri::async_runtime::spawn_blocking(move || {
        board::import_file(&PathBuf::from(board_dir), &PathBuf::from(path))
            .map(|(asset, width, height)| ImportedAsset { asset, width, height })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Raw-body import for clipboard pastes. Headers: `x-board-dir` (URI-encoded), `x-ext`.
#[tauri::command]
fn import_image_bytes(request: tauri::ipc::Request) -> Result<ImportedAsset, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("Expected raw image bytes".into());
    };
    let h = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let dir = percent_decode(&h("x-board-dir").ok_or("missing x-board-dir")?);
    let ext = h("x-ext").unwrap_or_else(|| "png".into());
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
        return Err(format!("Unsupported pasted type '{ext}'"));
    }
    board::write_asset(&PathBuf::from(dir), bytes, &ext).map(|(asset, width, height)| ImportedAsset { asset, width, height })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(windows)]
const OLD_IDENTIFIER: &str = "com.sunsh.boardcreate";
#[cfg(windows)]
const IDENTIFIER: &str = "com.sunsh.moodcraft";

/// The app was called BoardCreate. On first launch as MoodCraft, copy (never
/// move) the old app data across: models.json, workflow overrides, scratch
/// boards, plus the WebView's localStorage (settings, last-opened board).
/// Runs before the webview starts so localStorage is in place when it opens.
fn migrate_from_boardcreate() {
    #[cfg(windows)]
    for (var, sub) in [("APPDATA", None), ("LOCALAPPDATA", Some(["EBWebView", "Default", "Local Storage"]))] {
        let Some(base) = std::env::var_os(var).map(PathBuf::from) else { continue };
        let (old, new) = (base.join(OLD_IDENTIFIER), base.join(IDENTIFIER));
        if new.exists() || !old.is_dir() {
            continue;
        }
        let (from, to) = match sub {
            Some(parts) => (parts.iter().fold(old, |p, s| p.join(s)), parts.iter().fold(new.clone(), |p, s| p.join(s))),
            None => (old, new.clone()),
        };
        if !from.is_dir() {
            continue;
        }
        if let Err(e) = board::copy_tree(&from, &to) {
            eprintln!("MoodCraft: couldn't copy {} to {}: {e}", from.display(), to.display());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    migrate_from_boardcreate();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let config_dir = app.path().app_config_dir().ok();
            let models = models::load_config(config_dir.as_deref());
            let comfy = Arc::new(Comfy::new(models, override_dir(app.handle())));
            app.manage(AppState { comfy: comfy.clone() });
            tauri::async_runtime::spawn(comfy::ws_loop(sink(app.handle()), comfy));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            comfy_check,
            comfy_status,
            comfy_set_url,
            generate,
            cancel_generation,
            models_get,
            models_list,
            models_set,
            models_test,
            comfy_free,
            board_new,
            board_open,
            board_save,
            board_save_as,
            subboard_create,
            import_image,
            import_image_bytes,
            import_url,
            export_deck,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| {
            // Release ComfyUI's VRAM when the board closes (only then: models
            // stay loaded between generations so they remain fast).
            if let tauri::RunEvent::Exit = event {
                let Some(state) = app.try_state::<AppState>() else { return };
                let c = state.comfy.clone();
                let _ = tauri::async_runtime::block_on(comfy::free_memory(&c, std::time::Duration::from_secs(2)));
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn percent_decoding() {
        assert_eq!(super::percent_decode("E%3A%5CBoards%5Cmy%20board"), "E:\\Boards\\my board");
        assert_eq!(super::percent_decode("%E2%9C%93"), "✓");
    }
}
