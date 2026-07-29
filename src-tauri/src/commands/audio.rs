//! Audio-generation commands: gambling-safe audio providers, their keys, and generate.

use base64::Engine;

use super::{config_dir, find_descriptor, now_ms, projects_root};
use crate::commands::generate::next_variation_id;
use crate::model::asset::AssetKind;
use crate::model::asset_record::{AssetRecord, Variation, VariationStatus};
use crate::model::game_config::{AudioCueDef, AudioKind, GameConfig};
use crate::providers::audio::{self, AudioProviderInfo, AudioRequest};
use crate::settings;
use crate::{secrets, storage};

/// List the gambling-safe audio providers and whether each is configured.
#[tauri::command]
#[specta::specta]
pub fn list_audio_providers(app: tauri::AppHandle) -> Result<Vec<AudioProviderInfo>, String> {
    Ok(audio::list_audio_providers(&settings::load(&config_dir(&app)?)))
}

#[tauri::command]
#[specta::specta]
pub fn set_stability_key(key: String) -> Result<(), String> {
    secrets::set(secrets::STABILITY_KEY, &key)
}
#[tauri::command]
#[specta::specta]
pub fn stability_key_present() -> bool {
    secrets::present(secrets::STABILITY_KEY)
}
#[tauri::command]
#[specta::specta]
pub fn set_vertex_token(token: String) -> Result<(), String> {
    secrets::set(secrets::VERTEX_KEY, &token)
}
#[tauri::command]
#[specta::specta]
pub fn vertex_token_present() -> bool {
    secrets::present(secrets::VERTEX_KEY)
}

/// Read an audio file for playback as a `data:` URL with the correct audio mime.
/// `name` = `"raw"` (the generated source) or a stage name (`"wav"` = the normalized take).
#[tauri::command]
#[specta::specta]
pub fn get_variation_audio(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    name: String,
) -> Result<String, String> {
    let base = projects_root(&app)?;
    let record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record".to_string())?;
    let var = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| "variation not found".to_string())?;
    let rel = if name == "raw" {
        var.raw_file.clone()
    } else {
        let stage = var
            .stages
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("stage \"{name}\" not found"))?;
        format!("variations/{variation_id}/{}", stage.file)
    };
    let bytes = storage::read_asset_file(&base, &game_id, &asset_key, &rel)?;
    let mime = match rel.rsplit('.').next() {
        Some("webm") => "audio/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// The audio cue backing an asset key (from `config.audio.cues`).
fn cue_for<'a>(config: &'a GameConfig, key: &str) -> Option<&'a AudioCueDef> {
    config.audio.cues.iter().find(|c| c.key == key)
}

/// Build the text-to-audio prompt for a cue: the game's audio **style master** + the cue's own
/// description + style-neutral kind scaffolding. (House style lives in the style master, never
/// hardcoded here — same rule as image prompts.)
pub fn assemble_audio_prompt(config: &GameConfig, cue: &AudioCueDef) -> String {
    let mut parts: Vec<String> = Vec::new();
    let style = config.audio.style_prompt.trim();
    if !style.is_empty() {
        parts.push(style.to_string());
    }
    let subject = cue.description.trim();
    parts.push(if subject.is_empty() {
        cue.name.clone()
    } else {
        subject.to_string()
    });
    match cue.kind {
        AudioKind::Music => parts.push("A music bed for a slot game; clean full mix, no voice.".into()),
        AudioKind::Sfx => {
            parts.push("A short game sound effect; clean and punchy, no music bed, no voice.".into())
        }
    }
    if cue.looped {
        parts.push("Must loop seamlessly with no audible seam.".into());
    }
    parts.join(" ")
}

/// Generate audio variation(s) for a music/sfx asset — the audio analogue of
/// `generate_variation`. `count` (1–4) requests run concurrently. Returns the updated record;
/// errors only when EVERY request failed.
#[tauri::command]
#[specta::specta]
pub async fn generate_audio_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    provider_id: String,
    count: u32,
) -> Result<AssetRecord, String> {
    let cfg = settings::load(&config_dir(&app)?);
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let descriptor = find_descriptor(&project.config, &asset_key)?;
    let kind = match descriptor.kind {
        AssetKind::Music => AudioKind::Music,
        AssetKind::Sfx => AudioKind::Sfx,
        _ => return Err(format!("{asset_key} is not an audio asset")),
    };
    let cue = cue_for(&project.config, &asset_key)
        .ok_or_else(|| format!("no audio cue \"{asset_key}\" in the config"))?
        .clone();
    let prompt = assemble_audio_prompt(&project.config, &cue);

    let provider = audio::build_audio_provider(&provider_id, &cfg)?;
    if !provider.configured() {
        return Err(format!("Audio provider \"{provider_id}\" is not configured."));
    }
    match kind {
        AudioKind::Music if !provider.does_music() => {
            return Err(format!("{} can't make music", provider.display_name()))
        }
        AudioKind::Sfx if !provider.does_sfx() => {
            return Err(format!("{} can't make sound effects", provider.display_name()))
        }
        _ => {}
    }

    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(&asset_key));
    let parent = record.active_variation.clone();
    let count = count.clamp(1, 4) as usize;

    let outs = futures::future::join_all((0..count).map(|_| {
        let req = AudioRequest {
            prompt: prompt.clone(),
            kind,
            looped: cue.looped,
            target_secs: cue.target_secs,
            seed: Some(random_seed()),
        };
        let provider = &provider;
        async move { (req.seed, provider.generate(&req).await) }
    }))
    .await;

    let mut errors = Vec::new();
    for (used_seed, out) in outs {
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let var_id = next_variation_id(&record);
        let raw_name = format!("raw.{}", out.format);
        let raw_file =
            storage::write_variation_file(&base, &game_id, &asset_key, &var_id, &raw_name, &out.bytes)?;
        record.variations.push(Variation {
            id: var_id.clone(),
            parent: parent.clone(),
            created_at: now_ms(),
            prompt_snapshot: prompt.clone(),
            provider: provider_id.clone(),
            model: None,
            seed: used_seed.map(|s| s as f64),
            raw_file,
            status: VariationStatus::Ready,
            background: Default::default(),
            stages: Vec::new(),
            mass_report: None,
            tone_report: None,
            audio_report: None,
            locked: false,
        });
        record.active_variation = Some(var_id);
    }
    if record.active_variation == parent && !errors.is_empty() {
        return Err(errors.remove(0));
    }
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Process an audio variation: ffmpeg loudness-normalize + transcode to the webm+mp3 twins.
/// The audio analogue of `process_variation`. Returns the updated record.
#[tauri::command]
#[specta::specta]
pub async fn process_audio_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let looped = cue_for(&project.config, &asset_key)
        .map(|c| c.looped)
        .unwrap_or(false);
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| format!("no asset record for {asset_key}"))?;
    let var_dir = storage::variation_dir(&base, &game_id, &asset_key, &variation_id);
    let raw_name = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("no variation {variation_id}"))?
        .raw_file
        .rsplit('/')
        .next()
        .unwrap_or("raw.wav")
        .to_string();
    let raw_path = var_dir.join(&raw_name);

    // ffmpeg is blocking — keep it off the async runtime thread.
    let (stages, report) = tokio::task::spawn_blocking(move || {
        crate::processing::audio::process_audio(&raw_path, &var_dir, looped)
    })
    .await
    .map_err(|e| format!("audio process task failed: {e}"))??;

    if let Some(var) = record.variations.iter_mut().find(|v| v.id == variation_id) {
        var.stages = stages;
        var.audio_report = Some(report);
    }
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (t.as_nanos() as u64) ^ (t.subsec_nanos() as u64).wrapping_mul(2_654_435_761)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::game_config::default_audio_cues;

    #[test]
    fn audio_prompt_layers_style_then_cue_then_scaffold() {
        let mut cfg = crate::model::game_config::GameConfig {
            game_id: "g".into(),
            name: "G".into(),
            brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
            win_type: crate::model::game_config::WinType::Lines,
            cols: 5,
            rows: 3,
            symbols: vec![],
            has_feature_background: false,
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
            has_audio: true,
            audio: Default::default(),
        };
        cfg.audio.style_prompt = "dark baroque orchestral".into();
        cfg.audio.cues = default_audio_cues();
        let music = cfg.audio.cues.iter().find(|c| c.key == "bgm_main").unwrap();
        let p = assemble_audio_prompt(&cfg, music);
        assert!(p.starts_with("dark baroque orchestral"), "style master leads: {p}");
        assert!(p.contains("background music"), "cue description present");
        assert!(p.to_lowercase().contains("loop"), "looped hint present");
    }
}
