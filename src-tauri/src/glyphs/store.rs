//! Disk layout for the glyph library and per-asset glyph settings.

use std::path::{Path, PathBuf};

use super::doc::{AssetGlyph, GlyphInfo};
use crate::storage;

/// Ids become folder names — restrict them to the slug alphabet so they can never
/// escape the library dir.
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!("invalid glyph id \"{id}\""));
    }
    Ok(())
}

/// The cross-game library folder. Underscore-prefixed: it holds no `project.json`,
/// so the game list never mistakes it for a project.
pub fn library_dir(root: &Path) -> PathBuf {
    root.join("_glyphs")
}

pub fn glyph_dir(root: &Path, id: &str) -> PathBuf {
    library_dir(root).join(id)
}

pub fn shape_path(root: &Path, id: &str) -> PathBuf {
    glyph_dir(root, id).join("shape.png")
}

fn meta_path(root: &Path, id: &str) -> PathBuf {
    glyph_dir(root, id).join("glyph.json")
}

pub fn read_glyph(root: &Path, id: &str) -> Result<Option<GlyphInfo>, String> {
    validate_id(id)?;
    let path = meta_path(root, id);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read glyph meta: {e}"))?;
    serde_json::from_str(&text).map(Some).map_err(|e| format!("parse glyph meta: {e}"))
}

pub fn write_glyph(root: &Path, info: &GlyphInfo) -> Result<(), String> {
    validate_id(&info.id)?;
    let dir = glyph_dir(root, &info.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create glyph dir: {e}"))?;
    let text = serde_json::to_string_pretty(info).map_err(|e| format!("serialize glyph: {e}"))?;
    std::fs::write(meta_path(root, &info.id), text).map_err(|e| format!("write glyph meta: {e}"))
}

/// Every library glyph, newest first.
pub fn list_glyphs(root: &Path) -> Result<Vec<GlyphInfo>, String> {
    let dir = library_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if let Ok(Some(info)) = read_glyph(root, &id) {
            out.push(info);
        }
    }
    out.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
    Ok(out)
}

pub fn delete_glyph(root: &Path, id: &str) -> Result<(), String> {
    validate_id(id)?;
    let dir = glyph_dir(root, id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("delete glyph: {e}"))?;
    }
    Ok(())
}

// ── Per-asset glyph settings ──────────────────────────────────────────────────

fn asset_glyph_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    storage::asset_dir(base, game_id, asset_key).join("glyph.json")
}

pub fn read_asset_glyph(
    base: &Path,
    game_id: &str,
    asset_key: &str,
) -> Result<Option<AssetGlyph>, String> {
    let path = asset_glyph_path(base, game_id, asset_key);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read glyph settings: {e}"))?;
    serde_json::from_str(&text).map(Some).map_err(|e| format!("parse glyph settings: {e}"))
}

pub fn write_asset_glyph(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    glyph: &AssetGlyph,
) -> Result<(), String> {
    let path = asset_glyph_path(base, game_id, asset_key);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create asset dir: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(glyph).map_err(|e| format!("serialize glyph settings: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write glyph settings: {e}"))
}
