//! Parallax-layers command surface. Thin wrappers over `crate::layers`.
//!
//! SAM weights status/download are shared with the studio (`studio_sam_status` /
//! `studio_sam_download`) — the model is app-global.

use base64::Engine;

use super::studio::{sam_segment_blocking, snapshot_active_png, MaskResult};
use super::{config_dir, now_ms, projects_root};
use crate::layers::depth::DepthStatus;
use crate::layers::doc::{Layer, LayersDoc};
use crate::layers::export::LayersExportReport;
use crate::layers::fill::FillStatus;
use crate::layers::propose::LayerProposal;
use crate::layers::{depth, export, fill, propose, split, store};
use crate::studio::doc::SamPrompt;
use crate::studio::{sam, sha256_hex};

/// Hole-fill cap for scenery selections: ~0.05% of the image, floor 64 px². Small SAM
/// speckle gets filled; real see-through openings (arches, canopy gaps) stay open.
fn scenery_hole_cap(w: u32, h: u32) -> u32 {
    ((w as u64 * h as u64) / 2000).max(64) as u32
}

/// Open (or create) the parallax layers for an asset. On first open the active processed
/// variation is snapshotted to `layers/source.png` and a 4-band doc is seeded
/// (sky catch-all / far / mid / foreground).
#[tauri::command]
#[specta::specta]
pub async fn layers_open(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<LayersDoc, String> {
    let base = projects_root(&app)?;
    if let Some(mut doc) = store::read_doc(&base, &game_id, &asset_key)? {
        // Follow the bench: if a NEWER take became active and no layer has a selection
        // yet, silently re-snapshot so the view never opens on stale art. Docs with
        // selections keep their pixels (Reimport is the explicit path).
        let pristine = doc.layers.iter().all(|l| l.mask_hash.is_none());
        if pristine {
            let active = crate::storage::read_asset_record(&base, &game_id, &asset_key)?
                .and_then(|r| r.active_variation);
            if active.is_some_and(|a| a != doc.source.variation_id) {
                let (source, png) = snapshot_active_png(&base, &game_id, &asset_key)?;
                store::write_file(&store::source_path(&base, &game_id, &asset_key), &png)?;
                doc.source = source;
                doc.updated_at = now_ms();
                store::write_doc(&base, &game_id, &asset_key, &doc)?;
            }
        }
        return Ok(doc);
    }
    let (source, png) = snapshot_active_png(&base, &game_id, &asset_key)?;
    store::write_file(&store::source_path(&base, &game_id, &asset_key), &png)?;
    let doc = LayersDoc::seed(source, now_ms());
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Persist the doc (rename / reorder / speeds / add / delete flow through here).
#[tauri::command]
#[specta::specta]
pub async fn layers_save(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    mut doc: LayersDoc,
) -> Result<LayersDoc, String> {
    let base = projects_root(&app)?;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Re-snapshot `source.png` from the current active processed variation (after a new
/// concept was approved). Masks/cuts/fills go stale via the source-hash change.
#[tauri::command]
#[specta::specta]
pub async fn layers_reimport_source(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<LayersDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset yet".to_string())?;
    let (source, png) = snapshot_active_png(&base, &game_id, &asset_key)?;
    store::write_file(&store::source_path(&base, &game_id, &asset_key), &png)?;
    doc.source = source;
    for layer in &mut doc.layers {
        layer.bbox = None;
        layer.filled_hash = None;
        layer.filled_bbox = None;
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Read a file under the asset's layers dir as a data URL (source, masks, cuts, fills…).
#[tauri::command]
#[specta::specta]
pub fn layers_get_image(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    rel: String,
) -> Result<String, String> {
    if rel.contains("..") {
        return Err("invalid path".into());
    }
    let base = projects_root(&app)?;
    let path = store::layers_dir(&base, &game_id, &asset_key).join(&rel);
    let bytes = std::fs::read(&path).map_err(|e| format!("read {rel} failed: {e}"))?;
    let mime = match rel.rsplit('.').next() {
        Some("webp") => "image/webp",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// Whether the depth-estimation model weights are on disk.
#[tauri::command]
#[specta::specta]
pub async fn layers_depth_status(app: tauri::AppHandle) -> Result<DepthStatus, String> {
    let config = config_dir(&app)?;
    Ok(if depth::weights_ready(&config) { DepthStatus::Ready } else { DepthStatus::Missing })
}

/// Download the depth-estimation weights (~190 MB total), emitting
/// `layers://depth-progress` events.
#[tauri::command]
#[specta::specta]
pub async fn layers_depth_download(app: tauri::AppHandle) -> Result<DepthStatus, String> {
    let config = config_dir(&app)?;
    depth::download(&app, &config).await?;
    Ok(DepthStatus::Ready)
}

/// Propose layers from ESTIMATED DEPTH: run Depth Anything V2 over the scene, slice the
/// depth map into `count` bands (far → near), and rebuild the doc with one layer per band
/// — selections come from actual depth, not object guessing. Replaces all existing layers
/// and their selections/cuts/fills.
#[tauri::command]
#[specta::specta]
pub async fn layers_propose_depth(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    count: u32,
) -> Result<LayersDoc, String> {
    let config = config_dir(&app)?;
    if !depth::weights_ready(&config) {
        return Err("depth model not downloaded yet".into());
    }
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgb8();
    let n = count.clamp(2, 8) as usize;
    let stamp = now_ms();

    tauri::async_runtime::spawn_blocking(move || {
        let depth_map = depth::estimate(&config, &source)?;
        let masks = depth::band_masks(&depth_map, &source, n)?;

        // Full reset: depth bands replace whatever layer structure existed.
        let _ = std::fs::remove_dir_all(store::layers_dir(&base, &game_id, &asset_key).join("parts"));
        doc.layers = depth::band_plan(n)
            .into_iter()
            .map(|(id, name, speed)| Layer::new(&id, &name, speed))
            .collect();
        // Band 0 (farthest) is the catch-all — no author mask; it claims the remainder.
        for (i, layer) in doc.layers.iter_mut().enumerate().skip(1) {
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(masks[i].clone())
                .write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| format!("encode mask for {}: {e}", layer.id))?;
            let png = buf.into_inner();
            store::write_file(
                &store::layer_dir(&base, &game_id, &asset_key, &layer.id).join("mask.png"),
                &png,
            )?;
            layer.mask_hash = Some(sha256_hex(&png));
        }
        doc.updated_at = stamp;
        store::write_doc(&base, &game_id, &asset_key, &doc)?;
        Ok(doc)
    })
    .await
    .map_err(|e| format!("depth propose task failed: {e}"))?
}

/// Ask gpt-4o vision for a depth-band plan (count layers, farthest first, seed points).
#[tauri::command]
#[specta::specta]
pub async fn layers_propose(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    count: u32,
    hint: String,
) -> Result<Vec<LayerProposal>, String> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    crate::studio::segment::set_vision_model(
        &crate::settings::load(&config_dir(&app)?).openai_vision_model,
    );
    let base = projects_root(&app)?;
    let png = std::fs::read(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("read source failed: {e}"))?;
    propose::propose_layers(&key, &png, count, &hint).await
}

/// Cloud "paint-out" cut for a depth layer: gpt-image-2 repaints everything except the
/// named band/element as flat magenta; keying yields the layer mask. The semantic
/// option for backgrounds where depth bands and click-prompted SAM struggle.
#[tauri::command]
#[specta::specta]
pub async fn layers_cloud_cut(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    layer_id: String,
) -> Result<MaskResult, String> {
    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    let layer = doc
        .layers
        .iter()
        .find(|l| l.id == layer_id)
        .ok_or_else(|| format!("layer \"{layer_id}\" not found"))?;
    let target = if layer.name.trim().is_empty() { layer.id.clone() } else { layer.name.clone() };
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    super::studio::cloud_paint_out(&api_key, source, &target).await
}

/// Run SAM with the given click prompts against this asset's scene and return the refined
/// candidate selection. Pure preview — nothing is persisted. Unlike the studio variant,
/// large enclosed openings (arches, canopy gaps) are NOT filled.
#[tauri::command]
#[specta::specta]
pub async fn layers_segment(
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
        .ok_or_else(|| "no layers for this asset".to_string())?;
    let source = image::open(store::source_path(&base, &game_id, &asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let sam_dir = store::layers_dir(&base, &game_id, &asset_key).join("sam");

    let sha = doc.source.sha256.clone();
    let cap = scenery_hole_cap(doc.source.width, doc.source.height);
    tauri::async_runtime::spawn_blocking(move || {
        // Union semantics: depth bands are usually several disjoint regions — every
        // positive point seeds its own SAM decode and the masks are unioned.
        sam_segment_blocking(&weights, model, &source, &sha, &sam_dir, &prompts, Some(cap), true)
    })
    .await
    .map_err(|e| format!("segment task failed: {e}"))?
}

/// Persist a layer's selection (accepted SAM result or manual brush work) and the prompts
/// that produced it. Must be a full-source-size gray PNG data URL. Not valid for the
/// catch-all layer (index 0) — its claim is derived.
#[tauri::command]
#[specta::specta]
pub async fn layers_set_mask(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    layer_id: String,
    mask_data_url: String,
    prompts: Vec<SamPrompt>,
) -> Result<LayersDoc, String> {
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    let idx = doc
        .layers
        .iter()
        .position(|l| l.id == layer_id)
        .ok_or_else(|| format!("layer \"{layer_id}\" not in doc — save the doc first"))?;
    if idx == 0 {
        return Err("the farthest layer is the catch-all — it needs no selection".into());
    }

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
        &store::layer_dir(&base, &game_id, &asset_key, &layer_id).join("mask.png"),
        &png,
    )?;

    let layer = &mut doc.layers[idx];
    layer.mask_hash = Some(sha256_hex(&png));
    layer.prompts = prompts;
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    Ok(doc)
}

/// Trace a layer's selection into simplified, editable polygon rings (outer boundaries
/// and holes; closed implicitly). `tolerance` is the simplification strength in source px.
#[tauri::command]
#[specta::specta]
pub async fn layers_trace_outline(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    layer_id: String,
    tolerance: f64,
) -> Result<Vec<Vec<[f64; 2]>>, String> {
    let base = projects_root(&app)?;
    let path = store::layer_dir(&base, &game_id, &asset_key, &layer_id).join("mask.png");
    let mask = image::open(&path)
        .map_err(|e| format!("no selection for {layer_id} yet: {e}"))?
        .to_luma8();
    let tol = tolerance.clamp(0.0, 32.0);
    tauri::async_runtime::spawn_blocking(move || {
        Ok(crate::studio::outline::trace_outlines(&mask, tol))
    })
    .await
    .map_err(|e| format!("trace task failed: {e}"))?
}

/// Cut all layers from the approved pixels (exclusive claim; layer 0 takes the rest).
#[tauri::command]
#[specta::specta]
pub async fn layers_cut(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<LayersDoc, String> {
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        split::cut_layers(&base, &game_id, &asset_key, doc)
    })
    .await
    .map_err(|e| format!("cut task failed: {e}"))?
}

/// Per-layer fill state (clear / pending / fresh / stale).
#[tauri::command]
#[specta::specta]
pub async fn layers_fill_status(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<Vec<FillStatus>, String> {
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    tauri::async_runtime::spawn_blocking(move || Ok(fill::status(&base, &game_id, &asset_key, &doc)))
        .await
        .map_err(|e| format!("status task failed: {e}"))?
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FillProgress {
    layer_id: String,
    state: String, // start | done | cached | error
    message: Option<String>,
}

const FILL_EVENT: &str = "layers://fill-progress";

/// Fill hidden bands. `only` limits to one layer; `force` regenerates even on a cache hit
/// (this is the per-layer "regenerate without touching the others" path). Emits
/// `layers://fill-progress` events per layer.
#[tauri::command]
#[specta::specta]
pub async fn layers_fill(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    only: Option<String>,
    force: bool,
) -> Result<LayersDoc, String> {
    use futures::StreamExt;
    use tauri::Emitter;

    let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
    let base = projects_root(&app)?;
    let mut doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;

    let layer_ids: Vec<String> = doc
        .layers
        .iter()
        .filter(|l| l.bbox.is_some())
        .filter(|l| only.as_ref().is_none_or(|id| &l.id == id))
        .map(|l| l.id.clone())
        .collect();
    if layer_ids.is_empty() {
        return Err("no cut layers to fill".into());
    }

    struct Work {
        job: crate::studio::inpaint::InpaintJob,
        cut_bbox: crate::studio::doc::Rect,
    }
    let mut work: Vec<Work> = Vec::new();
    {
        let base_c = base.clone();
        let (game_id_c, asset_key_c) = (game_id.clone(), asset_key.clone());
        let doc_c = doc.clone();
        let planned: Vec<(String, Option<crate::studio::inpaint::InpaintJob>)> =
            tauri::async_runtime::spawn_blocking(move || {
                layer_ids
                    .iter()
                    .map(|id| {
                        (
                            id.clone(),
                            fill::plan_layer(&base_c, &game_id_c, &asset_key_c, &doc_c, id)
                                .ok()
                                .flatten(),
                        )
                    })
                    .collect()
            })
            .await
            .map_err(|e| format!("plan task failed: {e}"))?;

        for (id, job) in planned {
            let Some(job) = job else { continue }; // nothing covers it
            let layer = doc.layers.iter().find(|l| l.id == id).unwrap();
            let cached = layer.filled_hash.as_deref() == Some(job.hash.as_str())
                && store::layer_dir(&base, &game_id, &asset_key, &id)
                    .join(format!("filled.{}.png", job.hash))
                    .is_file();
            if cached && !force {
                let _ = app.emit(
                    FILL_EVENT,
                    FillProgress { layer_id: id, state: "cached".into(), message: None },
                );
                continue;
            }
            work.push(Work { cut_bbox: layer.bbox.unwrap(), job });
        }
    }

    // Run jobs with bounded concurrency.
    let results: Vec<(String, Result<(String, crate::studio::doc::Rect), String>)> =
        futures::stream::iter(work.into_iter().map(|w| {
            let base = base.clone();
            let (game_id, asset_key) = (game_id.clone(), asset_key.clone());
            let api_key = api_key.clone();
            let app = app.clone();
            async move {
                let _ = app.emit(
                    FILL_EVENT,
                    FillProgress {
                        layer_id: w.job.part_id.clone(),
                        state: "start".into(),
                        message: None,
                    },
                );
                let res = fill::run_job(&base, &game_id, &asset_key, &api_key, &w.job, w.cut_bbox)
                    .await
                    .map(|bbox| (w.job.hash.clone(), bbox));
                let _ = app.emit(
                    FILL_EVENT,
                    FillProgress {
                        layer_id: w.job.part_id.clone(),
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

    let mut errors: Vec<String> = Vec::new();
    for (layer_id, res) in results {
        match res {
            Ok((hash, bbox)) => {
                if let Some(l) = doc.layers.iter_mut().find(|l| l.id == layer_id) {
                    l.filled_hash = Some(hash);
                    l.filled_bbox = Some(bbox);
                }
            }
            Err(e) => errors.push(format!("{layer_id}: {e}")),
        }
    }
    doc.updated_at = now_ms();
    store::write_doc(&base, &game_id, &asset_key, &doc)?;
    if !errors.is_empty() {
        return Err(format!("some layers failed — {}", errors.join("; ")));
    }
    Ok(doc)
}

/// Write the export set: one full-dims alpha WebP per layer + `parallax.json`.
#[tauri::command]
#[specta::specta]
pub async fn layers_export(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<LayersExportReport, String> {
    let base = projects_root(&app)?;
    let doc = store::read_doc(&base, &game_id, &asset_key)?
        .ok_or_else(|| "no layers for this asset".to_string())?;
    tauri::async_runtime::spawn_blocking(move || export::export(&base, &game_id, &asset_key, &doc))
        .await
        .map_err(|e| format!("export task failed: {e}"))?
}
