//! Non-secret app settings (provider endpoints, model choices). Persisted as JSON in
//! the app data dir. Secrets (API keys) live in the keychain, not here.

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;

fn default_draw_things_url() -> String {
    "http://127.0.0.1:7860".to_string()
}

fn default_openai_image_model() -> String {
    "gpt-image-2".to_string()
}

fn default_gemini_image_model() -> String {
    // Nano Banana 2 — near-Pro quality at Flash price, and far better instruction
    // following (the flat-magenta chroma background) than the 2.5 original.
    "gemini-3.1-flash-image-preview".to_string()
}

fn default_spritecook_model() -> String {
    "gemini-3.1-flash-image".to_string()
}

fn default_openai_vision_model() -> String {
    "gpt-4o".to_string()
}

fn default_stable_audio_model() -> String {
    "stable-audio-2".to_string()
}

fn default_lyria_model() -> String {
    "lyria-002".to_string()
}

fn default_vertex_location() -> String {
    "us-central1".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// Base URL of the local Draw Things / A1111-compatible HTTP server.
    #[serde(default = "default_draw_things_url")]
    pub draw_things_url: String,
    /// OpenAI image model id.
    #[serde(default = "default_openai_image_model")]
    pub openai_image_model: String,
    /// Gemini image model id (Nano Banana 2; use gemini-3-pro-image-preview for the Pro tier).
    #[serde(default = "default_gemini_image_model")]
    pub gemini_image_model: String,
    /// SpriteCook generation model id.
    #[serde(default = "default_spritecook_model")]
    pub spritecook_model: String,
    /// OpenAI vision model used for part proposals / region labeling.
    #[serde(default = "default_openai_vision_model")]
    pub openai_vision_model: String,
    /// Absolute path to the folder that holds project folders. Empty = default
    /// (`<app_data_dir>/projects`). Lets projects live inside a repo or on an SSD.
    #[serde(default)]
    pub projects_root: String,
    /// Stability "Stable Audio" model id (music + SFX provider).
    #[serde(default = "default_stable_audio_model")]
    pub stable_audio_model: String,
    /// Google Lyria model id on Vertex AI.
    #[serde(default = "default_lyria_model")]
    pub lyria_model: String,
    /// Google Cloud project id for Vertex AI (the Lyria provider).
    #[serde(default)]
    pub vertex_project: String,
    /// Vertex AI region, e.g. `us-central1`.
    #[serde(default = "default_vertex_location")]
    pub vertex_location: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            draw_things_url: default_draw_things_url(),
            openai_image_model: default_openai_image_model(),
            gemini_image_model: default_gemini_image_model(),
            spritecook_model: default_spritecook_model(),
            openai_vision_model: default_openai_vision_model(),
            projects_root: String::new(),
            stable_audio_model: default_stable_audio_model(),
            lyria_model: default_lyria_model(),
            vertex_project: String::new(),
            vertex_location: default_vertex_location(),
        }
    }
}

fn settings_path(base: &Path) -> std::path::PathBuf {
    base.join("settings.json")
}

/// Load settings, falling back to defaults if the file is missing/unreadable.
pub fn load(base: &Path) -> AppSettings {
    let path = settings_path(base);
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Persist settings.
pub fn save(base: &Path, settings: &AppSettings) -> Result<(), String> {
    std::fs::create_dir_all(base).map_err(|e| format!("create dir failed: {e}"))?;
    let json = serde_json::to_vec_pretty(settings).map_err(|e| format!("serialize failed: {e}"))?;
    let path = settings_path(base);
    std::fs::write(&path, json).map_err(|e| format!("write settings failed: {e}"))
}
