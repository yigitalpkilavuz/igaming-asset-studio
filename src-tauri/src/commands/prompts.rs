//! Prompt commands: deterministic assembly + per-asset subject storage. No LLM.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{projects_root, find_descriptor, now_ms};
use crate::model::asset::Production;
use crate::model::asset_record::{AssetRecord, PromptState};
use crate::model::game_config::GameConfig;
use crate::prompts::assemble;
use crate::{secrets, storage, taxonomy};

/// Store the OpenAI API key in the OS keychain (used for gpt-image-1). Empty clears it.
#[tauri::command]
#[specta::specta]
pub fn set_openai_key(key: String) -> Result<(), String> {
    secrets::set(secrets::OPENAI_KEY, key.trim())
}

/// Store the Gemini API key ("Nano Banana"). Empty clears it.
#[tauri::command]
#[specta::specta]
pub fn set_gemini_key(key: String) -> Result<(), String> {
    secrets::set(secrets::GEMINI_KEY, key.trim())
}

/// Whether a Gemini key is configured.
#[tauri::command]
#[specta::specta]
pub fn gemini_key_present() -> bool {
    secrets::present(secrets::GEMINI_KEY)
}

/// Whether an OpenAI key is configured (for cloud image generation).
#[tauri::command]
#[specta::specta]
pub fn openai_key_present() -> bool {
    secrets::present(secrets::OPENAI_KEY)
}

/// Store the SpriteCook API key (`sc_live_…`). Empty clears it.
#[tauri::command]
#[specta::specta]
pub fn set_spritecook_key(key: String) -> Result<(), String> {
    secrets::set(secrets::SPRITECOOK_KEY, key.trim())
}

/// Whether a SpriteCook key is configured.
#[tauri::command]
#[specta::specta]
pub fn spritecook_key_present() -> bool {
    secrets::present(secrets::SPRITECOOK_KEY)
}

/// Store the Gamelab Studio API key. Empty clears it.
#[tauri::command]
#[specta::specta]
pub fn set_gamelab_key(key: String) -> Result<(), String> {
    secrets::set(secrets::GAMELAB_KEY, key.trim())
}

/// Whether a Gamelab Studio key is configured.
#[tauri::command]
#[specta::specta]
pub fn gamelab_key_present() -> bool {
    secrets::present(secrets::GAMELAB_KEY)
}

/// Load an asset's record, returning a fresh empty one if it doesn't exist yet.
#[tauri::command]
#[specta::specta]
pub fn get_asset_record(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    Ok(storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(asset_key)))
}

/// List all existing asset records for a project.
#[tauri::command]
#[specta::specta]
pub fn list_asset_records(
    app: tauri::AppHandle,
    game_id: String,
) -> Result<Vec<AssetRecord>, String> {
    let base = projects_root(&app)?;
    storage::list_asset_records(&base, &game_id)
}

/// Save an asset's subject / overrides.
#[tauri::command]
#[specta::specta]
pub fn save_prompt(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    subject: String,
    override_prompt: String,
    negative_override: String,
    layered: bool,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(&asset_key));
    record.prompt.subject = subject;
    record.prompt.override_prompt = override_prompt;
    record.prompt.negative_override = negative_override;
    record.prompt.layered = layered;
    record.prompt.updated_at = now_ms();
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// The assembled prompt actually sent to a provider — for live preview in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssembledPrompt {
    pub positive: String,
    pub negative: String,
}

/// Assemble the final prompt from the (live) config + the given subject/overrides.
/// Pure — reflects unsaved config edits too.
#[tauri::command]
#[specta::specta]
pub fn assemble_prompt(
    config: GameConfig,
    asset_key: String,
    subject: String,
    override_prompt: String,
    negative_override: String,
    layered: bool,
) -> Result<AssembledPrompt, String> {
    let descriptor = find_descriptor(&config, &asset_key)?;
    let prompt = PromptState {
        subject,
        override_prompt,
        negative_override,
        layered,
        updated_at: 0.0,
    };
    let a = assemble::assemble(&config, &descriptor, &prompt);
    Ok(AssembledPrompt {
        positive: a.positive,
        negative: a.negative,
    })
}

/// Seed a starting `subject` for every raster asset that has none yet (symbols draw
/// from their config description; others from the descriptor). Deterministic, no API.
/// Returns the number seeded.
#[tauri::command]
#[specta::specta]
pub fn seed_subjects(app: tauri::AppHandle, game_id: String) -> Result<u32, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let assets = taxonomy::derive_assets(&project.config);

    let mut count = 0u32;
    for asset in assets.iter().filter(|a| a.production == Production::Raster) {
        let mut record = storage::read_asset_record(&base, &game_id, &asset.key)?
            .unwrap_or_else(|| AssetRecord::new(&asset.key));
        if !record.prompt.subject.trim().is_empty() || !record.prompt.override_prompt.trim().is_empty() {
            continue;
        }
        let seed = assemble::subject_seed(&project.config, asset);
        if seed.is_empty() {
            continue;
        }
        record.prompt.subject = seed;
        record.prompt.updated_at = now_ms();
        storage::write_asset_record(&base, &game_id, &record)?;
        count += 1;
    }
    Ok(count)
}
