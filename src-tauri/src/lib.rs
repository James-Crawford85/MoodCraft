pub mod board;
pub mod comfy;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use comfy::{CheckReport, Comfy, GenRequest, Sink};

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

/// `<app_config>/workflows/` — drop `t2i.json` / `edit.json` here to override
/// the built-in templates without rebuilding.
fn override_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("workflows"))
}

/// Let the webview load images from this board folder via the asset protocol.
fn allow_board(app: &AppHandle, dir: &std::path::Path) -> Result<(), String> {
    app.asset_protocol_scope().allow_directory(dir, true).map_err(|e| e.to_string())
}

// --- ComfyUI -----------------------------------------------------------------

#[tauri::command]
async fn comfy_check(app: AppHandle, state: State<'_, AppState>) -> Result<CheckReport, String> {
    Ok(comfy::check(&state.comfy, override_dir(&app).as_deref()).await)
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
    let ov = override_dir(&app);
    tauri::async_runtime::spawn(comfy::run_job(sink(&app), comfy, req, ov));
    Ok(())
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

/// Save the board into a new folder, copying its assets along.
#[tauri::command]
async fn board_save_as(app: AppHandle, from: String, to: String, json: String) -> Result<(), String> {
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let comfy = Arc::new(Comfy::new());
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { comfy: comfy.clone() })
        .setup(move |app| {
            tauri::async_runtime::spawn(comfy::ws_loop(sink(app.handle()), comfy.clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            comfy_check,
            comfy_status,
            comfy_set_url,
            generate,
            cancel_generation,
            board_new,
            board_open,
            board_save,
            board_save_as,
            import_image,
            import_image_bytes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn percent_decoding() {
        assert_eq!(super::percent_decode("E%3A%5CBoards%5Cmy%20board"), "E:\\Boards\\my board");
        assert_eq!(super::percent_decode("%E2%9C%93"), "✓");
    }
}
