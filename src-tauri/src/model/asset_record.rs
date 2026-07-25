use serde::{Deserialize, Serialize};
use specta::Type;

/// The editable prompt state for one asset. Persisted in `assets/<key>/asset.json`.
///
/// Prompts are assembled deterministically from the game's style master + a per-category
/// composition template + this per-asset `subject`. No LLM is involved. Power users can
/// bypass assembly with a full `override`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub struct PromptState {
    /// The variable part: what this asset actually is (a short phrase).
    #[serde(default)]
    pub subject: String,
    /// Optional full-prompt override — if non-empty, used verbatim instead of assembly.
    #[serde(default)]
    pub override_prompt: String,
    /// Optional per-asset negative override.
    #[serde(default)]
    pub negative_override: String,
    /// Backgrounds only: the scene will be cut into parallax depth layers, so the prompt
    /// asks for cleanly separable planes (crisp silhouettes, no cross-plane haze/blur).
    #[serde(default)]
    pub layered: bool,
    /// Unix epoch millis of the last change.
    #[serde(default)]
    pub updated_at: f64,
}

/// A single generated image variation and its full lineage (M3 fills this in).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Variation {
    pub id: String,
    /// The variation this one branched from (retry / vary), if any.
    #[serde(default)]
    pub parent: Option<String>,
    pub created_at: f64,
    /// Exact prompt text used for this generation.
    pub prompt_snapshot: String,
    /// Provider id, e.g. `drawthings` / `openai_image`.
    pub provider: String,
    /// Model within the provider, if known.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub seed: Option<f64>,
    /// Relative path (from the asset dir) of the raw generated image.
    pub raw_file: String,
    pub status: VariationStatus,
    /// How the background was generated (drives post-processing bg removal).
    #[serde(default)]
    pub background: crate::providers::BgMode,
    /// Post-processing stage outputs (paths relative to the variation dir).
    #[serde(default)]
    pub stages: Vec<crate::processing::StageOutput>,
    /// Visual-mass sizing outcome from the last Process run (symbols only).
    #[serde(default)]
    pub mass_report: Option<MassReport>,
    /// Locked/favourited — protected from history pruning (`delete_variation` refuses it).
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum VariationStatus {
    Generating,
    Ready,
    Failed,
}

/// Outcome of the symbol FIT pass. `clamped_by` names which limit fired; `flag`
/// reports UNDER/OVERWEIGHT vs the class tolerance — an art problem to surface, not
/// silently fudge (the object needs a base, mount or bracket at generation time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MassReport {
    /// Ink fraction of the cell actually reached after the fit.
    pub achieved: f64,
    /// The class's configured ink target.
    pub target: f64,
    /// True when a clamp fired (box or placement).
    pub clamped: bool,
    /// "none" | "box" | "placement".
    #[serde(default)]
    pub clamped_by: String,
    /// Ink fraction before the fit.
    #[serde(default)]
    pub ink_before: f64,
    /// The two candidate scales and the blended, clamped result.
    #[serde(default)]
    pub scale_height: f64,
    #[serde(default)]
    pub scale_area: f64,
    #[serde(default)]
    pub scale_final: f64,
    /// "ok" | "underweight" | "overweight" vs the class tolerance.
    #[serde(default)]
    pub flag: String,
    /// scale_final > 1.15 — the source lacks resolution, regenerate larger.
    #[serde(default)]
    pub upscaled: bool,
}

/// Per-asset working record: the prompt plus every generated variation. Canonical copy
/// lives at `<project>/assets/<asset_key>/asset.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssetRecord {
    pub key: String,
    pub prompt: PromptState,
    #[serde(default)]
    pub variations: Vec<Variation>,
    #[serde(default)]
    pub active_variation: Option<String>,
    /// Template only (e.g. an `l_base` empty plate other symbols compose onto) —
    /// excluded from Export and Publish; it never ships.
    #[serde(default)]
    pub template_only: bool,
}

impl AssetRecord {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            prompt: PromptState::default(),
            variations: Vec::new(),
            active_variation: None,
            template_only: false,
        }
    }
}
