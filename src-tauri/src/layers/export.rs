//! Layer export: each layer re-inflated to full source dimensions (so the stack aligns by
//! construction), encoded as alpha WebP, plus `parallax.json` describing order and speeds.
//! `export::build_dist` ships this set instead of the flat raster when it exists.

use std::path::Path;

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::doc::LayersDoc;
use super::store;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LayersExportReport {
    /// Written files (relative to the export dir), back → front.
    pub files: Vec<String>,
    /// Layers exported from a stale or missing fill (raw cut used instead).
    pub stale: Vec<String>,
    pub width: u32,
    pub height: u32,
}

/// The manifest written next to the layer images.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallaxManifest {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<ParallaxLayerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallaxLayerEntry {
    pub id: String,
    pub file: String,
    pub speed: f64,
}

/// Write `export/<key>.<id>.webp` per layer (back → front) + `export/parallax.json`.
pub fn export(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &LayersDoc,
) -> Result<LayersExportReport, String> {
    let (w, h) = (doc.source.width, doc.source.height);
    let dir = store::export_dir(base, game_id, asset_key);
    // Rebuild from scratch so renamed/deleted layers don't leave orphans.
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("clear export dir: {e}"))?;
    }

    let mut files = Vec::new();
    let mut stale = Vec::new();
    let mut entries = Vec::new();
    for layer in &doc.layers {
        let Some(bbox) = layer.bbox else {
            continue; // fully occluded / never cut — nothing to ship
        };
        // Prefer the fresh fill; fall back to the raw cut (and report it).
        let (tex_path, tex_bbox) = match (&layer.filled_hash, layer.filled_bbox) {
            (Some(hash), Some(fb)) => {
                let p = store::layer_dir(base, game_id, asset_key, &layer.id)
                    .join(format!("filled.{hash}.png"));
                if p.is_file() {
                    (p, fb)
                } else {
                    stale.push(layer.id.clone());
                    (store::layer_dir(base, game_id, asset_key, &layer.id).join("cut.png"), bbox)
                }
            }
            _ => {
                stale.push(layer.id.clone());
                (store::layer_dir(base, game_id, asset_key, &layer.id).join("cut.png"), bbox)
            }
        };
        let tex = image::open(&tex_path)
            .map_err(|e| format!("open texture for {}: {e}", layer.id))?
            .to_rgba8();

        // Paste at bbox into a full-dims canvas — alignment by construction.
        let mut canvas = RgbaImage::new(w, h);
        for (x, y, p) in tex.enumerate_pixels() {
            if p.0[3] == 0 {
                continue;
            }
            let gx = tex_bbox.x + x as i32;
            let gy = tex_bbox.y + y as i32;
            if gx >= 0 && gy >= 0 && (gx as u32) < w && (gy as u32) < h {
                canvas.put_pixel(gx as u32, gy as u32, *p);
            }
        }

        let webp = webp::Encoder::from_rgba(canvas.as_raw(), w, h).encode(85.0).to_vec();
        let file = format!("{asset_key}.{}.webp", layer.id);
        store::write_file(&dir.join(&file), &webp)?;
        files.push(file.clone());
        entries.push(ParallaxLayerEntry { id: layer.id.clone(), file, speed: layer.speed });
    }
    if entries.len() < 2 {
        return Err("fewer than 2 layers have pixels — cut the layers first".into());
    }

    let manifest = ParallaxManifest { width: w, height: h, layers: entries };
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("encode manifest: {e}"))?;
    store::write_file(&dir.join("parallax.json"), &json)?;

    Ok(LayersExportReport { files, stale, width: w, height: h })
}

/// Read the export manifest, or `None` when this asset has no layer export.
pub fn read_manifest(
    base: &Path,
    game_id: &str,
    asset_key: &str,
) -> Result<Option<ParallaxManifest>, String> {
    let path = store::export_dir(base, game_id, asset_key).join("parallax.json");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read parallax manifest: {e}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("parse parallax manifest: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::SourceRef;
    use image::GrayImage;

    #[test]
    fn export_writes_aligned_webps_and_manifest() {
        let base = std::env::temp_dir().join(format!("wf_layers_export_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "bg_base_landscape");
        let (w, h) = (100u32, 60u32);

        let source = RgbaImage::from_fn(w, h, |x, _| image::Rgba([(x * 2) as u8, 50, 90, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        store::write_file(&store::source_path(&base, gid, key), &buf.into_inner()).unwrap();

        let mut doc = crate::layers::doc::LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        doc.layers.truncate(2);
        let mask = GrayImage::from_fn(w, h, |_, y| image::Luma([if y >= 30 { 255 } else { 0 }]));
        let mut mbuf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(mask)
            .write_to(&mut mbuf, image::ImageFormat::Png)
            .unwrap();
        store::write_file(
            &store::layer_dir(&base, gid, key, "far").join("mask.png"),
            &mbuf.into_inner(),
        )
        .unwrap();
        let doc = crate::layers::split::cut_layers(&base, gid, key, doc).unwrap();

        let report = export(&base, gid, key, &doc).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!((report.width, report.height), (w, h));
        assert_eq!(report.stale.len(), 2, "no fills yet — both exported from raw cuts");

        // Every exported layer decodes to exactly source dims (alignment contract).
        for f in &report.files {
            let img = image::open(store::export_dir(&base, gid, key).join(f)).unwrap();
            assert_eq!((img.width(), img.height()), (w, h), "{f}");
        }
        let manifest = read_manifest(&base, gid, key).unwrap().unwrap();
        assert_eq!(manifest.layers.len(), 2);
        assert!(manifest.layers[0].speed < manifest.layers[1].speed);

        let _ = std::fs::remove_dir_all(&base);
    }
}
