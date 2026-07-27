//! Animation Studio command surface. Thin wrappers over `crate::studio`.

use base64::Engine;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{config_dir, now_ms, projects_root};
use crate::storage;
use crate::studio::doc::{Rect, SamLabel, SamPrompt, SourceRef, StudioDoc};
use crate::studio::preview::{PreviewBundle, StudioExportReport};
use crate::studio::sam::SamStatus;
use crate::studio::segment::PartProposal;
use crate::studio::doc::{Attachment, Clip};
use crate::studio::inpaint::InpaintStatus;
use crate::studio::{fx, inpaint, mesh, motion, preview, rig, sam, segment, sha256_hex, store};

/// Open (or create) the studio for an asset. On first open the active processed variation
/// is snapshotted to `studio/source.png` and a degenerate single-part doc with a canned
/// "idle" breathe clip is seeded — immediately previewable and exportable.
#[tauri::command]
#[specta::specta]
pub async fn studio_open(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    if let Some(mut doc) = store::read_doc(&base, &game_id, &asset_key)? {
        // Self-heal drifted docs (crashed sessions, version skew) so opening never
        // dead-ends on an inconsistent skeleton.
        let mut dirty = store::sanitize(&base, &game_id, &asset_key, &mut doc);
        dirty |= seed_motion_brief(&base, &game_id, &asset_key, &mut doc);
        // Follow the bench: when a NEWER take became active and the studio is still
        // pristine (nothing cut yet), silently re-snapshot the source so opening
        // Animate never shows stale art. Docs with real work keep their pixels — the
        // UI offers an explicit "Update source" instead.
        let pristine = doc.parts.len() == 1 && doc.parts[0].id == "all";
        if pristine {
            let active = crate::storage::read_asset_record(&base, &game_id, &asset_key)?
                .and_then(|r| r.active_variation);
            if active.is_some_and(|a| a != doc.source.variation_id) {
                resnapshot(&base, &game_id, &asset_key, &mut doc)?;
                dirty = true;
            }
        }
        if dirty {
            doc.updated_at = now_ms();
            store::write_doc(&base, &game_id, &asset_key, &doc)?;
        }
        return Ok(doc);
    }
    let (mut doc, _) = create_studio(&base, &game_id, &asset_key)?;
    if seed_motion_brief(&base, &game_id, &asset_key, &mut doc) {
        store::write_doc(&base, &game_id, &asset_key, &doc)?;
    }
    Ok(doc)
}

/// Fill an empty motion brief from the Blueprint's per-symbol animation note, so the
/// planned motion is on screen from the very first Cut step. Returns true if changed.
fn seed_motion_brief(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
    doc: &mut StudioDoc,
) -> bool {
    if !doc.motion_brief.trim().is_empty() {
        return false;
    }
    let Some(sym) = asset_key.strip_prefix("symbol_") else {
        return false;
    };
    let Ok(project) = storage::read_project(base, game_id) else {
        return false;
    };
    let planned = project
        .config
        .symbols
        .iter()
        .find(|s| s.key == sym)
        .map(|s| s.animation.trim().to_string())
        .unwrap_or_default();
    if planned.is_empty() {
        return false;
    }
    doc.motion_brief = planned;
    true
}

/// Persist the doc (stamps `updatedAt`).
#[tauri::command]
#[specta::specta]
pub async fn studio_save(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    mut doc: StudioDoc,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Re-snapshot `source.png` from the current active processed variation. Keeps the rest of
/// the doc; masks/cuts may become stale (the source hash changes, invalidating caches).
#[tauri::command]
#[specta::specta]
pub async fn studio_reimport_source(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset yet".to_string())?;
    resnapshot(&base, &game_id, &asset_key, &mut doc)?;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Re-snapshot `source.png` from the current active variation into an existing doc
/// (refreshes the degenerate whole-image part when it's still the only one).
fn resnapshot(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
    doc: &mut StudioDoc,
) -> Result<(), String> {
    let (source, png) = snapshot_source(base, game_id, asset_key)?;
    if doc.parts.len() == 1 && doc.parts[0].id == "all" {
        store::write_file(
            &store::part_dir(base, game_id, asset_key, "all").join("cut.png"),
            &png,
        )?;
        doc.parts[0].bbox = Some(crate::studio::doc::Rect {
            x: 0,
            y: 0,
            w: source.width,
            h: source.height,
        });
    }
    doc.source = source;
    Ok(())
}

/// Read a file under the asset's studio dir as a data URL (source, masks, cuts…).
#[tauri::command]
#[specta::specta]
pub fn studio_get_image(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    rel: String,
) -> Result<String, String> {
    if rel.contains("..") {
        return Err("invalid path".into());
    }
    let base = projects_root(&app)?;
    let path = store::studio_dir(&base, &game_id, &asset_key).join(&rel);
    let bytes = std::fs::read(&path).map_err(|e| format!("read {rel} failed: {e}"))?;
    let mime = match rel.rsplit('.').next() {
        Some("webp") => "image/webp",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// Fast path for keyframe editing: emit ONLY the skeleton JSON (same emitter as export),
/// so the frontend can reuse its cached atlas/textures. ~ms instead of re-encoding pages.
#[tauri::command]
#[specta::specta]
pub fn studio_skeleton_only(doc: StudioDoc) -> Result<String, String> {
    let skeleton = crate::studio::spine42::emit(&doc)?;
    serde_json::to_string(&skeleton).map_err(|e| format!("serialize skeleton failed: {e}"))
}

/// Build the preview bundle for a (possibly unsaved) doc — same emit+pack as export.
#[tauri::command]
#[specta::specta]
pub async fn studio_preview_bundle(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    doc: StudioDoc,
) -> Result<PreviewBundle, String> {
    let base = projects_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || preview::bundle(&base, &game_id, &asset_key, &doc))
        .await
        .map_err(|e| format!("preview task failed: {e}"))?
}

/// Save the doc, then write the Spine export set to `studio/export/`.
#[tauri::command]
#[specta::specta]
pub async fn studio_export(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    mut doc: StudioDoc,
) -> Result<StudioExportReport, String> {
    let base = projects_root(&app)?;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    tauri::async_runtime::spawn_blocking(move || preview::export(&base, &game_id, &asset_key, &doc))
        .await
        .map_err(|e| format!("export task failed: {e}"))?
}

// ── Segmentation (Phase 2) ────────────────────────────────────────────────────

/// Is a local SAM model available (and which tier)?
#[tauri::command]
#[specta::specta]
pub fn studio_sam_status(app: tauri::AppHandle) -> Result<SamStatus, String> {
    let config = config_dir(&app)?;
    Ok(match sam::weights::best_available(&config) {
        Some(model) => SamStatus::Ready {
            path: sam::weights::weights_path(&config, model).to_string_lossy().into_owned(),
            model: model.tag().to_string(),
        },
        None => SamStatus::Missing,
    })
}

/// Download SAM weights (`hq` = full ViT-B ~375 MB, else MobileSAM ~40 MB), emitting
/// `studio://sam-progress` events.
#[tauri::command]
#[specta::specta]
pub async fn studio_sam_download(app: tauri::AppHandle, hq: bool) -> Result<SamStatus, String> {
    let config = config_dir(&app)?;
    let model = if hq { sam::weights::SamModel::Base } else { sam::weights::SamModel::Tiny };
    let path = sam::weights::download(&app, &config, model).await?;
    Ok(SamStatus::Ready {
        path: path.to_string_lossy().into_owned(),
        model: model.tag().to_string(),
    })
}

/// Ask gpt-4o vision for a part plan (names + SAM seed points) for this asset's source.
#[tauri::command]
#[specta::specta]
pub async fn studio_propose_parts(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    hint: String,
) -> Result<Vec<PartProposal>, String> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    segment::set_vision_model(&crate::settings::load(&config_dir(&app)?).openai_vision_model);
    let base = projects_root(&app)?;
    let png = std::fs::read(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("read source failed: {e}"))?;
    segment::propose_parts(&key, &png, &hint).await
}

/// Region-first auto cut: SAM segments the whole source into candidate regions (grid
/// sweep, real boundaries), gpt-4o only LABELS them into named parts (no coordinate
/// guessing). Replaces the doc's parts with the labeled plan and persists their masks.
#[tauri::command]
#[specta::specta]
pub async fn studio_auto_cut(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    hint: String,
) -> Result<StudioDoc, String> {
    use crate::studio::regions;

    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let config = config_dir(&app)?;
    segment::set_vision_model(&crate::settings::load(&config).openai_vision_model);
    let Some(model) = sam::weights::best_available(&config) else {
        return Err("segmentation model not downloaded yet".into());
    };
    let weights = sam::weights::weights_path(&config, model);
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    // Snapshot current parts + masks so a disappointing auto-cut is one-click reversible.
    stash_autocut_undo(&base, &game_id, &asset_key, &doc)?;
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let sam_dir = store::studio_dir(&base, &game_id, &asset_key).join("sam");
    let sha = doc.source.sha256.clone();

    // 1. Discover regions + render the numbered overlay (blocking: SAM + image work).
    let source_c = source.clone();
    let (regions, overlay) = tauri::async_runtime::spawn_blocking(move || {
        let regions = regions::discover(&weights, model, &source_c, &sha, &sam_dir, 16)?;
        let overlay = regions::overlay_png(&source_c, &regions)?;
        Ok::<_, String>((regions, overlay))
    })
    .await
    .map_err(|e| format!("region discovery failed: {e}"))??;

    // 2. Label the regions (network) — the model sees the ORIGINAL art + the overlay.
    let original_png = {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode original: {e}"))?;
        buf.into_inner()
    };
    // Labeling is best-effort: on a network/LLM failure or an empty result, fall back to
    // the discovered regions as parts, so auto-cut always returns an editable result
    // instead of erroring (and wiping the snapshotted work).
    let labeled = regions::label(
        &api_key,
        &original_png,
        &overlay,
        regions.len(),
        &hint,
        &doc.motion_brief,
    )
    .await
    .unwrap_or_default();

    // 3. Build part masks (polished to click-path quality), replace parts, persist.
    tauri::async_runtime::spawn_blocking(move || {
        let (sw, sh) = source.dimensions();
        let mut rgb = image::RgbImage::new(sw, sh);
        let mut alpha_img = image::GrayImage::new(sw, sh);
        for (x, y, p) in source.enumerate_pixels() {
            let a = p.0[3] as u32;
            let blend = |c: u8| ((c as u32 * a + 128 * (255 - a)) / 255) as u8;
            rgb.put_pixel(x, y, image::Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
            alpha_img.put_pixel(x, y, image::Luma([p.0[3]]));
        }
        // Resolve to (id, name, raw mask). Labeled path: union each part's regions, then
        // repair connectivity ACROSS parts (stray fragments move to the neighbour they
        // touch). Fallback path (labeler failed/empty): one part per discovered region.
        let named: Vec<(String, String, bool, image::GrayImage)> = if !labeled.is_empty() {
            let mut raw_masks: Vec<(String, image::GrayImage)> = labeled
                .iter()
                .map(|lp| (lp.id.clone(), regions::part_mask(&regions, &lp.region_ids)))
                .collect();
            regions::repair_parts(&mut raw_masks);
            raw_masks
                .into_iter()
                .enumerate()
                .map(|(i, (id, m))| (id, labeled[i].name.clone(), labeled[i].deformable, m))
                .collect()
        } else {
            regions
                .iter()
                .map(|r| (format!("part_{}", r.id), format!("Part {}", r.id), false, r.mask.clone()))
                .collect()
        };

        let mut parts = Vec::new();
        for (id, name, deformable, raw) in &named {
            let mask = regions::polish_mask(raw, &rgb, &alpha_img);
            if segment::mask_bbox(&mask).is_none() {
                continue;
            }
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(mask)
                .write_to(&mut png, image::ImageFormat::Png)
                .map_err(|e| format!("encode mask for {id}: {e}"))?;
            let png = png.into_inner();
            store::write_file(
                &store::part_dir(&base, &game_id, &asset_key, id).join("mask.png"),
                &png,
            )?;
            parts.push(crate::studio::doc::Part {
                id: id.clone(),
                name: name.clone(),
                prompts: Vec::new(),
                bbox: None,
                mask_hash: Some(sha256_hex(&png)),
                completed_hash: None,
                completed_bbox: None,
                texture: crate::studio::doc::PartTexture::Cut,
                deformable: *deformable,
                attachment_only: false,
            });
        }
        if parts.is_empty() {
            return Err("auto-cut produced no usable parts — try manual selection".into());
        }
        doc.parts = parts;
        doc.updated_at = now_ms();
        store::write_doc(&base, &game_id, &asset_key, &doc)?;
        Ok(doc)
    })
    .await
    .map_err(|e| format!("mask build failed: {e}"))?
}

/// Snapshot the current studio doc + every part mask into a one-slot undo cache, so a
/// disappointing auto-cut can be reverted without losing hand-selected masks.
fn stash_autocut_undo(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
) -> Result<(), String> {
    let undo = store::studio_dir(base, game_id, asset_key).join("autocut_undo");
    let _ = std::fs::remove_dir_all(&undo);
    let doc_json = serde_json::to_string(doc).map_err(|e| format!("undo doc: {e}"))?;
    store::write_file(&undo.join("studio.json"), doc_json.as_bytes())?;
    for p in &doc.parts {
        let src = store::part_dir(base, game_id, asset_key, &p.id).join("mask.png");
        if src.is_file() {
            let bytes = std::fs::read(&src).map_err(|e| format!("read mask {}: {e}", p.id))?;
            store::write_file(&undo.join("parts").join(&p.id).join("mask.png"), &bytes)?;
        }
    }
    Ok(())
}

/// Is there an auto-cut snapshot to undo for this asset?
#[tauri::command]
#[specta::specta]
pub fn studio_autocut_undo_available(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<bool, String> {
    let base = projects_root(&app)?;
    Ok(store::studio_dir(&base, &game_id, &asset_key)
        .join("autocut_undo")
        .join("studio.json")
        .is_file())
}

/// Restore the parts + masks snapshotted before the last auto-cut (one-slot undo).
#[tauri::command]
#[specta::specta]
pub async fn studio_undo_auto_cut(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let undo = store::studio_dir(&base, &game_id, &asset_key).join("autocut_undo");
    let doc_path = undo.join("studio.json");
    if !doc_path.is_file() {
        return Err("nothing to undo — no snapshot from a previous auto-cut".into());
    }
    let bytes = std::fs::read(&doc_path).map_err(|e| format!("read undo doc: {e}"))?;
    let mut doc: StudioDoc =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse undo doc: {e}"))?;
    for p in &doc.parts {
        let snap = undo.join("parts").join(&p.id).join("mask.png");
        if snap.is_file() {
            let mb = std::fs::read(&snap).map_err(|e| format!("read undo mask {}: {e}", p.id))?;
            store::write_file(
                &store::part_dir(&base, &game_id, &asset_key, &p.id).join("mask.png"),
                &mb,
            )?;
        }
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    let _ = std::fs::remove_dir_all(&undo);
    Ok(doc)
}

/// A candidate mask for one part, previewed in the UI before the user accepts it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MaskResult {
    /// Full-source-size 0/255 gray PNG as a data URL.
    pub mask_data_url: String,
    /// Set pixels (post-refinement).
    pub area: u32,
    pub bbox: Option<Rect>,
}

/// Cloud "paint-out" cut: gpt-image-2 repaints EVERYTHING except the named part as one
/// flat magenta field; keying the result yields the part's mask. Slower and paid
/// (~10–30 s, cents per call) versus local SAM, but the model understands semantics
/// ("the left cable", "the glow behind the crown"), which can beat point-prompted
/// segmentation on hard art. Returns a candidate mask — same preview/apply flow as SAM.
#[tauri::command]
#[specta::specta]
pub async fn studio_cloud_cut(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    part_id: String,
) -> Result<MaskResult, String> {
    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let part = doc
        .parts
        .iter()
        .find(|p| p.id == part_id)
        .ok_or_else(|| format!("part \"{part_id}\" not found"))?;
    let target = if part.name.trim().is_empty() { part.id.clone() } else { part.name.clone() };

    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    cloud_paint_out(&api_key, source, &target).await
}

/// Shared paint-out core (studio parts + parallax layers): flatten onto magenta, ask
/// gpt-image-2 to repaint everything except `target` magenta, key the result, snap the
/// surviving mask to the original art's edges.
pub(crate) async fn cloud_paint_out(
    api_key: &str,
    source: image::RgbaImage,
    target: &str,
) -> Result<MaskResult, String> {
    let (w, h) = source.dimensions();

    // Flatten transparency onto magenta so "everything else" already reads magenta and
    // the model only has to extend it over the unwanted art. (Opaque sources pass
    // through unchanged.)
    let mut flat = image::RgbaImage::new(w, h);
    for (x, y, p) in source.enumerate_pixels() {
        let a = p.0[3] as u32;
        let blend = |c: u8, k: u8| ((c as u32 * a + k as u32 * (255 - a)) / 255) as u8;
        flat.put_pixel(
            x,
            y,
            image::Rgba([blend(p.0[0], 255), blend(p.0[1], 0), blend(p.0[2], 255), 255]),
        );
    }
    let mut flat_png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(flat)
        .write_to(&mut flat_png, image::ImageFormat::Png)
        .map_err(|e| format!("encode failed: {e}"))?;

    let prompt = format!(
        "Repaint this image so that EVERY pixel that is not part of the {target} becomes one \
solid, perfectly flat, uniform magenta (#FF00FF) — no gradient, texture or shadow in the \
magenta area. Keep the {target} itself COMPLETELY unchanged: identical pixels, position, \
scale, colours and edges — do not move, redraw, restyle or resize it. The output is only \
the untouched {target} on flat magenta, with a crisp clean boundary."
    );
    let size = crate::providers::openai_image::edit_size(w, h);
    let edited =
        crate::providers::openai_image::edit_image(api_key, flat_png.get_ref(), &prompt, &size, None)
            .await?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut out = image::load_from_memory(&edited)
            .map_err(|e| format!("decode edit failed: {e}"))?
            .to_rgba8();
        if out.dimensions() != (w, h) {
            out = image::imageops::resize(&out, w, h, image::imageops::FilterType::Triangle);
        }
        // Key the magenta out of the EDITED image, then intersect with the original
        // alpha: whatever survived is the part.
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(out)
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode failed: {e}"))?;
        let keyed_png =
            crate::processing::chromakey::chroma_key(&buf.into_inner(), crate::providers::CHROMA_RGB, 0.18)?;
        let keyed = image::load_from_memory(&keyed_png)
            .map_err(|e| format!("decode keyed failed: {e}"))?
            .to_rgba8();

        let mut mask = image::GrayImage::new(w, h);
        let mut rgb = image::RgbImage::new(w, h);
        let mut alpha = image::GrayImage::new(w, h);
        for (x, y, s) in source.enumerate_pixels() {
            rgb.put_pixel(x, y, image::Rgb([s.0[0], s.0[1], s.0[2]]));
            alpha.put_pixel(x, y, image::Luma([s.0[3]]));
            let k = keyed.get_pixel(x, y).0[3];
            mask.put_pixel(x, y, image::Luma([if k > 127 && s.0[3] >= 8 { 255 } else { 0 }]));
        }
        // Same clean-up the auto-cut applies: close, capped hole fill, guided-filter
        // snap to the ORIGINAL art's edges, alpha clip.
        let polished = crate::studio::regions::polish_mask(&mask, &rgb, &alpha);
        let area = polished.pixels().filter(|p| p.0[0] > 127).count() as u32;
        if area < 16 {
            return Err(
                "the paint-out left nothing — the model likely repainted the part too; try again or use clicks"
                    .to_string(),
            );
        }
        let bbox = crate::studio::matte::mask_bbox(&polished);
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(polished)
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| format!("encode mask failed: {e}"))?;
        Ok(MaskResult {
            mask_data_url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(png.into_inner())
            ),
            area,
            bbox,
        })
    })
    .await
    .map_err(|e| format!("cloud cut task failed: {e}"))?
}

/// Run SAM with the given click prompts against this asset's source and return the
/// refined candidate mask. Pure preview — nothing is persisted.
#[tauri::command]
#[specta::specta]
pub async fn studio_segment(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    prompts: Vec<SamPrompt>,
) -> Result<MaskResult, String> {
    let config = config_dir(&app)?;
    let Some(model) = sam::weights::best_available(&config) else {
        return Err("segmentation model not downloaded yet".into());
    };
    let weights = sam::weights::weights_path(&config, model);
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let sam_dir = store::studio_dir(&base, &game_id, &asset_key).join("sam");

    let sha = doc.source.sha256.clone();
    tauri::async_runtime::spawn_blocking(move || {
        sam_segment_blocking(&weights, model, &source, &sha, &sam_dir, &prompts, None, false)
    })
    .await
    .map_err(|e| format!("segment task failed: {e}"))?
}

/// One SAM click-to-selection round trip (blocking; run inside `spawn_blocking`):
/// composite over neutral gray → SAM → upscale → guided edge snap → refine. Shared by
/// the studio (fills all holes) and the layers view (`hole_cap_px` keeps real openings).
///
/// `union_points`: SAM treats multiple positive points as ONE object constraint, which
/// collapses disjoint regions (a tree left + rocks right) to the dominant blob. With
/// union semantics each positive point gets its own decode (encoder embedding is cached;
/// decodes are ~40 ms) and the masks are unioned — right for scenery depth bands.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sam_segment_blocking(
    weights: &std::path::Path,
    model: crate::studio::sam::weights::SamModel,
    source: &image::RgbaImage,
    source_sha: &str,
    sam_dir: &std::path::Path,
    prompts: &[SamPrompt],
    hole_cap_px: Option<u32>,
    union_points: bool,
) -> Result<MaskResult, String> {
    let (w, h) = source.dimensions();
    // SAM sees the art composited over neutral gray; alpha clips the final mask.
    let mut rgb = image::RgbImage::new(w, h);
    let mut alpha = image::GrayImage::new(w, h);
    for (x, y, p) in source.enumerate_pixels() {
        let a = p.0[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 128 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
        alpha.put_pixel(x, y, image::Luma([p.0[3]]));
    }
    let sam_prompts: Vec<(f64, f64, bool)> = prompts
        .iter()
        .map(|p| (p.x as f64, p.y as f64, p.label == SamLabel::Positive))
        .collect();
    let positives: Vec<(f64, f64, bool)> =
        sam_prompts.iter().filter(|p| p.2).copied().collect();
    let negatives: Vec<(f64, f64, bool)> =
        sam_prompts.iter().filter(|p| !p.2).copied().collect();
    if positives.is_empty() {
        return Err("add at least one positive point".into());
    }

    let resized = sam::with_engine(weights, model, |engine| {
        if union_points && positives.len() > 1 {
            // One decode per positive (each with all negatives), unioned per-pixel.
            let mut acc: Option<image::GrayImage> = None;
            for pos in &positives {
                let mut ps = vec![*pos];
                ps.extend_from_slice(&negatives);
                let m = engine.segment(source_sha, &rgb, &ps, Some(sam_dir))?;
                acc = Some(match acc {
                    None => m,
                    Some(mut u) => {
                        for (up, mp) in u.pixels_mut().zip(m.pixels()) {
                            up.0[0] = up.0[0].max(mp.0[0]);
                        }
                        u
                    }
                });
            }
            Ok(acc.unwrap())
        } else {
            engine.segment(source_sha, &rgb, &sam_prompts, Some(sam_dir))
        }
    })?;
    // Scale the mask back to source dims (bilinear soft boundary), then snap that soft
    // boundary onto the guide image's actual color edges — SAM works at 1024 internally,
    // so without this the silhouette is mushy/stair-stepped at full resolution.
    let full = image::imageops::resize(&resized, w, h, image::imageops::FilterType::Triangle);
    let soft: Vec<f32> = full.pixels().map(|p| p.0[0] as f32 / 255.0).collect();
    let gf_radius = ((w.max(h) as usize) / 200).clamp(4, 12);
    let snapped = crate::studio::matte::guided_filter_rgb(&soft, &rgb, gf_radius, 1e-4);
    let mut raw = image::GrayImage::new(w, h);
    for (px, &v) in raw.pixels_mut().zip(snapped.iter()) {
        px.0[0] = if v > 0.5 { 255 } else { 0 };
    }
    let positives_px: Vec<(u32, u32)> = prompts
        .iter()
        .filter(|p| p.label == SamLabel::Positive)
        .map(|p| {
            (
                (p.x * (w - 1) as f64).round() as u32,
                (p.y * (h - 1) as f64).round() as u32,
            )
        })
        .collect();
    let refined = crate::studio::matte::refine(&raw, Some(&alpha), &positives_px, hole_cap_px);

    let bbox = segment::mask_bbox(&refined);
    let area = refined.pixels().filter(|p| p.0[0] > 127).count() as u32;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(refined)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode mask: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    Ok(MaskResult { mask_data_url: format!("data:image/png;base64,{b64}"), area, bbox })
}

/// Persist a part's mask (from an accepted SAM result or manual brush work) and the
/// prompts that produced it. The mask must be a full-source-size gray PNG data URL.
#[tauri::command]
#[specta::specta]
pub async fn studio_set_mask(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    part_id: String,
    mask_data_url: String,
    prompts: Vec<SamPrompt>,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;

    let b64 = mask_data_url
        .rsplit("base64,")
        .next()
        .ok_or_else(|| "bad mask data URL".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("decode mask: {e}"))?;
    let mask = image::load_from_memory(&bytes)
        .map_err(|e| format!("parse mask: {e}"))?
        .to_luma8();
    if mask.dimensions() != (doc.source.width, doc.source.height) {
        return Err(format!(
            "mask is {}×{}, expected {}×{}",
            mask.width(),
            mask.height(),
            doc.source.width,
            doc.source.height
        ));
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(mask)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode mask: {e}"))?;
    let png = png.into_inner();
    store::write_file(
        &store::part_dir(&base, &game_id, &asset_key, &part_id).join("mask.png"),
        &png,
    )?;

    let part = doc
        .parts
        .iter_mut()
        .find(|p| p.id == part_id)
        .ok_or_else(|| format!("part \"{part_id}\" not in doc — save the doc first"))?;
    part.mask_hash = Some(sha256_hex(&png));
    part.prompts = prompts;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Cut all masked parts from the original pixels and rebuild the static skeleton.
#[tauri::command]
#[specta::specta]
pub async fn studio_cut_parts(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        segment::cut_parts(&base, &game_id, &asset_key, doc)
    })
    .await
    .map_err(|e| format!("cut task failed: {e}"))?
}

// ── Rig (Phase 3) ─────────────────────────────────────────────────────────────

/// Auto-rig the cut parts: mask analysis (adjacency, PCA, joint bands) in Rust; the parent
/// tree from gpt-4o when a key is set, else a pure heuristic. Replaces `doc.bones`.
#[tauri::command]
#[specta::specta]
pub async fn studio_auto_rig(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    if !doc.parts.iter().any(|p| p.bbox.is_some() && p.mask_hash.is_some()) {
        return Err("cut parts first — auto-rig needs segmented parts".into());
    }

    // Load masks off the async runtime and analyze.
    let analysis = {
        let base = base.clone();
        let (game_id, asset_key) = (game_id.clone(), asset_key.clone());
        let part_ids: Vec<String> = doc
            .parts
            .iter()
            .filter(|p| p.mask_hash.is_some())
            .map(|p| p.id.clone())
            .collect();
        tauri::async_runtime::spawn_blocking(move || {
            let mut masks = Vec::new();
            for id in part_ids {
                let path = store::part_dir(&base, &game_id, &asset_key, &id).join("mask.png");
                let m = image::open(&path)
                    .map_err(|e| format!("open mask for {id}: {e}"))?
                    .to_luma8();
                masks.push((id, m));
            }
            rig::analyze(&masks)
        })
        .await
        .map_err(|e| format!("analysis task failed: {e}"))??
    };

    // Structure from AI when possible, heuristic otherwise (or on any AI failure).
    let tree = match crate::secrets::get(crate::secrets::OPENAI_KEY)?.filter(|k| !k.is_empty()) {
        Some(key) => {
            match rig::propose_tree_ai(&key, &analysis, doc.source.width, doc.source.height).await
            {
                Ok(t) => t,
                Err(err) => {
                    eprintln!("auto-rig: AI tree failed ({err}), using heuristic");
                    rig::heuristic_tree(&analysis)
                }
            }
        }
        None => rig::heuristic_tree(&analysis),
    };

    doc = rig::apply(doc, &analysis, &tree);

    // Deformable parts (hair/cloak/tail) auto-become meshes bound along the bone chain
    // apply() just laid down, so they wave with its sway physics — hands-off. A vertex near
    // the attachment blends toward the parent bone (seam bows); the rest follows the chain.
    let deformable_ids: Vec<String> =
        doc.parts.iter().filter(|p| p.deformable).map(|p| p.id.clone()).collect();
    for part_id in deformable_ids {
        let Some(slot_idx) = doc.slots.iter().position(|s| s.part_id == part_id) else {
            continue;
        };
        let Some(part) = doc.part(&part_id) else { continue };
        let Some(bbox) = part.effective_bbox() else { continue };
        let tex_path = store::part_texture_path(&base, &game_id, &asset_key, part);
        let own_bone = doc.slots[slot_idx].bone.clone();
        let mesh_data = tauri::async_runtime::spawn_blocking(move || {
            let tex = image::open(&tex_path)
                .map_err(|e| format!("open part texture: {e}"))?
                .to_rgba8();
            Ok::<_, String>(mesh::generate(&tex, bbox))
        })
        .await
        .map_err(|e| format!("mesh task failed: {e}"))??;
        if let Some(mut mesh_data) = mesh_data {
            mesh_data.weights = mesh::auto_weights(&doc, &own_bone, &mesh_data);
            doc.slots[slot_idx].attachment = Attachment::Mesh(mesh_data);
        }
    }

    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

// ── Inpainting (Phase 4) ──────────────────────────────────────────────────────

/// Per-part occlusion/inpaint state (clear / pending / fresh / stale).
#[tauri::command]
#[specta::specta]
pub async fn studio_inpaint_status(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<Vec<InpaintStatus>, String> {
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        Ok(inpaint::status(&base, &game_id, &asset_key, &doc))
    })
    .await
    .map_err(|e| format!("status task failed: {e}"))?
}

/// Inpaint one part's hidden band. `force` regenerates even on a cache hit.
#[tauri::command]
#[specta::specta]
pub async fn studio_inpaint_part(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    part_id: String,
    force: bool,
) -> Result<StudioDoc, String> {
    inpaint_parts(&app, &game_id, &asset_key, Some(part_id), force).await
}

/// Inpaint every stale/pending occluded part (bounded concurrency), emitting
/// `studio://inpaint-progress` events per part.
#[tauri::command]
#[specta::specta]
pub async fn studio_inpaint_all(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<StudioDoc, String> {
    inpaint_parts(&app, &game_id, &asset_key, None, false).await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InpaintProgress {
    part_id: String,
    state: String, // start | done | cached | error
    message: Option<String>,
}

const INPAINT_EVENT: &str = "studio://inpaint-progress";

async fn inpaint_parts(
    app: &tauri::AppHandle,
    game_id: &str,
    asset_key: &str,
    only: Option<String>,
    force: bool,
) -> Result<StudioDoc, String> {
    use futures::StreamExt;
    use tauri::Emitter;

    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app.clone())?;
    let mut doc = store::read_doc(&base, game_id, asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;

    // Plan jobs up front (blocking image work).
    let part_ids: Vec<String> = doc
        .parts
        .iter()
        .filter(|p| p.bbox.is_some() && p.mask_hash.is_some())
        .filter(|p| only.as_ref().is_none_or(|id| &p.id == id))
        .map(|p| p.id.clone())
        .collect();
    if part_ids.is_empty() {
        return Err("no cut parts to inpaint".into());
    }

    struct Work {
        job: inpaint::InpaintJob,
        cut_bbox: crate::studio::doc::Rect,
    }
    let mut work: Vec<Work> = Vec::new();
    {
        let base_c = base.clone();
        let (game_id_c, asset_key_c) = (game_id.to_string(), asset_key.to_string());
        let doc_c = doc.clone();
        let force_c = force;
        let planned: Vec<(String, Option<inpaint::InpaintJob>)> =
            tauri::async_runtime::spawn_blocking(move || {
                part_ids
                    .iter()
                    .map(|id| {
                        (
                            id.clone(),
                            inpaint::plan(&base_c, &game_id_c, &asset_key_c, &doc_c, id)
                                .ok()
                                .flatten(),
                        )
                    })
                    .collect()
            })
            .await
            .map_err(|e| format!("plan task failed: {e}"))?;

        for (id, job) in planned {
            let Some(job) = job else {
                // Unoccluded: make sure the part isn't stuck on a stale completed texture.
                if let Some(p) = doc.parts.iter_mut().find(|p| p.id == id) {
                    if only.is_some() {
                        p.texture = crate::studio::doc::PartTexture::Cut;
                    }
                }
                continue;
            };
            let part = doc.parts.iter().find(|p| p.id == id).unwrap();
            let cached = part.completed_hash.as_deref() == Some(job.hash.as_str())
                && store::part_dir(&base, game_id, asset_key, &id)
                    .join(format!("completed.{}.png", job.hash))
                    .is_file();
            if cached && !force_c {
                let _ = app.emit(
                    INPAINT_EVENT,
                    InpaintProgress { part_id: id, state: "cached".into(), message: None },
                );
                continue;
            }
            work.push(Work { cut_bbox: part.bbox.unwrap(), job });
        }
    }

    // Run jobs with bounded concurrency.
    let results: Vec<(String, Result<(String, crate::studio::doc::Rect), String>)> =
        futures::stream::iter(work.into_iter().map(|w| {
            let base = base.clone();
            let (game_id, asset_key) = (game_id.to_string(), asset_key.to_string());
            let api_key = api_key.clone();
            let app = app.clone();
            async move {
                let _ = app.emit(
                    INPAINT_EVENT,
                    InpaintProgress {
                        part_id: w.job.part_id.clone(),
                        state: "start".into(),
                        message: None,
                    },
                );
                let res =
                    inpaint::run_job(&base, &game_id, &asset_key, &api_key, &w.job, w.cut_bbox)
                        .await
                        .map(|bbox| (w.job.hash.clone(), bbox));
                let _ = app.emit(
                    INPAINT_EVENT,
                    InpaintProgress {
                        part_id: w.job.part_id.clone(),
                        state: if res.is_ok() { "done".into() } else { "error".into() },
                        message: res.as_ref().err().cloned(),
                    },
                );
                (w.job.part_id.clone(), res)
            }
        }))
        .buffer_unordered(2)
        .collect()
        .await;

    // Apply results.
    let mut errors: Vec<String> = Vec::new();
    for (part_id, res) in results {
        match res {
            Ok((hash, bbox)) => {
                if let Some(p) = doc.parts.iter_mut().find(|p| p.id == part_id) {
                    p.completed_hash = Some(hash);
                    p.completed_bbox = Some(bbox);
                    p.texture = crate::studio::doc::PartTexture::Completed;
                }
            }
            Err(e) => errors.push(format!("{part_id}: {e}")),
        }
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, game_id, asset_key, &doc)?;
    if !errors.is_empty() {
        return Err(format!("some parts failed — {}", errors.join("; ")));
    }
    Ok(doc)
}

// ── Mesh + bake (Phase 7) ─────────────────────────────────────────────────────

/// Toggle a part between a rigid region and a deformable auto-weighted mesh. Enabling
/// (re)generates the mesh from the part's CURRENT texture, so re-running after an inpaint
/// or recut refreshes it.
#[tauri::command]
#[specta::specta]
pub async fn studio_set_mesh(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    part_id: String,
    enabled: bool,
    // Target grid cells across the long side (density); 0 = default (~8).
    cells: u32,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let slot_idx = doc
        .slots
        .iter()
        .position(|s| s.part_id == part_id)
        .ok_or_else(|| format!("no slot for part \"{part_id}\""))?;

    if !enabled {
        doc.slots[slot_idx].attachment = Attachment::Region;
    } else {
        let part = doc
            .part(&part_id)
            .ok_or_else(|| format!("unknown part \"{part_id}\""))?;
        let bbox = part
            .effective_bbox()
            .ok_or_else(|| format!("part \"{part_id}\" has no texture yet"))?;
        let tex_path = store::part_texture_path(&base, &game_id, &asset_key, part);
        let own_bone = doc.slots[slot_idx].bone.clone();
        let detail = if cells == 0 { 8 } else { cells.clamp(3, 20) };
        let mesh_data = tauri::async_runtime::spawn_blocking(move || {
            let tex = image::open(&tex_path)
                .map_err(|e| format!("open part texture: {e}"))?
                .to_rgba8();
            mesh::generate_cells(&tex, bbox, detail).ok_or_else(|| "part texture is empty".to_string())
        })
        .await
        .map_err(|e| format!("mesh task failed: {e}"))??;
        let mut mesh_data = mesh_data;
        mesh_data.weights = mesh::auto_weights(&doc, &own_bone, &mesh_data);
        doc.slots[slot_idx].attachment = Attachment::Mesh(mesh_data);
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Bake a looping 2.5D depth-turn into a mesh-deform clip for a part. Estimates a relief map
/// (Depth-Anything), builds an UNWEIGHTED grid mesh, and projects it through a pinhole camera
/// across a yaw/pitch sweep — the per-frame offsets become a Spine `deform` timeline that plays
/// on the stock runtime with no custom code. Sets the part's slot to the (unweighted) mesh.
#[tauri::command]
#[specta::specta]
pub async fn studio_bake_turn(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    part_id: String,
    clip_name: String,
    params: crate::studio::turn::TurnParams,
    // Mesh density (cells across the long side); 0 = default (~10 for a smooth turn).
    density: u32,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let config = config_dir(&app)?;
    if !crate::layers::depth::weights_ready(&config) {
        return Err("depth model isn't downloaded yet — download it (Layers ▸ Depth) first".into());
    }
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let slot_idx = doc
        .slots
        .iter()
        .position(|s| s.part_id == part_id)
        .ok_or_else(|| format!("no slot for part \"{part_id}\""))?;
    let slot_name = doc.slots[slot_idx].name.clone();
    let part = doc
        .part(&part_id)
        .ok_or_else(|| format!("unknown part \"{part_id}\""))?;
    let bbox = part
        .effective_bbox()
        .ok_or_else(|| format!("part \"{part_id}\" has no texture yet"))?;
    let tex_path = store::part_texture_path(&base, &game_id, &asset_key, part);
    let detail = if density == 0 { 10 } else { density.clamp(3, 24) };
    let dur = params.duration.clamp(0.4, 12.0);

    let bake_slot = slot_name.clone();
    let bake_params = params.clone();
    let (mesh_data, timeline) = tauri::async_runtime::spawn_blocking(move || {
        let tex = image::open(&tex_path)
            .map_err(|e| format!("open part texture: {e}"))?
            .to_rgba8();
        let (tw, th) = tex.dimensions();
        // Flatten to RGB over neutral gray (the depth model's convention); alpha isn't consumed.
        let mut rgb = image::RgbImage::new(tw, th);
        for (x, y, p) in tex.enumerate_pixels() {
            let a = p.0[3] as u32;
            let blend = |c: u8| ((c as u32 * a + 128 * (255 - a)) / 255) as u8;
            rgb.put_pixel(x, y, image::Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
        }
        let raw = crate::layers::depth::estimate(&config, &rgb)?;
        // Cleanup: subject-normalize + edge-aware smooth + neutral background; auto-scale depth
        // by how much REAL relief the part has so flat art turns subtly, deep art fully.
        let alpha: Vec<u8> = tex.pixels().map(|p| p.0[3]).collect();
        let gain = crate::studio::turn::depth_gain(&raw, &alpha);
        let relief = crate::studio::turn::clean_relief(&raw, &alpha, &rgb, tw, th);
        let mut bake_params = bake_params;
        bake_params.depth *= gain;
        let mesh_data = mesh::generate_cells(&tex, bbox, detail)
            .ok_or_else(|| "part texture is empty".to_string())?;
        let timeline = crate::studio::turn::bake(
            &bake_slot, &mesh_data, bbox, &relief, &alpha, tw, th, &bake_params,
        );
        Ok::<_, String>((mesh_data, timeline))
    })
    .await
    .map_err(|e| format!("turn task failed: {e}"))??;

    // The deform mesh becomes the slot's setup attachment (UNWEIGHTED so deform is 1:1 with verts).
    doc.slots[slot_idx].attachment = Attachment::Mesh(mesh_data);

    // Replace this slot's deform timeline inside the target clip (create the clip if absent).
    let name = if clip_name.trim().is_empty() { "turn".to_string() } else { clip_name.trim().to_string() };
    if let Some(clip) = doc.clips.iter_mut().find(|c| c.name == name) {
        clip.timelines.retain(|tl| {
            !matches!(&tl.target, crate::studio::doc::TimelineTarget::SlotDeform(s) if *s == slot_name)
        });
        clip.timelines.push(timeline);
        clip.duration = clip.duration.max(dur);
        clip.looping = true;
    } else {
        let id: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
            .trim_matches('_')
            .to_string();
        let id = if id.is_empty() { "turn".to_string() } else { id };
        doc.clips.push(Clip { id, name, duration: dur, looping: true, timelines: vec![timeline] });
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Save baked clip frames (PNG data URLs from the real runtime) as a spritesheet +
/// metadata under `studio/export/sheets/` — the fallback for non-Spine consumers.
#[tauri::command]
#[specta::specta]
pub async fn studio_save_spritesheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    clip: String,
    fps: u32,
    frames: Vec<String>,
) -> Result<String, String> {
    if frames.is_empty() {
        return Err("no frames to bake".into());
    }
    let base = projects_root(&app)?;
    let dir = store::export_dir(&base, &game_id, &asset_key).join("sheets");
    let clip_slug: String = clip
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    tauri::async_runtime::spawn_blocking(move || {
        let mut decoded: Vec<image::RgbaImage> = Vec::with_capacity(frames.len());
        for (i, f) in frames.iter().enumerate() {
            let b64 = f
                .rsplit("base64,")
                .next()
                .ok_or_else(|| format!("frame {i}: bad data URL"))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("frame {i}: {e}"))?;
            decoded.push(
                image::load_from_memory(&bytes)
                    .map_err(|e| format!("frame {i}: {e}"))?
                    .to_rgba8(),
            );
        }
        let (fw, fh) = decoded[0].dimensions();
        if decoded.iter().any(|f| f.dimensions() != (fw, fh)) {
            return Err("frames have inconsistent dimensions".into());
        }
        let count = decoded.len() as u32;
        let cols = (count as f64).sqrt().ceil() as u32;
        let sheet_rows = count.div_ceil(cols);
        let mut sheet = image::RgbaImage::new(cols * fw, sheet_rows * fh);
        for (i, f) in decoded.iter().enumerate() {
            let (cx, cy) = (i as u32 % cols, i as u32 / cols);
            image::imageops::overlay(&mut sheet, f, (cx * fw) as i64, (cy * fh) as i64);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(sheet)
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| format!("encode sheet: {e}"))?;
        store::write_file(&dir.join(format!("{clip_slug}.png")), &png.into_inner())?;

        let meta = serde_json::json!({
            "clip": clip,
            "fps": fps,
            "frameWidth": fw,
            "frameHeight": fh,
            "frames": count,
            "columns": cols,
        });
        store::write_file(
            &dir.join(format!("{clip_slug}.json")),
            serde_json::to_string_pretty(&meta).unwrap().as_bytes(),
        )?;
        Ok(dir.join(format!("{clip_slug}.png")).to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| format!("bake task failed: {e}"))?
}

// ── FX layers (lights/glows) ──────────────────────────────────────────────────

/// Generate a light-effect layer with gpt-image-2: the effect is painted in place over
/// the symbol on PURE BLACK (black = invisible under additive blending), luminance
/// becomes alpha, and the result lands as an ordinary part on an ADDITIVE slot with its
/// own bone — animatable, physics-able, reorderable like any other part.
#[tauri::command]
#[specta::specta]
pub async fn studio_generate_fx(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    name: String,
    brief: String,
) -> Result<StudioDoc, String> {
    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    if brief.trim().is_empty() {
        return Err("describe the light effect first".into());
    }
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;

    // Unique slug across parts AND bones (the layer gets a bone of the same name).
    let slug_base = {
        let s = segment::slugify(&name);
        if s.is_empty() { "fx".to_string() } else { s }
    };
    let mut id = slug_base.clone();
    let mut i = 2;
    while doc.parts.iter().any(|p| p.id == id)
        || doc.bones.iter().any(|b| b.name == id)
        || id == "root"
    {
        id = format!("{slug_base}_{i}");
        i += 1;
    }

    // Reference = source flattened on black; the model paints only the light, in place.
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let (w, h) = source.dimensions();
    let flat = fx::flatten_on_black(&source);
    let mut ref_png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(flat)
        .write_to(&mut ref_png, image::ImageFormat::Png)
        .map_err(|e| format!("encode reference: {e}"))?;

    let size = crate::providers::openai_image::edit_size(w, h);
    let out_png = crate::providers::openai_image::edit_image(
        &api_key,
        &ref_png.into_inner(),
        &fx::fx_prompt(brief.trim()),
        &size,
        None,
    )
    .await?;

    // Back to source dims → luminance-to-alpha → trim → persist as a part texture.
    let generated = image::load_from_memory(&out_png)
        .map_err(|e| format!("decode fx: {e}"))?
        .to_rgba8();
    let resized = image::imageops::resize(&generated, w, h, image::imageops::FilterType::CatmullRom);
    let lit = fx::luminance_to_alpha(&resized);
    let bbox = fx::alpha_bbox(&lit)
        .ok_or_else(|| "the model returned no light — try a more concrete brief".to_string())?;
    let trimmed =
        image::imageops::crop_imm(&lit, bbox.x as u32, bbox.y as u32, bbox.w, bbox.h).to_image();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(trimmed)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode fx: {e}"))?;
    store::write_file(
        &store::part_dir(&base, &game_id, &asset_key, &id).join("cut.png"),
        &png.into_inner(),
    )?;

    // Part (front-most; reorder in the ledger) + additive slot + bone at its centre.
    doc.parts.push(crate::studio::doc::Part {
        id: id.clone(),
        name: if name.trim().is_empty() { id.clone() } else { name.trim().to_string() },
        prompts: vec![],
        bbox: Some(bbox),
        mask_hash: None, // no mask = FX layer; re-cuts pass it through untouched
        completed_hash: None,
        completed_bbox: None,
        texture: crate::studio::doc::PartTexture::Cut,
        deformable: false, // FX light layers are rigid quads
        attachment_only: false,
    });
    doc.bones.push(crate::studio::doc::Bone::new(
        id.clone(),
        Some("root".into()),
        bbox.x as f64 + bbox.w as f64 / 2.0,
        bbox.y as f64 + bbox.h as f64 / 2.0,
    ));
    doc.slots.push(crate::studio::doc::Slot {
        name: id.clone(),
        bone: id.clone(),
        part_id: id.clone(),
        attachment: Attachment::Region,
        blend: crate::studio::doc::BlendMode::Additive,
        alternates: Vec::new(),
    });

    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

// ── Attachment swap (alternate art on a slot) ───────────────────────────────────

/// Register `aligned` (source-scale, positioned over the host box) as an ATTACHMENT-ONLY part
/// on `host_part_id`'s slot: trim to content, persist the texture, push the part + the alternate
/// reference. The part gets no slot/bone of its own — it only ever shows as a swap.
fn commit_alternate(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
    doc: &mut StudioDoc,
    host_part_id: &str,
    name: &str,
    aligned: image::RgbaImage,
    host_bbox: Rect,
) -> Result<String, String> {
    let slug = {
        let s = segment::slugify(name);
        if s.is_empty() { "alt".to_string() } else { s }
    };
    let mut id = slug.clone();
    let mut i = 2;
    while doc.parts.iter().any(|p| p.id == id) || doc.bones.iter().any(|b| b.name == id) || id == "root" {
        id = format!("{slug}_{i}");
        i += 1;
    }
    let trim = fx::alpha_bbox(&aligned).ok_or_else(|| "the alternate image is fully transparent".to_string())?;
    let cropped =
        image::imageops::crop_imm(&aligned, trim.x as u32, trim.y as u32, trim.w, trim.h).to_image();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(cropped)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode alternate: {e}"))?;
    store::write_file(
        &store::part_dir(base, game_id, asset_key, &id).join("cut.png"),
        &png.into_inner(),
    )?;
    // Alternate box in source space = host origin + the trim within the host-sized image.
    let alt_bbox = Rect { x: host_bbox.x + trim.x, y: host_bbox.y + trim.y, w: trim.w, h: trim.h };
    doc.parts.push(crate::studio::doc::Part {
        id: id.clone(),
        name: if name.trim().is_empty() { id.clone() } else { name.trim().to_string() },
        prompts: vec![],
        bbox: Some(alt_bbox),
        mask_hash: None, // no mask = passes re-cut through untouched, like an FX layer
        completed_hash: None,
        completed_bbox: None,
        texture: crate::studio::doc::PartTexture::Cut,
        deformable: false,
        attachment_only: true,
    });
    let slot = doc
        .slots
        .iter_mut()
        .find(|s| s.part_id == host_part_id)
        .ok_or_else(|| format!("no slot for host part \"{host_part_id}\""))?;
    slot.alternates.push(id.clone());
    Ok(id)
}

/// Add an alternate attachment to a slot by importing a PNG (data URL). The image is scaled to
/// the host part's box so it overlays the base, then trimmed and registered as a swap option.
#[tauri::command]
#[specta::specta]
pub async fn studio_add_alternate(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    host_part_id: String,
    name: String,
    png_data_url: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let host = doc
        .part(&host_part_id)
        .ok_or_else(|| format!("unknown part \"{host_part_id}\""))?;
    let hb = host
        .effective_bbox()
        .ok_or_else(|| format!("part \"{host_part_id}\" has no texture yet"))?;

    let b64 = png_data_url
        .rsplit("base64,")
        .next()
        .ok_or_else(|| "bad image data URL".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("decode image: {e}"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("read image: {e}"))?
        .to_rgba8();
    let aligned = image::imageops::resize(&img, hb.w, hb.h, image::imageops::FilterType::CatmullRom);
    commit_alternate(&base, &game_id, &asset_key, &mut doc, &host_part_id, &name, aligned, hb)?;

    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

#[derive(serde::Deserialize)]
struct RegionBox {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default = "one_f64")]
    w: f64,
    #[serde(default = "one_f64")]
    h: f64,
}
fn one_f64() -> f64 {
    1.0
}

/// Ask the vision model for the smallest region of `crop_png` that must be regenerated to make
/// `change`, as a padded pixel `Rect`. `None` on any failure (caller falls back to whole crop).
async fn locate_region(api_key: &str, crop_png: &[u8], change: &str, cw: u32, ch: u32) -> Option<Rect> {
    const SYS: &str = "You are shown one image: a game-symbol part. Return ONLY a JSON object \
        {\"x\":,\"y\":,\"w\":,\"h\":} — the normalized (0..1) bounding box of the SMALLEST region \
        that must be regenerated to make the described change. For a local change (eyes, mouth, a \
        gem) box it tightly; for a global change (recolor/restyle the whole part) return \
        {\"x\":0,\"y\":0,\"w\":1,\"h\":1}.";
    let txt = segment::vision_json(api_key, SYS, &format!("Change to make: {change}"), crop_png)
        .await
        .ok()?;
    let b: RegionBox = serde_json::from_str(&txt).ok()?;
    let (pad_x, pad_y) = ((cw as f64 * 0.04) as i32, (ch as f64 * 0.04) as i32);
    let x = (b.x.clamp(0.0, 1.0) * cw as f64) as i32 - pad_x;
    let y = (b.y.clamp(0.0, 1.0) * ch as f64) as i32 - pad_y;
    let rx = x.max(0);
    let ry = y.max(0);
    let w = (b.w.clamp(0.02, 1.0) * cw as f64).round() as i32 + 2 * pad_x;
    let h = (b.h.clamp(0.02, 1.0) * ch as f64).round() as i32 + 2 * pad_y;
    let rw = (w.min(cw as i32 - rx)).max(1) as u32;
    let rh = (h.min(ch as i32 - ry)).max(1) as u32;
    Some(Rect { x: rx, y: ry, w: rw, h: rh })
}

/// Add an alternate attachment by GENERATING a variant of the host part with gpt-image-2. The
/// vision model locates the sub-region to change from the prompt, gpt-image regenerates ONLY
/// that (masked) region, and we composite the base pixel-identical everywhere else — so the
/// alternate (closed eyes, open mouth, lit gem) registers PERFECTLY with the base, no drift.
#[tauri::command]
#[specta::specta]
pub async fn studio_generate_alternate(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    host_part_id: String,
    name: String,
    prompt: String,
) -> Result<StudioDoc, String> {
    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    if prompt.trim().is_empty() {
        return Err("describe the alternate first".into());
    }
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    let host = doc
        .part(&host_part_id)
        .ok_or_else(|| format!("unknown part \"{host_part_id}\""))?;
    let hb = host
        .effective_bbox()
        .ok_or_else(|| format!("part \"{host_part_id}\" has no texture yet"))?;
    let host_tex_path = store::part_texture_path(&base, &game_id, &asset_key, host);

    // Vision uses the configured model (gpt-4o family); ensure it's a vision model.
    segment::set_vision_model(&crate::settings::load(&config_dir(&app)?).openai_vision_model);

    let host_tex = image::open(&host_tex_path)
        .map_err(|e| format!("open host texture: {e}"))?
        .to_rgba8();
    let (cw, ch) = host_tex.dimensions();
    let mut crop_cur = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(host_tex.clone())
        .write_to(&mut crop_cur, image::ImageFormat::Png)
        .map_err(|e| format!("encode reference: {e}"))?;
    let crop_png = crop_cur.into_inner();

    // Locate the sub-region to regenerate (smart: reads the prompt + sees the art); fall back to
    // the whole crop if the model can't localize.
    let region = locate_region(&api_key, &crop_png, prompt.trim(), cw, ch)
        .await
        .unwrap_or(Rect { x: 0, y: 0, w: cw, h: ch });
    let feather = (cw.min(ch) as f32 * 0.04).max(2.0);
    let region_mask = fx::feathered_region(cw, ch, region, feather);
    let mut mask_cur = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(fx::edit_mask_png(&region_mask))
        .write_to(&mut mask_cur, image::ImageFormat::Png)
        .map_err(|e| format!("encode mask: {e}"))?;

    let edit_prompt = format!(
        "{}. Keep the same subject, art style, framing, scale and transparent background; change \
         ONLY the transparent (unmasked) region and blend it seamlessly into the rest.",
        prompt.trim()
    );
    let size = crate::providers::openai_image::edit_size(cw, ch);
    let out_png = crate::providers::openai_image::edit_image(
        &api_key,
        &crop_png,
        &edit_prompt,
        &size,
        Some(&mask_cur.into_inner()),
    )
    .await?;
    let generated = image::load_from_memory(&out_png)
        .map_err(|e| format!("decode alternate: {e}"))?
        .to_rgba8();
    let generated = image::imageops::resize(&generated, cw, ch, image::imageops::FilterType::CatmullRom);
    // Register-perfect: base pixels everywhere outside the region, generated inside.
    let aligned = fx::composite_region(&host_tex, &generated, &region_mask);
    commit_alternate(&base, &game_id, &asset_key, &mut doc, &host_part_id, &name, aligned, hb)?;

    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Remove an alternate attachment from a slot (and drop the now-orphan part + its texture).
#[tauri::command]
#[specta::specta]
pub async fn studio_remove_alternate(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    host_part_id: String,
    alt_id: String,
) -> Result<StudioDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    if let Some(slot) = doc.slots.iter_mut().find(|s| s.part_id == host_part_id) {
        slot.alternates.retain(|a| a != &alt_id);
    }
    // Drop the attachment-only part (only if nothing else references it).
    let still_used = doc.slots.iter().any(|s| s.alternates.contains(&alt_id));
    if !still_used {
        doc.parts.retain(|p| !(p.id == alt_id && p.attachment_only));
        // Any swap timelines that pointed past the shrunken list are clamped by sanitize.
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

// ── AI motion (Phase 6) ───────────────────────────────────────────────────────

/// Draft a keyframe clip from a text brief over the current skeleton. Returns the clip
/// only — the frontend decides whether/where to apply it (cheap reroll, one-shot revert).
#[tauri::command]
#[specta::specta]
pub async fn studio_ai_draft_clip(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    name: String,
    brief: String,
) -> Result<Clip, String> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    if brief.trim().is_empty() {
        return Err("describe the motion first".into());
    }
    // Give the director EYES: the symbol snapshot, so it grounds motion in materials it sees.
    let image = std::fs::read(store::source_path(&base, &game_id, &asset_key)).ok();
    motion::draft_clip(&key, &doc, &name, &brief, image.as_deref()).await
}

/// Higher-quality draft on demand (the "Polish" button): best-of-N candidates ranked by a
/// deterministic score, then a vision critic picks the most tasteful/apt one.
#[tauri::command]
#[specta::specta]
pub async fn studio_ai_polish_clip(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    name: String,
    brief: String,
    candidates: u32,
) -> Result<Clip, String> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    if brief.trim().is_empty() {
        return Err("describe the motion first".into());
    }
    let image = std::fs::read(store::source_path(&base, &game_id, &asset_key)).ok();
    motion::draft_clip_polished(&key, &doc, &name, &brief, image.as_deref(), candidates).await
}

/// AI breakdown keys between two times of a clip. Takes the clip from the frontend
/// (unsaved edits included) and returns the updated clip.
#[tauri::command]
#[specta::specta]
pub async fn studio_ai_inbetween(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    clip: Clip,
    from_time: f64,
    to_time: f64,
    count: u32,
) -> Result<Clip, String> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no studio for this asset".to_string())?;
    motion::inbetween(&key, &doc, &clip, from_time, to_time, count.clamp(1, 5)).await
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// First-open setup: snapshot the source, seed the doc, write the degenerate cut.
fn create_studio(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
) -> Result<(StudioDoc, Vec<u8>), String> {
    let (source, png) = snapshot_source(base, game_id, asset_key)?;
    store::write_file(
        &store::part_dir(base, game_id, asset_key, "all").join("cut.png"),
        &png,
    )?;
    let doc = StudioDoc::seed(source, now_ms());
    store::write_doc(base, game_id, asset_key, &doc)?;
    Ok((doc, png))
}

/// Snapshot the active variation's best transparent image to `studio/source.png`.
fn snapshot_source(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
) -> Result<(SourceRef, Vec<u8>), String> {
    let (source, png_bytes) = snapshot_active_png(base, game_id, asset_key)?;
    store::write_file(&store::source_path(base, game_id, asset_key), &png_bytes)?;
    Ok((source, png_bytes))
}

/// Resolve the active variation's best image to PNG bytes + a [`SourceRef`], WITHOUT
/// writing anywhere — callers (studio, layers) persist to their own `source.png`.
/// Prefers the processed `png` stage, then decodes the `webp` stage, then falls back to
/// the raw generation.
pub(crate) fn snapshot_active_png(
    base: &std::path::Path,
    game_id: &str,
    asset_key: &str,
) -> Result<(SourceRef, Vec<u8>), String> {
    let record = storage::read_asset_record(base, game_id, asset_key)?
        .ok_or_else(|| "asset has no generated image yet".to_string())?;
    let active_id = record
        .active_variation
        .clone()
        .ok_or_else(|| "asset has no active variation".to_string())?;
    let variation = record
        .variations
        .iter()
        .find(|v| v.id == active_id)
        .ok_or_else(|| "active variation not found".to_string())?;

    let stage_rel = |name: &str| {
        variation
            .stages
            .iter()
            .find(|s| s.name == name)
            .map(|s| format!("variations/{}/{}", active_id, s.file))
    };

    let png_bytes = if let Some(rel) = stage_rel("png") {
        storage::read_asset_file(base, game_id, asset_key, &rel)?
    } else if let Some(rel) = stage_rel("webp") {
        let bytes = storage::read_asset_file(base, game_id, asset_key, &rel)?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("decode webp: {e}"))?;
        encode_png(&img)?
    } else {
        // Unprocessed: use the raw generation as-is (may still carry a background).
        storage::read_asset_file(base, game_id, asset_key, &variation.raw_file)?
    };

    let img = image::load_from_memory(&png_bytes).map_err(|e| format!("decode source: {e}"))?;
    // Normalize to PNG bytes (the stage may already be PNG; re-encode only if it wasn't).
    let png_bytes = if png_bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        png_bytes
    } else {
        encode_png(&img)?
    };

    Ok((
        SourceRef {
            variation_id: active_id,
            width: img.width(),
            height: img.height(),
            sha256: sha256_hex(&png_bytes),
        },
        png_bytes,
    ))
}

fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok(buf.into_inner())
}
