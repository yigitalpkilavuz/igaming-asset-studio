//! The canonical parallax-layers document. Layer order = depth order: index 0 is the
//! FARTHEST layer (the catch-all — it has no author mask and claims every pixel no other
//! layer wants), the last index is the nearest foreground.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::studio::doc::{Rect, SamPrompt, SourceRef};

pub const DOC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LayersDoc {
    pub version: u32,
    pub source: SourceRef,
    /// Back → front. Index 0 is the catch-all (no mask).
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub settings: LayerSettings,
    pub updated_at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    /// Slug id — also the on-disk dir and export file name.
    pub id: String,
    pub name: String,
    /// SAM seed prompts, normalized 0..1 (empty for the catch-all).
    #[serde(default)]
    pub prompts: Vec<SamPrompt>,
    /// sha256 of the author mask PNG; `None` for the catch-all layer (index 0), whose
    /// geometry hash is derived from the masks above it at cut time.
    #[serde(default)]
    pub mask_hash: Option<String>,
    /// Tight bbox of `cut.png` in source coords; `None` until cut.
    #[serde(default)]
    pub bbox: Option<Rect>,
    /// Fill cache key (8-hex) of `filled.<h>.png`; `None` = not filled.
    #[serde(default)]
    pub filled_hash: Option<String>,
    #[serde(default)]
    pub filled_bbox: Option<Rect>,
    /// Parallax multiplier: 0 = pinned to the camera (infinite distance), 1 = moves with
    /// the play field. Foregrounds may exceed 1 slightly.
    pub speed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct LayerSettings {
    /// Maximum camera offset the game will apply, in source px (drives fill band width).
    pub max_shift_px: u32,
    /// Duplicated seam band under nearer layers (px). Motion is guaranteed for parallax,
    /// so this defaults wider than the studio's 3.
    pub seam_fringe: u32,
    /// Explicit fill band radius override (px); 0 = derive per layer from speed deltas.
    pub band_px: u32,
}

impl Default for LayerSettings {
    fn default() -> Self {
        Self { max_shift_px: 0, seam_fringe: 6, band_px: 0 }
    }
}

impl Layer {
    pub fn new(id: &str, name: &str, speed: f64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            prompts: Vec::new(),
            mask_hash: None,
            bbox: None,
            filled_hash: None,
            filled_bbox: None,
            speed,
        }
    }
}

impl LayersDoc {
    /// Fresh doc with the standard 4-band split. `max_shift_px` seeds to ~5% of width.
    pub fn seed(source: SourceRef, now: f64) -> Self {
        let max_shift = (source.width / 20).max(16);
        Self {
            version: DOC_VERSION,
            source,
            layers: vec![
                Layer::new("sky", "Sky", 0.05),
                Layer::new("far", "Far background", 0.2),
                Layer::new("mid", "Mid-ground", 0.5),
                Layer::new("foreground", "Foreground", 1.0),
            ],
            settings: LayerSettings { max_shift_px: max_shift, ..Default::default() },
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_has_catch_all_bottom_and_rising_speeds() {
        let doc = LayersDoc::seed(
            SourceRef { variation_id: "v".into(), width: 1600, height: 900, sha256: "s".into() },
            0.0,
        );
        assert_eq!(doc.layers.len(), 4);
        assert!(doc.layers[0].mask_hash.is_none());
        assert!(doc.layers.windows(2).all(|w| w[0].speed < w[1].speed));
        assert_eq!(doc.settings.max_shift_px, 80);
        // Round-trips through JSON (IPC shape).
        let json = serde_json::to_string(&doc).unwrap();
        let back: LayersDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
    }
}
