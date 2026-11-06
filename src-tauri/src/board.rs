//! Board persistence. A board is a folder: `board.json` + `assets/*.png`,
//! plus `boards/<name>/` for subboards (each a board folder of its own).
//! Paths inside `board.json` are always board-relative (`assets/x.png`, `boards/x`).

use std::path::{Component, Path, PathBuf};

pub const BOARD_FILE: &str = "board.json";
pub const ASSETS_DIR: &str = "assets";
pub const SUBBOARDS_DIR: &str = "boards";

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

/// File extension for image bytes we can show and send to ComfyUI, from the
/// bytes themselves (not the URL or the server's content type).
pub fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    use imagesize::ImageType;
    match imagesize::image_type(bytes).ok()? {
        ImageType::Png => Some("png"),
        ImageType::Jpeg => Some("jpg"),
        ImageType::Webp => Some("webp"),
        ImageType::Gif => Some("gif"),
        ImageType::Bmp => Some("bmp"),
        _ => None,
    }
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

/// Create an empty subboard folder `boards/<slug>-<id>/` inside `parent`;
/// returns its board-relative path.
pub fn create_subboard(parent: &Path, title: &str) -> Result<String, String> {
    let slug: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "board".to_string() } else { slug.chars().take(40).collect() };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let rel = format!("{SUBBOARDS_DIR}/{slug}-{}", &id[..6]);
    let dir = parent.join(&rel);
    std::fs::create_dir_all(dir.join(ASSETS_DIR)).map_err(|e| format!("Create {}: {e}", dir.display()))?;
    let json = serde_json::json!({ "app": "BoardCreate", "version": 1, "title": title, "camera": { "x": 0, "y": 0, "zoom": 1 }, "cards": [] });
    save(&dir, &serde_json::to_string_pretty(&json).unwrap())?;
    Ok(rel)
}

/// Copy a folder tree, keeping files that already exist at the target.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        let t = entry.file_type().map_err(|e| e.to_string())?;
        if t.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if t.is_file() && !target.exists() {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Copy all assets (and subboards) from one board folder into another (Save As).
pub fn copy_assets(from: &Path, to: &Path) -> Result<(), String> {
    let src = from.join(ASSETS_DIR);
    let dst = to.join(ASSETS_DIR);
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    if from == to {
        return Ok(());
    }
    let subs = from.join(SUBBOARDS_DIR);
    if subs.is_dir() {
        // Refuse to copy a board into its own subtree (it would recurse forever).
        if to.starts_with(from) {
            return Err("Choose a folder outside the current board.".into());
        }
        copy_tree(&subs, &to.join(SUBBOARDS_DIR))?;
    }
    if !src.exists() {
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
    fn subboards_are_created_and_copied() {
        let root = std::env::temp_dir().join(format!("bc-sub-{}", uuid::Uuid::new_v4().simple()));
        let parent = root.join("parent");
        std::fs::create_dir_all(parent.join(ASSETS_DIR)).unwrap();
        std::fs::write(parent.join(ASSETS_DIR).join("a.png"), b"x").unwrap();
        let rel = create_subboard(&parent, "Ideas: Rooftop / Night!").unwrap();
        assert!(rel.starts_with("boards/ideas-rooftop-night-"), "{rel}");
        let child = parent.join(&rel);
        assert!(load(&child).unwrap().contains("\"title\": \"Ideas: Rooftop / Night!\""));
        std::fs::write(child.join(ASSETS_DIR).join("b.png"), b"y").unwrap();
        let grand = create_subboard(&child, "").unwrap();
        assert!(grand.starts_with("boards/board-"));

        let copy = root.join("copy");
        copy_assets(&parent, &copy).unwrap();
        assert!(copy.join(ASSETS_DIR).join("a.png").exists());
        assert!(copy.join(&rel).join(ASSETS_DIR).join("b.png").exists());
        assert!(copy.join(&rel).join(&grand).join(BOARD_FILE).exists());
        assert!(copy_assets(&parent, &parent.join("inside")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sniffs_image_types() {
        assert_eq!(sniff_image_ext(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0"), Some("png"));
        assert_eq!(sniff_image_ext(b"<!doctype html><html>"), None);
    }

    #[test]
    fn rejects_escaping_paths() {
        let b = Path::new("C:/boards/x");
        assert!(resolve_asset(b, "../secret.png").is_err());
        assert!(resolve_asset(b, "C:/Windows/x.png").is_err());
        assert!(resolve_asset(b, "assets/a.png").is_ok());
    }
}
