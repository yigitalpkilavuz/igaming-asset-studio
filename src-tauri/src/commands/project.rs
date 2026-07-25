//! Project + taxonomy commands.

use super::{projects_root, now_ms};
use crate::model::asset::AssetDescriptor;
use crate::model::game_config::GameConfig;
use crate::model::project::Project;
use crate::model::ProjectSummary;
use crate::{storage, taxonomy};

/// Derive the required-asset list for a config. Pure — no persistence.
#[tauri::command]
#[specta::specta]
pub fn derive_assets(config: GameConfig) -> Vec<AssetDescriptor> {
    taxonomy::derive_assets(&config)
}

/// Delete a game and everything under it — assets, takes, studio docs, exports.
/// Guarded: only removes a folder that actually contains a `project.json`.
#[tauri::command]
#[specta::specta]
pub fn delete_project(app: tauri::AppHandle, game_id: String) -> Result<(), String> {
    storage::validate_game_id(&game_id)?;
    let dir = storage::project_dir(&projects_root(&app)?, &game_id);
    if !dir.join("project.json").is_file() {
        return Err(format!("\"{game_id}\" is not a project folder"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("delete failed: {e}"))
}

/// Create a new project. Errors if one with the same id already exists.
#[tauri::command]
#[specta::specta]
pub fn create_project(app: tauri::AppHandle, config: GameConfig) -> Result<Project, String> {
    storage::validate_game_id(&config.game_id)?;
    let base = projects_root(&app)?;
    if storage::project_exists(&base, &config.game_id) {
        return Err(format!("A project \"{}\" already exists.", config.game_id));
    }
    let project = Project::new(config, now_ms());
    storage::write_project(&base, &project)?;
    Ok(project)
}

/// Save a config for an existing project, or create it if new. Preserves `created_at`.
#[tauri::command]
#[specta::specta]
pub fn save_project_config(app: tauri::AppHandle, config: GameConfig) -> Result<Project, String> {
    storage::validate_game_id(&config.game_id)?;
    let base = projects_root(&app)?;
    let now = now_ms();
    let project = match storage::read_project(&base, &config.game_id) {
        Ok(mut existing) => {
            existing.config = config;
            existing.updated_at = now;
            existing
        }
        Err(_) => Project::new(config, now),
    };
    storage::write_project(&base, &project)?;
    Ok(project)
}

/// Load one project by id.
#[tauri::command]
#[specta::specta]
pub fn get_project(app: tauri::AppHandle, game_id: String) -> Result<Project, String> {
    let base = projects_root(&app)?;
    storage::read_project(&base, &game_id)
}

/// List all projects (summaries), newest-updated first.
#[tauri::command]
#[specta::specta]
pub fn list_projects(app: tauri::AppHandle) -> Result<Vec<ProjectSummary>, String> {
    let base = projects_root(&app)?;
    storage::list_projects(&base)
}

/// The absolute projects root, for display in the UI.
#[tauri::command]
#[specta::specta]
pub fn projects_root_path(app: tauri::AppHandle) -> Result<String, String> {
    Ok(projects_root(&app)?.to_string_lossy().into_owned())
}
