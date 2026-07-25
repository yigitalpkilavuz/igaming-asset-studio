//! Cutting the approved scene into depth layers via the shared exclusive-claim matte.
//! Layer 0 (the catch-all) gets a synthesized full-frame mask, so every source pixel
//! belongs to exactly one layer and the stack reassembles into the exact concept image.

use std::path::Path;

use image::GrayImage;

use crate::studio::matte;
use crate::studio::sha256_hex;

use super::doc::LayersDoc;
use super::store;

/// Cut all layers. Layers 1..N must have `mask.png` on disk (error names the ones that
/// don't); layer 0 claims everything the others leave. A layer whose claim ends up empty
/// (fully occluded) keeps its slot in the doc but loses bbox/cut. Any layer whose mask
/// geometry changed has its fill invalidated — including the layers BELOW a changed mask,
/// whose hidden bands shifted (the fill hash covers that, but we clear eagerly so the UI
/// flips to "stale" without a replan).
pub fn cut_layers(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    mut doc: LayersDoc,
) -> Result<LayersDoc, String> {
    if doc.layers.len() < 2 {
        return Err("need at least 2 layers to split".into());
    }
    let source = image::open(store::source_path(base, game_id, asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let (w, h) = source.dimensions();

    // Author masks for layers 1..N; synthesized full-frame mask for the catch-all.
    let mut ordered: Vec<(String, GrayImage)> = Vec::new();
    let mut mask_pngs: Vec<Option<Vec<u8>>> = Vec::new(); // parallel to doc.layers
    let mut missing: Vec<String> = Vec::new();
    for (i, layer) in doc.layers.iter().enumerate() {
        if i == 0 {
            ordered.push((layer.id.clone(), GrayImage::from_pixel(w, h, image::Luma([255]))));
            mask_pngs.push(None);
            continue;
        }
        let path = store::layer_dir(base, game_id, asset_key, &layer.id).join("mask.png");
        if !path.is_file() {
            missing.push(layer.id.clone());
            mask_pngs.push(None);
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read mask {}: {e}", layer.id))?;
        let m = image::load_from_memory(&bytes)
            .map_err(|e| format!("open mask for {}: {e}", layer.id))?
            .to_luma8();
        if m.dimensions() != (w, h) {
            return Err(format!(
                "mask for \"{}\" is {}×{}, expected source dims {w}×{h}",
                layer.id,
                m.width(),
                m.height()
            ));
        }
        ordered.push((layer.id.clone(), m));
        mask_pngs.push(Some(bytes));
    }
    if !missing.is_empty() {
        return Err(format!(
            "these layers have no selection yet: {} — select them (or delete them) first",
            missing.join(", ")
        ));
    }

    let pieces = matte::exclusive_cut(
        &source,
        &ordered,
        &matte::CutOptions {
            seam_fringe: doc.settings.seam_fringe,
            // The catch-all's full-frame mask leaves nothing unclaimed.
            rescue_unclaimed: false,
        },
    )?;
    let by_id: std::collections::HashMap<&str, &matte::CutPiece> =
        pieces.iter().map(|p| (p.id.as_str(), p)).collect();

    // The catch-all's geometry hash: the masks above define its claim, so hash those.
    let above_concat: Vec<u8> = mask_pngs
        .iter()
        .flatten()
        .flat_map(|b| b.iter().copied())
        .collect();

    let mut any_mask_changed = false;
    for (i, layer) in doc.layers.iter_mut().enumerate() {
        let new_hash = if i == 0 {
            Some(sha256_hex(&above_concat))
        } else {
            mask_pngs[i].as_deref().map(sha256_hex)
        };
        if layer.mask_hash != new_hash {
            any_mask_changed = true;
        }
        layer.mask_hash = new_hash;

        match by_id.get(layer.id.as_str()) {
            Some(piece) => {
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(piece.image.clone())
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|e| format!("encode cut for {}: {e}", layer.id))?;
                store::write_file(
                    &store::layer_dir(base, game_id, asset_key, &layer.id).join("cut.png"),
                    &png.into_inner(),
                )?;
                layer.bbox = Some(piece.bbox);
            }
            None => {
                // Fully occluded — keep the layer, clear its outputs so the UI shows it.
                layer.bbox = None;
            }
        }
    }
    // A re-cut with ANY changed mask moves hidden bands everywhere below the change;
    // clearing all fills keeps the UI honest (the hash check would flag them anyway).
    if any_mask_changed {
        for layer in &mut doc.layers {
            layer.filled_hash = None;
            layer.filled_bbox = None;
        }
    }

    store::write_doc(base, game_id, asset_key, &doc)?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{Rect, SourceRef};
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

    fn png_gray(img: &GrayImage) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn png_rgba(img: &RgbaImage) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// The phase-acceptance test: an opaque scene splits into 3 layers whose stack
    /// recomposes pixel-identical (≤3/255, feather tolerance) to the source.
    #[test]
    fn cut_layers_stack_reassembles_source_exactly() {
        let base = std::env::temp_dir().join(format!("wf_layers_split_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "bg_base_landscape");
        let (w, h) = (120u32, 80u32);

        // Opaque gradient "scene".
        let source = RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 2) as u8, (y * 3) as u8, ((x + y) % 255) as u8, 255])
        });
        store::write_file(&store::source_path(&base, gid, key), &png_rgba(&source)).unwrap();

        let mut doc = LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        doc.layers.truncate(3); // sky (catch-all) / far / mid
        // far: bottom band; mid: a hill overlapping it.
        let far = rect_mask(w, h, Rect { x: 0, y: 40, w, h: 40 });
        let mid = rect_mask(w, h, Rect { x: 30, y: 55, w: 60, h: 25 });
        store::write_file(
            &store::layer_dir(&base, gid, key, "far").join("mask.png"),
            &png_gray(&far),
        )
        .unwrap();
        store::write_file(
            &store::layer_dir(&base, gid, key, "mid").join("mask.png"),
            &png_gray(&mid),
        )
        .unwrap();

        let doc = cut_layers(&base, gid, key, doc).unwrap();
        assert!(doc.layers.iter().all(|l| l.bbox.is_some()));
        assert!(doc.layers[0].mask_hash.is_some(), "catch-all geometry hash set");

        // Recompose back → front.
        let mut canvas = RgbaImage::new(w, h);
        for layer in &doc.layers {
            let cut = image::open(store::layer_dir(&base, gid, key, &layer.id).join("cut.png"))
                .unwrap()
                .to_rgba8();
            let bb = layer.bbox.unwrap();
            for (x, y, p) in cut.enumerate_pixels() {
                if p.0[3] == 0 {
                    continue;
                }
                let (gx, gy) = ((bb.x as u32) + x, (bb.y as u32) + y);
                let under = *canvas.get_pixel(gx, gy);
                let ta = p.0[3] as f32 / 255.0;
                let blend = |t: u8, u: u8| (t as f32 * ta + u as f32 * (1.0 - ta)).round() as u8;
                canvas.put_pixel(
                    gx,
                    gy,
                    image::Rgba([
                        blend(p.0[0], under.0[0]),
                        blend(p.0[1], under.0[1]),
                        blend(p.0[2], under.0[2]),
                        255,
                    ]),
                );
            }
        }
        for (x, y, p) in source.enumerate_pixels() {
            let q = canvas.get_pixel(x, y);
            for c in 0..3 {
                assert!(
                    (p.0[c] as i32 - q.0[c] as i32).abs() <= 3,
                    "pixel ({x},{y}) ch{c}: {} vs {}",
                    p.0[c],
                    q.0[c]
                );
            }
        }

        // Editing an upper mask invalidates fills and changes the catch-all's hash.
        let sky_hash = doc.layers[0].mask_hash.clone();
        let mut doc2 = doc.clone();
        doc2.layers[2].filled_hash = Some("deadbeef".into());
        let mid2 = rect_mask(w, h, Rect { x: 20, y: 50, w: 70, h: 30 });
        store::write_file(
            &store::layer_dir(&base, gid, key, "mid").join("mask.png"),
            &png_gray(&mid2),
        )
        .unwrap();
        let doc2 = cut_layers(&base, gid, key, doc2).unwrap();
        assert_ne!(doc2.layers[0].mask_hash, sky_hash, "catch-all hash tracks upper masks");
        assert!(doc2.layers.iter().all(|l| l.filled_hash.is_none()), "fills invalidated");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_selection_is_named_in_the_error() {
        let base = std::env::temp_dir().join(format!("wf_layers_miss_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "bg");
        let (w, h) = (40u32, 30u32);
        let source = RgbaImage::from_pixel(w, h, image::Rgba([9, 9, 9, 255]));
        store::write_file(&store::source_path(&base, gid, key), &png_rgba(&source)).unwrap();
        let doc = LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        let err = cut_layers(&base, gid, key, doc).unwrap_err();
        assert!(err.contains("far") && err.contains("foreground"), "err names layers: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
