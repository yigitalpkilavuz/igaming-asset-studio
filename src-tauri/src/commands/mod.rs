//! Tauri command surface. Thin wrappers over the model / taxonomy / storage / prompt
//! layers, split by domain.

pub mod assistant;
pub mod export;
pub mod generate;
pub mod glyphs;
pub mod layers;
pub mod process;
pub mod project;
pub mod prompts;
pub mod quality;
pub mod sheet;
pub mod studio;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;

use crate::model::asset::AssetDescriptor;
use crate::model::game_config::GameConfig;
use crate::taxonomy;

/// Find a derived asset descriptor by key for a config.
pub(crate) fn find_descriptor(
    config: &GameConfig,
    asset_key: &str,
) -> Result<AssetDescriptor, String> {
    taxonomy::derive_assets(config)
        .into_iter()
        .find(|a| a.key == asset_key)
        .ok_or_else(|| format!("asset \"{asset_key}\" is not part of this game's plan"))
}

/// Milliseconds since the Unix epoch, as `f64` (specta-friendly, exact past year 2100).
pub(crate) fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// Fixed app-data dir — holds `settings.json`. Never moves (we must read the
/// projects-root setting from here).
pub(crate) fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("could not resolve app data dir: {e}"))
}

/// The folder that holds project folders. Configurable via settings; defaults to
/// `<config_dir>/projects`.
pub(crate) fn projects_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config = config_dir(app)?;
    let settings = crate::settings::load(&config);
    let root = settings.projects_root.trim();
    Ok(if root.is_empty() {
        config.join("projects")
    } else {
        PathBuf::from(root)
    })
}
