//! Persisted types for glyph-on-base composition.

use serde::{Deserialize, Serialize};
use specta::Type;

/// One reusable shape in the cross-game library. The shape itself lives next to this
/// meta as `shape.png` (white fill, alpha = the mark, cropped with a small margin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GlyphInfo {
    pub id: String,
    pub name: String,
    /// The prompt the shape was generated from (empty for imports).
    pub prompt: String,
    pub created_at: f64,
}

/// Where a glyph sits on the base.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    /// Glyph centre as a fraction of the base width (0.5 = centred).
    pub x: f64,
    /// Glyph centre as a fraction of the base height.
    pub y: f64,
    /// Glyph long side as a fraction of the base's shorter side.
    pub scale: f64,
    /// Degrees, clockwise.
    pub rotation: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.5, y: 0.5, scale: 0.42, rotation: 0.0 }
    }
}

/// Emboss look controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EmbossParams {
    /// Bevel width in px at base resolution.
    pub depth: f64,
    /// Strength of the raised highlight toward the lower-left light (0–2).
    pub highlight: f64,
    /// Strength of the soft shadow away from the light (0–2).
    pub shadow: f64,
    /// Glyph fill tone relative to the base material (1 = untouched, <1 darker).
    pub fill_tone: f64,
}

impl Default for EmbossParams {
    fn default() -> Self {
        Self { depth: 6.0, highlight: 0.9, shadow: 0.9, fill_tone: 0.94 }
    }
}

/// Per-asset "draw a mark on a base" settings, persisted at `assets/<key>/glyph.json`.
/// The base is just another asset in the game (e.g. `symbol_l_base` — an empty plate
/// generated once through the normal flow); every symbol pointing at the same base
/// gets a pixel-identical template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssetGlyph {
    /// Asset key whose current best image is the shared template.
    pub base_asset: String,
    /// What to draw on it — the glyph generation prompt.
    pub prompt: String,
    /// Library shape backing this mark, once generated.
    #[serde(default)]
    pub glyph_id: Option<String>,
    pub transform: Transform,
    pub params: EmbossParams,
}
