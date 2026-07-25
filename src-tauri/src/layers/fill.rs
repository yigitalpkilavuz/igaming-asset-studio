//! Filling what nearer layers hide. Reuses the studio inpaint core (`plan_fill` /
//! `execute_fill`): gpt-image-2 masked edit over magenta, anti-drift composite (original
//! pixels always win). Two layer-specific twists:
//!
//! - The catch-all layer's zone is computed from its CLAIMED pixels (full frame minus the
//!   masks above), not a full-frame mask — otherwise the zone would degenerate into the
//!   entire silhouette of every nearer layer.
//! - The band radius derives from parallax speed deltas: layer i can be revealed by up to
//!   `max_j>i(speed_j − speed_i) × max_shift_px` pixels of relative motion. The sky under
//!   the foreground needs the WIDEST band.

use std::path::Path;

use image::GrayImage;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::studio::inpaint::{self, FillSpec, InpaintJob, InpaintState};
use crate::studio::doc::Rect;

use super::doc::LayersDoc;
use super::store;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FillStatus {
    pub layer_id: String,
    pub state: InpaintState,
}

/// Fill band radius for layer `idx`: explicit override, or the worst-case reveal from
/// relative parallax motion plus a small margin, clamped [16, 256] px.
pub fn band_radius(doc: &LayersDoc, idx: usize) -> u32 {
    if doc.settings.band_px > 0 {
        return doc.settings.band_px;
    }
    let max_shift = if doc.settings.max_shift_px > 0 {
        doc.settings.max_shift_px
    } else {
        (doc.source.width / 20).max(16)
    };
    let own_speed = doc.layers[idx].speed;
    let worst_delta = doc.layers[idx + 1..]
        .iter()
        .map(|l| (l.speed - own_speed).max(0.0))
        .fold(0.0f64, f64::max);
    let reveal = worst_delta * max_shift as f64;
    (reveal.ceil() as u32 + 8).clamp(16, 256)
}

fn layer_fill_prompt(layer_name: &str) -> String {
    format!(
        "This image shows only the {layer_name} depth layer of a 2D game background scene, \
isolated on flat magenta (#FF00FF) — the nearer scenery that covered it was removed. Fill \
ONLY the transparent regions of the mask: continue the {layer_name}'s terrain, sky, \
foliage, architecture and atmosphere with the exact same lighting direction, palette, \
perspective and painting style, completing what the nearer layers hid. Do not alter any \
visible pixel. Do not add characters, objects or text. Everything that is not the \
{layer_name} stays flat magenta, with crisp edges and no halos."
    )
}

/// Load a layer's author mask from disk.
fn load_mask(base: &Path, gid: &str, key: &str, layer_id: &str) -> Result<GrayImage, String> {
    image::open(store::layer_dir(base, gid, key, layer_id).join("mask.png"))
        .map(|i| i.to_luma8())
        .map_err(|e| format!("open mask for {layer_id}: {e}"))
}

/// Build the fill job for one layer, or `None` if nothing covers it (top-most layer, or
/// no overlap within the band). The catch-all's own geometry = full frame minus the masks
/// above it.
pub fn plan_layer(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &LayersDoc,
    layer_id: &str,
) -> Result<Option<InpaintJob>, String> {
    let idx = doc
        .layers
        .iter()
        .position(|l| l.id == layer_id)
        .ok_or_else(|| format!("unknown layer \"{layer_id}\""))?;
    let layer = &doc.layers[idx];
    let bbox = layer
        .bbox
        .ok_or_else(|| format!("layer \"{layer_id}\" is not cut yet"))?;
    if idx + 1 == doc.layers.len() {
        return Ok(None); // nearest layer — nothing covers it
    }

    // Masks above + their bytes (cache key input).
    let mut above_imgs: Vec<GrayImage> = Vec::new();
    let mut hash_extra: Vec<u8> = vec![if idx == 0 { b'C' } else { b'M' }];
    if idx > 0 {
        hash_extra.extend_from_slice(
            &std::fs::read(store::layer_dir(base, game_id, asset_key, layer_id).join("mask.png"))
                .map_err(|e| format!("read own mask: {e}"))?,
        );
    }
    for l in doc.layers.iter().skip(idx + 1) {
        let path = store::layer_dir(base, game_id, asset_key, &l.id).join("mask.png");
        if path.is_file() {
            hash_extra.extend_from_slice(
                &std::fs::read(&path).map_err(|e| format!("read mask for {}: {e}", l.id))?,
            );
            above_imgs.push(load_mask(base, game_id, asset_key, &l.id)?);
        }
    }
    if above_imgs.is_empty() {
        return Ok(None);
    }

    // Own geometry: author mask, or (catch-all) everything the layers above don't claim.
    let own: GrayImage = if idx == 0 {
        let (w, h) = above_imgs[0].dimensions();
        GrayImage::from_fn(w, h, |x, y| {
            let covered = above_imgs.iter().any(|m| m.get_pixel(x, y).0[0] > 127);
            image::Luma([if covered { 0 } else { 255 }])
        })
    } else {
        load_mask(base, game_id, asset_key, layer_id)?
    };

    let radius = band_radius(doc, idx);
    let above_refs: Vec<&GrayImage> = above_imgs.iter().collect();
    // Cheap pre-check so we don't decode cut.png for layers with nothing to fill.
    if inpaint::compute_zone(&own, &above_refs, radius).is_none() {
        return Ok(None);
    }
    let cut = image::open(store::layer_dir(base, game_id, asset_key, layer_id).join("cut.png"))
        .map_err(|e| format!("open cut for {layer_id}: {e}"))?
        .to_rgba8();

    inpaint::plan_fill(
        layer_id,
        &FillSpec {
            own_mask: &own,
            above_masks: &above_refs,
            cut: &cut,
            cut_bbox: bbox,
            radius,
            prompt: layer_fill_prompt(&layer.name),
            hash_extra: &hash_extra,
        },
    )
}

/// Run one layer's fill job through gpt-image-2 and write `filled.<hash8>.png`.
/// Returns the filled texture's source-coord bbox.
pub async fn run_job(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    api_key: &str,
    job: &InpaintJob,
    cut_bbox: Rect,
) -> Result<Rect, String> {
    let cut = image::open(
        store::layer_dir(base, game_id, asset_key, &job.part_id).join("cut.png"),
    )
    .map_err(|e| format!("open cut: {e}"))?
    .to_rgba8();
    let (filled, bbox) = inpaint::execute_fill(api_key, job, &cut, cut_bbox).await?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(filled)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode filled: {e}"))?;
    store::write_file(
        &store::layer_dir(base, game_id, asset_key, &job.part_id)
            .join(format!("filled.{}.png", job.hash)),
        &png.into_inner(),
    )?;
    Ok(bbox)
}

/// Per-layer fill state (recomputes the expected hash from the masks on disk).
pub fn status(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &LayersDoc,
) -> Vec<FillStatus> {
    doc.layers
        .iter()
        .filter(|l| l.bbox.is_some())
        .map(|l| {
            let state = match plan_layer(base, game_id, asset_key, doc, &l.id) {
                Ok(None) => InpaintState::Clear,
                Ok(Some(job)) => match &l.filled_hash {
                    Some(h) if *h == job.hash => InpaintState::Fresh,
                    Some(_) => InpaintState::Stale,
                    None => InpaintState::Pending,
                },
                Err(_) => InpaintState::Pending,
            };
            FillStatus { layer_id: l.id.clone(), state }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::SourceRef;
    use image::RgbaImage;

    fn rect_mask(w: u32, h: u32, r: Rect) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let inside = (x as i32) >= r.x
                && (y as i32) >= r.y
                && (x as i32) < r.x + r.w as i32
                && (y as i32) < r.y + r.h as i32;
            image::Luma([if inside { 255 } else { 0 }])
        })
    }

    #[test]
    fn band_radius_tracks_speed_deltas_and_sky_is_widest() {
        let mut doc = LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: 1600, height: 900, sha256: "s".into() },
            0.0,
        );
        doc.settings.max_shift_px = 100;
        // sky 0.05 / far 0.2 / mid 0.5 / fg 1.0
        let radii: Vec<u32> = (0..4).map(|i| band_radius(&doc, i)).collect();
        assert_eq!(radii[0], (0.95f64 * 100.0).ceil() as u32 + 8, "sky vs foreground");
        assert!(radii[0] > radii[1] && radii[1] > radii[2], "deeper → wider band");
        assert_eq!(radii[3], 16, "nearest layer clamps to the floor");
        doc.settings.band_px = 42;
        assert_eq!(band_radius(&doc, 0), 42, "explicit override wins");
    }

    #[test]
    fn bottom_zone_is_band_not_silhouette() {
        let base = std::env::temp_dir().join(format!("wf_layers_fill_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "bg");
        let (w, h) = (200u32, 150u32);

        let source = RgbaImage::from_pixel(w, h, image::Rgba([70, 80, 90, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        store::write_file(&store::source_path(&base, gid, key), &buf.into_inner()).unwrap();

        let mut doc = LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        doc.layers.truncate(2); // sky + far
        doc.settings.max_shift_px = 40; // reveal ≈ (0.2-0.05)*40 = 6 → clamps to 16
        // Big covering rect: its 16px-deep border ring is well under half its area.
        let far = rect_mask(w, h, Rect { x: 20, y: 40, w: 160, h: 100 });
        let mut mbuf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(far.clone())
            .write_to(&mut mbuf, image::ImageFormat::Png)
            .unwrap();
        store::write_file(
            &store::layer_dir(&base, gid, key, "far").join("mask.png"),
            &mbuf.into_inner(),
        )
        .unwrap();

        let doc = super::super::split::cut_layers(&base, gid, key, doc).unwrap();
        let job = plan_layer(&base, gid, key, &doc, "sky").unwrap().expect("sky is covered");

        // The zone must be a band around the far layer's silhouette, not the whole thing.
        let zone_px: u32 = job.zone.pixels().map(|p| (p.0[0] > 127) as u32).sum();
        let far_px: u32 = far.pixels().map(|p| (p.0[0] > 127) as u32).sum();
        assert!(zone_px > 0, "zone exists");
        assert!(
            zone_px < far_px / 2,
            "zone ({zone_px}px) must be a fraction of the covering silhouette ({far_px}px)"
        );

        // Nearest layer has no job; statuses agree.
        assert!(plan_layer(&base, gid, key, &doc, "far").unwrap().is_none());
        let st = status(&base, gid, key, &doc);
        assert_eq!(
            st.iter().find(|s| s.layer_id == "sky").unwrap().state,
            InpaintState::Pending
        );
        assert_eq!(
            st.iter().find(|s| s.layer_id == "far").unwrap().state,
            InpaintState::Clear
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
