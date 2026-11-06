//! Board persistence. A board is a folder: `board.json` + `assets/*.png`.
//! Asset paths inside `board.json` are always board-relative (`assets/x.png`).

use std::path::{Component, Path, PathBuf};

pub const BOARD_FILE: &str = "board.json";
pub const ASSETS_DIR: &str = "assets";

/// Resolve a board-relative asset path, rejecting anything that escapes the board.
pub fn resolve_asset(board_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_))) {
        return Err(format!("Invalid asset path '{rel}'"));
    }
    Ok(board_dir.join(rel_path))
}

/// Write image bytes as a new asset; returns (relative path, width, height).
pub fn write_asset(board_dir: &Path, bytes: &[u8], ext: &str) -> Result<(String, u32, u32), String> {
    let size = imagesize::blob_size(bytes).map_err(|e| format!("Not a readable image: {e}"))?;
    let dir = board_dir.join(ASSETS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Create {}: {e}", dir.display()))?;
    let name = format!("{}.{}", uuid::Uuid::new_v4().simple(), ext);
    std::fs::write(dir.join(&name), bytes).map_err(|e| format!("Write asset: {e}"))?;
    Ok((format!("{ASSETS_DIR}/{name}"), size.width as u32, size.height as u32))
}

/// Copy an external image file (drag & drop) into the board's assets.
pub fn import_file(board_dir: &Path, src: &Path) -> Result<(String, u32, u32), String> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"))
        .ok_or_else(|| format!("Unsupported file type: {}", src.display()))?;
    let bytes = std::fs::read(src).map_err(|e| format!("Read {}: {e}", src.display()))?;
    write_asset(board_dir, &bytes, &ext)
}

/// Atomically write board.json (write temp then rename).
pub fn save(board_dir: &Path, json: &str) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(json).map_err(|e| format!("Board JSON invalid: {e}"))?;
    std::fs::create_dir_all(board_dir.join(ASSETS_DIR)).map_err(|e| e.to_string())?;
    let tmp = board_dir.join("board.json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Write board: {e}"))?;
    std::fs::rename(&tmp, board_dir.join(BOARD_FILE)).map_err(|e| format!("Save board: {e}"))?;
    Ok(())
}

pub fn load(board_dir: &Path) -> Result<String, String> {
    let p = board_dir.join(BOARD_FILE);
    std::fs::read_to_string(&p).map_err(|e| format!("No board at {}: {e}", p.display()))
}

/// Copy all assets from one board folder into another (Save As).
pub fn copy_assets(from: &Path, to: &Path) -> Result<(), String> {
    let src = from.join(ASSETS_DIR);
    let dst = to.join(ASSETS_DIR);
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    if from == to || !src.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let target = dst.join(entry.file_name());
            if !target.exists() {
                std::fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        let b = Path::new("C:/boards/x");
        assert!(resolve_asset(b, "../secret.png").is_err());
        assert!(resolve_asset(b, "C:/Windows/x.png").is_err());
        assert!(resolve_asset(b, "assets/a.png").is_ok());
    }
}
