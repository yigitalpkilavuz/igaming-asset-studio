use serde::{Deserialize, Serialize};
use specta::Type;

use super::game_config::GameConfig;

/// Bumped when the on-disk `project.json` schema changes in a breaking way.
pub const SCHEMA_VERSION: u32 = 1;

/// A persisted project: the game config plus project-level metadata. Canonical copy
/// lives at `<projects_root>/<game_id>/project.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub schema_version: u32,
    pub config: GameConfig,
    /// Unix epoch milliseconds.
    pub created_at: f64,
    /// Unix epoch milliseconds.
    pub updated_at: f64,
    /// Last folder the game's finished assets were published to (remembered for one-click
    /// re-publish). Absolute path.
    #[serde(default)]
    pub publish_dir: Option<String>,
}

/// Lightweight listing entry for the projects picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub game_id: String,
    pub name: String,
    pub updated_at: f64,
    pub symbol_count: u32,
}

impl Project {
    pub fn new(config: GameConfig, now_ms: f64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            config,
            created_at: now_ms,
            updated_at: now_ms,
            publish_dir: None,
        }
    }

    pub fn summary(&self) -> ProjectSummary {
        ProjectSummary {
            game_id: self.config.game_id.clone(),
            name: self.config.name.clone(),
            updated_at: self.updated_at,
            symbol_count: self.config.symbols.len() as u32,
        }
    }
}
