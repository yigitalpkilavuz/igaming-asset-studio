use serde::{Deserialize, Serialize};
use specta::Type;

/// The pay system, which gates several asset categories (paylines vs way-indicator
/// vs cluster-outline) per ASSETS.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WinType {
    Lines,
    Ways,
    Scatter,
    Cluster,
}

/// A symbol's tier/role. Drives cardinality (one asset per symbol) and the oversize
/// exception for wild/scatter (ASSETS.md §2 Step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SymbolRole {
    High,
    Low,
    Wild,
    Scatter,
    Bonus,
    /// Mechanic-specific special symbol (e.g. an ink drop / multiplier orb). No-pay,
    /// standard cell size, but still a generatable raster.
    Special,
    /// A wild that EXPANDS to cover its reel: derives TWO assets — the normal
    /// cell-sized `symbol_<key>` plus a full-column `symbol_<key>_expanded` twin
    /// (fit pass skipped, full-height composition).
    ExpandingWild,
}

/// One symbol declared in the game config. The taxonomy expands one asset per entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SymbolDef {
    /// Stable slug used to build the asset key, e.g. `h1`, `l3`, `wild`, `ink_drop`.
    pub key: String,
    pub role: SymbolRole,
    /// Human display name, e.g. "Snail-Knight". Feeds prompt generation (M2).
    pub name: String,
    /// Art-direction notes for this symbol. Feeds prompt generation (M2).
    #[serde(default)]
    pub description: String,
    /// Intended motion, one short sentence — seeds AI spritesheet / motion drafting.
    /// Spritesheet animations cap at 24 frames (~2 s loop), so keep it loopable.
    #[serde(default)]
    pub animation: String,
    /// Manual scale multiplier layered ON TOP of the visual-mass rule (1 = neutral).
    /// The hard extent clamp still applies.
    #[serde(default = "default_nudge")]
    pub size_nudge: f64,
}

fn default_nudge() -> f64 {
    1.0
}

/// Symbol fit rule: the eye reads INK (covered pixels), not height — a hat and a
/// hand at equal height differ ~3× in ink and read as different pay classes. Each
/// class blends a height-based scale with an ink-based scale (geometric mean,
/// `area_weight` = the taste dial), anchors centroid-biased, and hard-clamps to the
/// safe box so nothing overflows its cell.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FitClass {
    /// Target ink as a fraction of cell area.
    pub ink: f64,
    /// Target bbox height as a fraction of cell height (the blend's other pole).
    pub height: f64,
    /// Acceptable |ink_after − ink| before the report flags UNDER/OVERWEIGHT.
    pub tolerance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSizing {
    #[serde(default = "d_low")]
    pub low: FitClass,
    #[serde(default = "d_high")]
    pub high: FitClass,
    #[serde(default = "d_wild")]
    pub wild: FitClass,
    #[serde(default = "d_scatter")]
    pub scatter: FitClass,
    /// 0 = pure height fit (the old broken behaviour), 1 = pure ink fit (comically
    /// large slim objects). Geometric blend between the two scales.
    #[serde(default = "d_area_weight")]
    pub area_weight: f64,
    /// Anchor = bias·centroid + (1−bias)·bbox centre. Pure bbox sits base-heavy
    /// objects low; pure centroid pushes wide-topped objects out of the safe box.
    #[serde(default = "d_centroid_bias")]
    pub centroid_bias: f64,
    /// Alpha below this (0–255) is chroma fringe / glow haze — not ink.
    #[serde(default = "d_alpha_floor")]
    pub alpha_floor: u8,
    /// The bbox may never exceed these fractions of the cell.
    #[serde(default = "d_safe_w")]
    pub safe_w: f64,
    #[serde(default = "d_safe_h")]
    pub safe_h: f64,
    /// Fixed square output canvas in px. 0 = keep the source canvas/resolution
    /// (today's shipping behaviour); e.g. 512 for fixed game cells (2× the ~228 px
    /// largest real cell).
    #[serde(default)]
    pub canvas: u32,
}

fn d_low() -> FitClass {
    FitClass { ink: 0.19, height: 0.60, tolerance: 0.02 }
}
fn d_high() -> FitClass {
    FitClass { ink: 0.30, height: 0.75, tolerance: 0.03 }
}
fn d_wild() -> FitClass {
    FitClass { ink: 0.34, height: 0.80, tolerance: 0.03 }
}
fn d_scatter() -> FitClass {
    FitClass { ink: 0.34, height: 0.80, tolerance: 0.03 }
}
fn d_area_weight() -> f64 {
    0.65
}
fn d_centroid_bias() -> f64 {
    0.7
}
fn d_alpha_floor() -> u8 {
    26
}
fn d_safe_w() -> f64 {
    0.92
}
fn d_safe_h() -> f64 {
    0.88
}

impl Default for SymbolSizing {
    fn default() -> Self {
        Self {
            low: d_low(),
            high: d_high(),
            wild: d_wild(),
            scatter: d_scatter(),
            area_weight: d_area_weight(),
            centroid_bias: d_centroid_bias(),
            alpha_floor: d_alpha_floor(),
            safe_w: d_safe_w(),
            safe_h: d_safe_h(),
            canvas: 0,
        }
    }
}

impl SymbolSizing {
    /// The fit class for a role. Bonus reads like a scatter; Special sits with the highs.
    pub fn class_for(&self, role: SymbolRole) -> FitClass {
        match role {
            SymbolRole::Low => self.low,
            SymbolRole::High | SymbolRole::Special => self.high,
            SymbolRole::Wild | SymbolRole::ExpandingWild => self.wild,
            SymbolRole::Scatter | SymbolRole::Bonus => self.scatter,
        }
    }
}

/// One buy-bonus mode. Two or more of these unlock the buy-bonus selector screen
/// (ASSETS.md §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BuyBonusMode {
    pub key: String,
    pub name: String,
}

/// The full, generic game configuration that drives asset derivation. Nothing here is
/// specific to any single game — Babewyn and a 5x3 lines game are just two instances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GameConfig {
    /// Folder-safe id, e.g. `babewyn_court`.
    pub game_id: String,
    /// Display name, e.g. "Babewyn Court".
    pub name: String,
    /// Free-form theme / art-direction brief for the whole game (documentation).
    #[serde(default)]
    pub brief: String,
    /// The **style master**: the hand-authored aesthetic core prepended to every
    /// asset prompt. This is the primary anti-slop lever — the studio owns it.
    #[serde(default)]
    pub style_prompt: String,
    /// The **negative master**: hand-authored negatives applied to every asset
    /// (suppresses the AI-slop tells). Falls back to a studio default if empty.
    #[serde(default)]
    pub negative_prompt: String,
    pub win_type: WinType,
    pub cols: u32,
    pub rows: u32,
    pub symbols: Vec<SymbolDef>,

    /// Whether the game ships a distinct feature/free-spins background (ASSETS.md §3).
    #[serde(default = "default_true")]
    pub has_feature_background: bool,

    /// Whether a buy-bonus button is shown (ASSETS.md §9 `button_buy_bonus`).
    #[serde(default)]
    pub has_buy_bonus: bool,
    /// Buy-bonus modes; `len() >= 2` unlocks the selector screen.
    #[serde(default)]
    pub buy_bonus_modes: Vec<BuyBonusMode>,

    /// Feature-collection meter mechanic (ASSETS.md §9 meter block).
    #[serde(default)]
    pub has_meter: bool,
    #[serde(default)]
    pub meter_thresholds: u32,

    /// Mystery-symbol mechanic (ASSETS.md §5/§6).
    #[serde(default)]
    pub has_mystery: bool,
    /// Hold-and-spin mechanic (ASSETS.md §5 `symbol_hold_indicator`).
    #[serde(default)]
    pub hold_and_spin: bool,

    /// On-screen mascot character beside/above the reels (ASSETS.md §10 feature hero).
    #[serde(default)]
    pub has_mascot: bool,
    /// Who the mascot is — flows into the generation prompt as the subject.
    #[serde(default)]
    pub mascot_description: String,

    /// Visual-mass sizing targets for symbols (Process normalizes to these).
    #[serde(default)]
    pub symbol_sizing: SymbolSizing,

    /// Default provider id for SYMBOL generation ("" = whatever the bench has selected).
    #[serde(default)]
    pub symbol_provider: String,

    /// Scene-asset system: plates / layers / set-piece sprites with composition guides.
    #[serde(default)]
    pub scene: SceneConfig,
}

fn default_true() -> bool {
    true
}

// ── Scene assets ──────────────────────────────────────────────────────────────

/// Scene-asset sub-types. Symbols are square grid cutouts; scene art is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SceneKind {
    /// Full-frame opaque background at a fixed preset resolution.
    Plate,
    /// Transparent element that stacks over a plate at the SAME canvas (e.g. the storm
    /// sky seen inside a window arch) — generated and exported with alpha.
    Layer,
    /// Standalone alpha object not bound to the reel grid (e.g. an observation booth).
    Sprite,
    /// Effect still meant to become a short looping spritesheet (flame, steam, arc).
    Fx,
    /// Tiny single-sprite alpha texture for the particle system (rain streak, dust mote).
    Particle,
}

impl SceneKind {
    /// The game-side naming prefix for this class, auto-applied to derived keys.
    /// Scene layers of every visual kind ship as `bg_*`; effects `fx_*`; particles `p_*`.
    pub fn prefix(self) -> &'static str {
        match self {
            SceneKind::Plate | SceneKind::Layer | SceneKind::Sprite => "bg_",
            SceneKind::Fx => "fx_",
            SceneKind::Particle => "p_",
        }
    }
}

/// A named fixed resolution that plates/layers generate and export at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScenePreset {
    pub key: String,
    pub width: u32,
    pub height: u32,
}

/// One labeled composition zone, in PERCENT of the canvas (0–100) — simple enough to
/// type by hand straight from a runtime real-estate report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideZone {
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A named set of zones bound to one preset (e.g. "hud_landscape": reel frame block,
/// HUD top/bottom bands, portrait-surviving centre strip, safe margins).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideTemplate {
    pub key: String,
    /// Preset these zones lay over.
    pub preset: String,
    #[serde(default)]
    pub zones: Vec<GuideZone>,
}

/// Placement metadata for the runtime scene manifest (`scene.json`). Contract owned by
/// the game runtime: normalized 0–1 canvas coordinates, anchor is a pivot within the
/// texture, z-order = manifest array order, parallax is a 0–1 depth multiplier
/// (back ≈ 0.05, mid ≈ 0.3, foreground ≈ 0.8) — code owns the drift amplitude.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScenePlacement {
    /// "cover" (full-bleed) | "anchored" (set-piece). Empty → cover for plates/layers,
    /// anchored for sprites/fx/particles.
    #[serde(default)]
    pub fit: String,
    /// Pivot within the texture, [x, y] 0–1 (default center; [0.5, 1] = floor-standing).
    #[serde(default)]
    pub anchor: Option<Vec<f64>>,
    /// Where the anchor lands, [x, y] normalized 0–1 of the live canvas.
    #[serde(default)]
    pub pos: Option<Vec<f64>>,
    /// Sprite height as a fraction of canvas height (width follows aspect). Anchored only.
    #[serde(default)]
    pub height: Option<f64>,
    /// Depth multiplier 0–1.
    #[serde(default)]
    pub parallax: Option<f64>,
    /// Cover oversize so parallax never reveals an edge (cover only, e.g. 0.08).
    #[serde(default)]
    pub overscan: Option<f64>,
    /// Optional blend mode: "" (normal) | "add" | "screen".
    #[serde(default)]
    pub blend: String,
    /// fx only: playback rate; the runtime converts to animationSpeed.
    #[serde(default)]
    pub fps: Option<f64>,
    /// fx only: whether the clip loops.
    #[serde(default, rename = "loop")]
    pub looped: Option<bool>,
}

/// One variant of a scene asset along the orientation/state axes ("landscape",
/// "portrait", "feature_landscape", "tempest"…). Derived asset key = `<asset>_<variant>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneVariantDef {
    pub key: String,
    /// Preset for plates/layers ("" → the project's first preset). Sprites may leave it
    /// empty for a square canvas.
    #[serde(default)]
    pub preset: String,
    /// Appended to the asset's shared prompt core for this variant only.
    #[serde(default)]
    pub extra_prompt: String,
    /// Placement OVERRIDES for this variant (shallow-merged over the asset's placement
    /// in the manifest; variants named "portrait"/"tablet" emit as manifest overrides).
    #[serde(default)]
    pub placement: Option<ScenePlacement>,
}

/// A scene asset: one shared prompt core plus linked variants managed as a group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneAssetDef {
    /// Base key; variants derive `<key>_<variant>` (e.g. `bg_base` → `bg_base_landscape`).
    pub key: String,
    pub kind: SceneKind,
    #[serde(default)]
    pub name: String,
    /// Shared prompt core — what this scene asset IS, across every variant.
    #[serde(default)]
    pub description: String,
    /// Provider override for this asset ("" → the scene class default).
    #[serde(default)]
    pub provider: String,
    /// The element has interior see-through openings (window glass, arch gaps): they
    /// are generated as flat chroma fills and keyed to real transparency by Process.
    #[serde(default)]
    pub cutouts: bool,
    /// The texture must tile seamlessly in the HORIZONTAL direction (panning cloud
    /// band): adds the tileable prompt clause + the seam check in the bench.
    #[serde(default)]
    pub wrap: bool,
    /// Runtime placement for the scene manifest (None → asset omitted from scene.json).
    #[serde(default)]
    pub placement: Option<ScenePlacement>,
    #[serde(default)]
    pub variants: Vec<SceneVariantDef>,
}

/// Per-project scene system config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SceneConfig {
    #[serde(default = "default_scene_presets")]
    pub presets: Vec<ScenePreset>,
    #[serde(default)]
    pub guides: Vec<GuideTemplate>,
    #[serde(default)]
    pub assets: Vec<SceneAssetDef>,
    /// Default provider id for the scene class ("" = whatever the bench has selected).
    #[serde(default)]
    pub default_provider: String,
    /// WebP quality for scene exports (10–100).
    #[serde(default = "d_webp_quality")]
    pub webp_quality: u32,
}

fn default_scene_presets() -> Vec<ScenePreset> {
    vec![
        ScenePreset { key: "landscape".into(), width: 2048, height: 1152 },
        ScenePreset { key: "portrait".into(), width: 1242, height: 2208 },
    ]
}
fn d_webp_quality() -> u32 {
    90
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            presets: default_scene_presets(),
            guides: Vec::new(),
            assets: Vec::new(),
            default_provider: String::new(),
            webp_quality: d_webp_quality(),
        }
    }
}

impl SceneConfig {
    /// Resolve a preset's dimensions ("" or unknown → the first preset, else a 16:9 default).
    pub fn preset_dims(&self, key: &str) -> (u32, u32) {
        self.presets
            .iter()
            .find(|p| p.key == key)
            .or_else(|| self.presets.first())
            .map(|p| (p.width, p.height))
            .unwrap_or((2048, 1152))
    }

    /// The class-prefixed base key an asset derives under (`bg_base` stays `bg_base`,
    /// `base` becomes `bg_base`; effects get `fx_`, particles `p_`).
    pub fn prefixed_key(def: &SceneAssetDef) -> String {
        let prefix = def.kind.prefix();
        if def.key.starts_with(prefix) {
            def.key.clone()
        } else {
            format!("{prefix}{}", def.key)
        }
    }

    /// The scene asset definition a derived asset key belongs to, if any
    /// (matches the prefixed base key exactly or as `<base>_<variant>`).
    pub fn def_for_key(&self, asset_key: &str) -> Option<&SceneAssetDef> {
        self.assets.iter().find(|def| {
            let base = Self::prefixed_key(def);
            asset_key == base || asset_key.starts_with(&format!("{base}_"))
        })
    }
}

impl GameConfig {
    /// Resolve an asset key to its symbol def, plus whether the key is the
    /// full-column `_expanded` twin of an expanding wild.
    pub fn symbol_for_asset(&self, asset_key: &str) -> Option<(&SymbolDef, bool)> {
        let k = asset_key.strip_prefix("symbol_")?;
        if let Some(s) = self.symbols.iter().find(|s| s.key == k) {
            return Some((s, false));
        }
        let base = k.strip_suffix("_expanded")?;
        self.symbols
            .iter()
            .find(|s| s.key == base && s.role == SymbolRole::ExpandingWild)
            .map(|s| (s, true))
    }

    // Used by prompt grouping in M2.
    #[allow(dead_code)]
    pub fn symbols_by_role(&self, role: SymbolRole) -> impl Iterator<Item = &SymbolDef> {
        self.symbols.iter().filter(move |s| s.role == role)
    }
}
