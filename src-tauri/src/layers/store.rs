//! On-disk layout for parallax layers, under `assets/<key>/layers/`:
//!
//! ```text
//! layers.json                 canonical LayersDoc (atomic writes)
//! source.png                  frozen snapshot of the processed variation
//! sam/embedding.st(+json)     cached SAM image-encoder output (independent of studio's)
//! parts/<id>/mask.png         8-bit gray author mask, full source dims (not for layer 0)
//! parts/<id>/cut.png          RGBA original pixels lifted by exclusive claim
//! parts/<id>/filled.<h>.png   cut + AI-filled hidden band (h = 8-hex cache key)
//! export/<key>.<id>.webp      full-source-dims layer, alpha WebP
//! export/parallax.json        { width, height, layers: [{id, file, speed}] }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use crate::storage;

use super::doc::LayersDoc;

pub fn layers_dir(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    storage::asset_dir(base, game_id, asset_key).join("layers")
}

pub fn layer_dir(base: &Path, game_id: &str, asset_key: &str, layer_id: &str) -> PathBuf {
    layers_dir(base, game_id, asset_key).join("parts").join(layer_id)
}

pub fn export_dir(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    layers_dir(base, game_id, asset_key).join("export")
}

pub fn source_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    layers_dir(base, game_id, asset_key).join("source.png")
}

pub fn doc_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    layers_dir(base, game_id, asset_key).join("layers.json")
}

/// Load the layers doc, or `None` if this asset has no layers yet.
pub fn read_doc(base: &Path, game_id: &str, asset_key: &str) -> Result<Option<LayersDoc>, String> {
    let path = doc_path(base, game_id, asset_key);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read layers doc failed: {e}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("parse layers doc failed: {e}"))
}

pub fn write_doc(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &LayersDoc,
) -> Result<(), String> {
    storage::atomic_write_json(&doc_path(base, game_id, asset_key), doc)
}

pub use crate::studio::store::write_file;
