//! Filesystem persistence. JSON manifests on disk are the canonical source of truth
//! (a project is a self-describing folder). All functions take an explicit `base`
//! directory so they are testable without a running Tauri app.

use std::fs;
use std::path::{Path, PathBuf};

use crate::model::asset_record::AssetRecord;
use crate::model::project::Project;
use crate::model::ProjectSummary;

/// `<projects_root>/<game_id>`. `root` is the folder that holds project folders.
pub fn project_dir(root: &Path, game_id: &str) -> PathBuf {
    root.join(game_id)
}

fn project_json_path(base: &Path, game_id: &str) -> PathBuf {
    project_dir(base, game_id).join("project.json")
}

/// Validate a game id is safe to use as a folder name.
pub fn validate_game_id(game_id: &str) -> Result<(), String> {
    if game_id.is_empty() {
        return Err("Game id must not be empty.".into());
    }
    if !game_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "Game id \"{game_id}\" must be lowercase letters, digits, underscore or hyphen."
        ));
    }
    Ok(())
}

/// Write JSON atomically: write to a temp file in the same dir, then rename.
pub(crate) fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "invalid path".to_string())?;
    fs::create_dir_all(dir).map_err(|e| format!("create dir failed: {e}"))?;
    let json = serde_json::to_vec_pretty(value).map_err(|e| format!("serialize failed: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json).map_err(|e| format!("write failed: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename failed: {e}"))?;
    Ok(())
}

/// Persist a project's `project.json`.
pub fn write_project(base: &Path, project: &Project) -> Result<(), String> {
    validate_game_id(&project.config.game_id)?;
    let path = project_json_path(base, &project.config.game_id);
    atomic_write_json(&path, project)
}

/// Load one project by id.
pub fn read_project(base: &Path, game_id: &str) -> Result<Project, String> {
    let path = project_json_path(base, game_id);
    let bytes = fs::read(&path).map_err(|e| format!("read {game_id} failed: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse {game_id} failed: {e}"))
}

/// Whether a project already exists on disk.
pub fn project_exists(base: &Path, game_id: &str) -> bool {
    project_json_path(base, game_id).is_file()
}

// ── Per-asset records (prompt + variations) ───────────────────────────────────

/// `<project>/assets/<asset_key>`
pub fn asset_dir(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    project_dir(base, game_id).join("assets").join(asset_key)
}

fn asset_json_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    asset_dir(base, game_id, asset_key).join("asset.json")
}

/// Load an asset record, or `None` if it has never been created.
pub fn read_asset_record(
    base: &Path,
    game_id: &str,
    asset_key: &str,
) -> Result<Option<AssetRecord>, String> {
    let path = asset_json_path(base, game_id, asset_key);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read {asset_key} failed: {e}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("parse {asset_key} failed: {e}"))
}

/// Persist an asset record.
pub fn write_asset_record(
    base: &Path,
    game_id: &str,
    record: &AssetRecord,
) -> Result<(), String> {
    let path = asset_json_path(base, game_id, &record.key);
    atomic_write_json(&path, record)
}

/// `<project>/assets/<asset_key>/variations/<var_id>`
pub fn variation_dir(base: &Path, game_id: &str, asset_key: &str, var_id: &str) -> PathBuf {
    asset_dir(base, game_id, asset_key)
        .join("variations")
        .join(var_id)
}

/// Write a variation image file and return its path relative to the asset dir.
pub fn write_variation_file(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    var_id: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let dir = variation_dir(base, game_id, asset_key, var_id);
    fs::create_dir_all(&dir).map_err(|e| format!("create variation dir failed: {e}"))?;
    fs::write(dir.join(filename), bytes).map_err(|e| format!("write image failed: {e}"))?;
    Ok(format!("variations/{var_id}/{filename}"))
}

/// Read a file stored relative to an asset dir (e.g. `variations/v001/raw.png`).
pub fn read_asset_file(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    rel_path: &str,
) -> Result<Vec<u8>, String> {
    let path = asset_dir(base, game_id, asset_key).join(rel_path);
    fs::read(&path).map_err(|e| format!("read {rel_path} failed: {e}"))
}

/// List all existing asset records for a project (those that have been touched).
pub fn list_asset_records(base: &Path, game_id: &str) -> Result<Vec<AssetRecord>, String> {
    let dir = project_dir(base, game_id).join("assets");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("read assets dir failed: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.path().is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if let Ok(Some(rec)) = read_asset_record(base, game_id, name) {
                out.push(rec);
            }
        }
    }
    Ok(out)
}

/// List all projects (summaries), newest-updated first. Skips unreadable entries.
/// `root` is the folder that holds project folders.
pub fn list_projects(root: &Path) -> Result<Vec<ProjectSummary>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| format!("read projects dir failed: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.path().is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if let Ok(project) = read_project(root, name) {
                out.push(project.summary());
            }
        }
    }
    out.sort_by(|a, b| {
        b.updated_at
            .partial_cmp(&a.updated_at)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::game_config::{GameConfig, WinType};

    fn cfg(id: &str) -> GameConfig {
        GameConfig {
            game_id: id.into(),
            name: id.into(),
            brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
            win_type: WinType::Lines,
            cols: 5,
            rows: 3,
            symbols: vec![],
            has_feature_background: true,
            has_buy_bonus: false,
            buy_bonus_modes: vec![],
            has_meter: false,
            meter_thresholds: 0,
            has_mystery: false,
            hold_and_spin: false,
            has_mascot: false,
            mascot_description: String::new(),
            symbol_sizing: Default::default(),
            symbol_tone: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
            has_audio: false,
            audio: Default::default(),
        }
    }

    #[test]
    fn round_trip_and_list() {
        let tmp = std::env::temp_dir().join(format!("wf_store_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);

        let p1 = Project::new(cfg("alpha"), 100.0);
        let mut p2 = Project::new(cfg("beta"), 200.0);
        p2.updated_at = 300.0;
        write_project(&tmp, &p1).unwrap();
        write_project(&tmp, &p2).unwrap();

        let back = read_project(&tmp, "alpha").unwrap();
        assert_eq!(back.config.game_id, "alpha");

        let list = list_projects(&tmp).unwrap();
        assert_eq!(list.len(), 2);
        // Newest-updated first.
        assert_eq!(list[0].game_id, "beta");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_bad_ids() {
        assert!(validate_game_id("Good_id-1").is_err()); // uppercase
        assert!(validate_game_id("bad id").is_err()); // space
        assert!(validate_game_id("good_id-1").is_ok());
    }
}
