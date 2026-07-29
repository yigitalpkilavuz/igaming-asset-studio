//! AI spritesheet generation via SpriteCook: upload an asset's APPROVED processed image,
//! animate it from a motion prompt, save the returned sheet under the asset. Complements
//! the Spine studio — right for FX loops and simple ambient motion where rigging is
//! overkill; the approved art stays the source.

use base64::Engine;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::studio::snapshot_active_png;
use super::projects_root;
use crate::providers::spritecook::{
    await_job, fetch_png, find_asset_id, find_asset_url, trunc_body, API_BASE,
};
use crate::storage;

/// Result of an AI-sheet run, persisted next to the asset.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AiSheet {
    /// Sheet PNG as a data URL (for immediate preview).
    pub data_url: String,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub prompt: String,
}

fn sheet_dir(base: &std::path::Path, game_id: &str, asset_key: &str) -> std::path::PathBuf {
    storage::asset_dir(base, game_id, asset_key).join("ai_sheet")
}

/// Metadata stored beside the sheet so it survives restarts.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SheetMeta {
    frames: u32,
    prompt: String,
}

/// The saved AI sheet for an asset, if any.
#[tauri::command]
#[specta::specta]
pub fn get_ai_sheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<Option<AiSheet>, String> {
    let base = projects_root(&app)?;
    let dir = sheet_dir(&base, &game_id, &asset_key);
    let png_path = dir.join("sheet.png");
    if !png_path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&png_path).map_err(|e| format!("read sheet: {e}"))?;
    let meta: SheetMeta = std::fs::read(dir.join("sheet.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(SheetMeta { frames: 8, prompt: String::new() });
    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode sheet: {e}"))?;
    Ok(Some(AiSheet {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ),
        frames: meta.frames,
        width: img.width(),
        height: img.height(),
        prompt: meta.prompt,
    }))
}

/// Generate an AI spritesheet for the asset's active PROCESSED image via SpriteCook.
/// `frames` is clamped to the detailed-mode range (2–24, even).
#[tauri::command]
#[specta::specta]
pub async fn generate_ai_sheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    prompt: String,
    frames: u32,
) -> Result<AiSheet, String> {
    let key = crate::secrets::get(crate::secrets::SPRITECOOK_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "SpriteCook API key is not set (add it in Settings).".to_string())?;
    if prompt.trim().is_empty() {
        return Err("Describe the motion first.".into());
    }
    let frames = (frames.clamp(2, 24) / 2) * 2;
    let base = projects_root(&app)?;
    let (_, png) = snapshot_active_png(&base, &game_id, &asset_key)?;

    let client = reqwest::Client::new();

    // 1. Import the approved image as a SpriteCook asset.
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let import_body = serde_json::json!({
        "image": data_url,
        "pixel": false,
        "display_name": format!("{game_id}/{asset_key}"),
        "file_name": format!("{asset_key}.png"),
    });
    let resp = client
        .post(format!("{API_BASE}/v1/api/assets/import"))
        .bearer_auth(&key)
        .json(&import_body)
        .send()
        .await
        .map_err(|e| format!("SpriteCook import failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("SpriteCook import error {status}: {}", trunc_body(&text)));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad import response: {e}"))?;
    let asset_id = find_asset_id(&parsed)
        .ok_or_else(|| format!("import returned no asset id: {}", trunc_body(&text)))?;

    // 2. Animate it into a spritesheet (synchronous; can take 30–90 s).
    let animate_body = serde_json::json!({
        "asset_id": asset_id,
        "prompt": prompt,
        "pixel": false,
        "output_frames": frames,
        "output_format": "spritesheet",
        "removebg": "Basic",
        "edge_margin": 6,
        // Anti-artifact floor for frame-to-frame coherence (style-neutral).
        "negative_prompt": "morphing, melting, warping identity, flickering, style drift \
between frames, changing colors between frames, extra limbs, deformed",
    });
    let resp = client
        .post(format!("{API_BASE}/v1/api/animate-sync"))
        .bearer_auth(&key)
        .json(&animate_body)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("SpriteCook animate failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("SpriteCook animate error {status}: {}", trunc_body(&text)));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad animate response: {e}"))?;
    // Animations queue (minutes at high frame counts); poll until the sheet URL appears,
    // relaying the job's progress to the bench.
    use tauri::Emitter;
    let app2 = app.clone();
    let done = await_job(
        &client,
        &key,
        parsed,
        &text,
        std::time::Duration::from_secs(900),
        move |v| {
            let progress = v.get("progress").and_then(|p| p.as_f64()).unwrap_or(0.0);
            let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let _ = app2.emit(
                "sheet://progress",
                serde_json::json!({ "progress": progress, "status": status }),
            );
        },
    )
    .await?;
    let url = find_asset_url(&done)
        .ok_or_else(|| "animate finished without a result URL".to_string())?;
    let sheet_png = fetch_png(&url).await?;

    // 3. Persist sheet + meta beside the asset.
    let dir = sheet_dir(&base, &game_id, &asset_key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create sheet dir: {e}"))?;
    std::fs::write(dir.join("sheet.png"), &sheet_png).map_err(|e| format!("write sheet: {e}"))?;
    let meta = SheetMeta { frames, prompt: prompt.clone() };
    std::fs::write(
        dir.join("sheet.json"),
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("encode meta: {e}"))?,
    )
    .map_err(|e| format!("write meta: {e}"))?;

    let img = image::load_from_memory(&sheet_png).map_err(|e| format!("decode sheet: {e}"))?;
    Ok(AiSheet {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&sheet_png)
        ),
        frames,
        width: img.width(),
        height: img.height(),
        prompt,
    })
}

/// Bake a short video clip into the SAME transparent FX spritesheet the SpriteCook lane produces
/// (`ai_sheet/sheet.png` + `sheet.json`), so preview + export reuse it unchanged. This is the
/// license-safe lane for motion a rig can't author (flames, bursts, transformations): the video is
/// bring-your-own (an imported file), matting is our own Rust cutters, and the clip never ships —
/// only the baked sprite does. `frames` is clamped to 2–24 (even); `bg` picks the matte, `loop_mode`
/// how the clip is made seamless.
#[tauri::command]
#[specta::specta]
pub async fn generate_video_sheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    src_path: String,
    bg: crate::studio::video::VideoBg,
    frames: u32,
    loop_mode: crate::studio::video::VideoLoop,
) -> Result<AiSheet, String> {
    let base = projects_root(&app)?;
    let video = std::path::PathBuf::from(&src_path);
    if !video.is_file() {
        return Err(format!("video not found: {src_path}"));
    }
    let dir = sheet_dir(&base, &game_id, &asset_key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create sheet dir: {e}"))?;

    // ffmpeg extraction + per-frame matte is blocking CPU/subprocess work — keep it off the async
    // runtime thread.
    let work = dir.join("_video_work");
    let (work2, video2) = (work.clone(), video.clone());
    let (strip_png, n) = tokio::task::spawn_blocking(move || {
        crate::studio::video::bake(&video2, bg, frames, loop_mode, &work2)
    })
    .await
    .map_err(|e| format!("bake task failed: {e}"))??;
    let _ = std::fs::remove_dir_all(&work);

    // Persist the sheet + meta beside the asset (same contract as the SpriteCook lane).
    std::fs::write(dir.join("sheet.png"), &strip_png).map_err(|e| format!("write sheet: {e}"))?;
    let name = video.file_name().and_then(|s| s.to_str()).unwrap_or("video").to_string();
    let prompt = format!("video: {name}");
    let meta = SheetMeta { frames: n, prompt: prompt.clone() };
    std::fs::write(
        dir.join("sheet.json"),
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("encode meta: {e}"))?,
    )
    .map_err(|e| format!("write meta: {e}"))?;

    let img = image::load_from_memory(&strip_png).map_err(|e| format!("decode sheet: {e}"))?;
    Ok(AiSheet {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&strip_png)
        ),
        frames: n,
        width: img.width(),
        height: img.height(),
        prompt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_id_hunt_covers_envelope_shapes() {
        for v in [
            serde_json::json!({ "asset_id": "a1" }),
            serde_json::json!({ "asset": { "id": "a1" } }),
            serde_json::json!({ "data": { "assetId": "a1" } }),
        ] {
            assert_eq!(find_asset_id(&v).as_deref(), Some("a1"), "{v}");
        }
        assert!(find_asset_id(&serde_json::json!({ "status": "ok" })).is_none());
    }
}
