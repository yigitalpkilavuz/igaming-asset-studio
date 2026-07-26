//! Post-processing commands: preflight, run pipeline, read processed images.

use base64::Engine;

use super::{projects_root, find_descriptor};
use crate::model::asset::AssetKind;
use crate::model::asset_record::AssetRecord;
use crate::processing::{self, Preflight};
use crate::storage;

/// Report which external post-processing tools are available.
#[tauri::command]
#[specta::specta]
pub fn preflight() -> Preflight {
    processing::detect()
}

/// Run the post-processing pipeline for one variation, producing shippable finals.
#[tauri::command]
#[specta::specta]
pub async fn process_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    process_variation_core(base, game_id, asset_key, variation_id).await
}

/// App-free core of `process_variation` — shared with the headless CLI.
pub async fn process_variation_core(
    base: std::path::PathBuf,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<AssetRecord, String> {
    let project = storage::read_project(&base, &game_id)?;
    let descriptor = find_descriptor(&project.config, &asset_key)?;

    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let idx = record
        .variations
        .iter()
        .position(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;

    let author_w = descriptor
        .author_w
        .ok_or_else(|| "asset has no author dimensions to process to".to_string())?;
    let author_h = descriptor
        .author_h
        .ok_or_else(|| "asset has no author dimensions to process to".to_string())?;
    let opaque = matches!(descriptor.kind, AssetKind::Background | AssetKind::Splash);
    let bg = record.variations[idx].background;

    let raw_path =
        storage::asset_dir(&base, &game_id, &asset_key).join(&record.variations[idx].raw_file);

    // Scene-class profile: cut-out keying + exact-size export come from the declared
    // scene asset class (particles ship at their EXACT tiny author size).
    let scene_def = project.config.scene.def_for_key(&asset_key);
    let chroma_sweep = scene_def.is_some_and(|d| d.cutouts);
    let exact_size =
        scene_def.is_some_and(|d| d.kind == crate::model::game_config::SceneKind::Particle);

    // Ship at the GENERATED resolution (keep quality), not the small nominal author size —
    // preserve the author aspect ratio, never downscale below the source, cap at 2048.
    // Exact-size classes (particles) override this and ship at the author dims.
    let src_dims = image::image_dimensions(&raw_path).ok();
    let (target_w, target_h) = if exact_size {
        (author_w.max(1), author_h.max(1))
    } else {
        final_target(author_w, author_h, src_dims)
    };
    // Scale any 9-slice insets to match the larger output.
    let (sx, sy) = (
        target_w as f64 / author_w.max(1) as f64,
        target_h as f64 / author_h.max(1) as f64,
    );
    let nine_slice = descriptor.nine_slice.map(|ins| crate::model::asset::Insets {
        top: (ins.top as f64 * sy).round() as u32,
        right: (ins.right as f64 * sx).round() as u32,
        bottom: (ins.bottom as f64 * sy).round() as u32,
        left: (ins.left as f64 * sx).round() as u32,
    });
    let stages_dir = storage::variation_dir(&base, &game_id, &asset_key, &variation_id).join("stages");
    let pre = processing::detect();

    // Symbols get their subject normalized to the tier's VISUAL-MASS target: the eye
    // judges symbol importance by covered area, not edge length, so a thin instrument
    // and a wide book in the same tier must carry the same mass. Targets + the extent
    // clamp are per-project config; the per-symbol nudge rides on top.
    let sym_info = project.config.symbol_for_asset(&asset_key);
    let mass = if descriptor.kind == AssetKind::Symbol && !sym_info.is_some_and(|(_, exp)| exp) {
        // The `_expanded` twin of an expanding wild skips the fit pass entirely — the
        // per-cell ink/height targets are meaningless for a full-column composition.
        let sym = sym_info.map(|(s, _)| s);
        let sizing = project.config.symbol_sizing;
        Some(processing::normalize::MassSpec {
            class: sym.map(|s| sizing.class_for(s.role)).unwrap_or(sizing.high),
            area_weight: sizing.area_weight,
            centroid_bias: sizing.centroid_bias,
            alpha_floor: sizing.alpha_floor,
            safe_w: sizing.safe_w,
            safe_h: sizing.safe_h,
            nudge: sym.map(|s| s.size_nudge).unwrap_or(1.0),
            canvas: sizing.canvas,
        })
    } else {
        None
    };

    // Tone spec: symbols use their class band (per-symbol override wins); scene
    // assets use their declared tonalTarget. No band → no pass.
    let tone_spec = {
        let t = project.config.symbol_tone;
        let band = if descriptor.kind == AssetKind::Symbol {
            project.config.symbol_for_asset(&asset_key).map(|(sym, _)| {
                match sym.tone_target {
                    Some(m) => (m, m),
                    None => {
                        let b = t.class_for(sym.role);
                        (b.min, b.max)
                    }
                }
            })
        } else {
            scene_def.and_then(|d| d.tonal_target).map(|b| (b.min, b.max))
        };
        band.map(|(min, max)| crate::processing::tone::ToneSpec {
            min,
            max,
            alpha_floor: t.alpha_floor,
            ceiling: t.ceiling,
            gamma_lo: t.gamma_lo,
            gamma_hi: t.gamma_hi,
        })
    };

    // Scene exports honor the project's WebP quality; everything else uses the default.
    let webp_quality = if descriptor.category == "scenes" {
        project.config.scene.webp_quality.clamp(10, 100) as f32
    } else {
        85.0
    };

    // Heavy image + subprocess work off the async runtime.
    let (stages, mass_report, tone_report) = tauri::async_runtime::spawn_blocking(move || {
        processing::run_pipeline(
            &raw_path,
            &stages_dir,
            target_w,
            target_h,
            opaque,
            bg,
            nine_slice,
            mass,
            tone_spec,
            webp_quality,
            chroma_sweep,
            &pre,
        )
    })
    .await
    .map_err(|e| format!("processing task failed: {e}"))??;

    record.variations[idx].stages = stages;
    record.variations[idx].mass_report = mass_report;
    record.variations[idx].tone_report = tone_report;
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Final ship dimensions: keep the SOURCE resolution as-is (so a 1024² generation or an
/// imported image is not downscaled to the small nominal author size), only shrinking when it
/// exceeds a 2048 cap (aspect preserved). Falls back to the author size when the source can't
/// be read.
fn final_target(author_w: u32, author_h: u32, src: Option<(u32, u32)>) -> (u32, u32) {
    const MAX: u32 = 2048;
    let Some((sw, sh)) = src else {
        return (author_w.max(1), author_h.max(1));
    };
    let (sw, sh) = (sw.max(1), sh.max(1));
    let long = sw.max(sh);
    if long <= MAX {
        (sw, sh)
    } else {
        let s = MAX as f64 / long as f64;
        (
            ((sw as f64 * s).round() as u32).max(1),
            ((sh as f64 * s).round() as u32).max(1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::final_target;

    #[test]
    fn keeps_source_resolution_capped_at_2048() {
        // A symbol generated at 1024² ships at 1024² (not shrunk to the ~220² author size).
        assert_eq!(final_target(220, 220, Some((1024, 1024))), (1024, 1024));
        // An imported background keeps its own resolution — NOT force-upscaled to the huge
        // 3200 author size (and no panic when author_long > cap).
        assert_eq!(final_target(3200, 1800, Some((1920, 1080))), (1920, 1080));
        // A source larger than the cap downscales, preserving aspect.
        assert_eq!(final_target(220, 220, Some((4096, 4096))), (2048, 2048));
        assert_eq!(final_target(3200, 1800, Some((4000, 3000))), (2048, 1536));
        // No readable source → author size.
        assert_eq!(final_target(220, 220, None), (220, 220));
    }
}

/// Read one of a variation's processed stage files as a data URL.
#[tauri::command]
#[specta::specta]
pub fn get_variation_stage_image(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    stage_name: String,
) -> Result<String, String> {
    let base = projects_root(&app)?;
    let record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record".to_string())?;
    let variation = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| "variation not found".to_string())?;
    let stage = variation
        .stages
        .iter()
        .find(|s| s.name == stage_name)
        .ok_or_else(|| format!("stage \"{stage_name}\" not found"))?;

    // `stage.file` is relative to the variation dir; make it relative to the asset dir.
    let rel = format!("variations/{}/{}", variation_id, stage.file);
    let bytes = storage::read_asset_file(&base, &game_id, &asset_key, &rel)?;
    let mime = match rel.rsplit('.').next() {
        Some("webp") => "image/webp",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}
