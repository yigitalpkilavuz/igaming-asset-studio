use serde::{Deserialize, Serialize};
use specta::Type;

/// Broad asset category — drives which dist subdirectory it lands in and how the UI
/// groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Symbol,
    Background,
    ReelChrome,
    SymbolChrome,
    Panel,
    Payline,
    Branding,
    Splash,
    /// The on-screen character beside/above the reels (idle + reaction clips).
    Mascot,
    /// Transparent scene element that stacks over a plate at the same canvas.
    SceneLayer,
    /// Standalone alpha set-piece object, not bound to the reel grid.
    SceneSprite,
    /// A looping/one-shot music bed (base game, free spins, big-win jingle).
    Music,
    /// A short sound effect (spin, reel stop, win tick, UI click…).
    Sfx,
    /// A bitmap font (BMFont `.xml` + `.webp` page atlas) for runtime `BitmapText`.
    Font,
}

/// How an asset is produced. Only `Raster` assets flow through the generation loop;
/// the rest are handled in code / at runtime and are surfaced as "waived" in the UI.
/// (ASSET_SOURCING flags procedural-vs-raster per asset.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Production {
    /// Generatable image (goes through providers + post-processing).
    Raster,
    /// Drawn in code with PIXI at runtime (paylines, cluster outlines, win glow).
    Procedural,
    /// Rendered from a bitmap font at runtime.
    RuntimeFont,
    /// Requires an external authoring tool (e.g. 9-slice measured by hand, Spine).
    ManualTool,
    /// Generatable audio (goes through the audio providers + ffmpeg post-processing).
    /// Deliberately NOT `Raster` so it bypasses every image path (generate/process/export/
    /// tone-gate/UI); the audio stages branch off at the four dedicated hook points.
    Audio,
    /// A bitmap font, baked deterministically from a `FontDef` at export (rasterize a bundled
    /// typeface → BMFont `.xml` + `.webp` page). Like `Audio`, NOT `Raster` — no AI/provider,
    /// no per-variation processing; the export loop branches to `bake_fonts`.
    Font,
}

/// Output file formats an asset must ship in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Webp,
    Png,
    Jpg,
    /// A `<name>.9.json` inset descriptor accompanies the raster (ASSETS.md §4).
    NineSliceJson,
    /// Ogg/Vorbis — primary audiosprite twin for Chrome/Firefox (Howler-picked first).
    Ogg,
    /// MPEG-4/AAC audiosprite twin (Safari + everywhere).
    M4a,
    /// MP3 — universal audiosprite fallback twin.
    Mp3,
    /// AC-3 audiosprite twin (Safari).
    Ac3,
    /// BMFont descriptor `.xml` (accompanies the `.webp` glyph page).
    FontXml,
}

/// 9-slice border insets in author px on the source raster (ASSETS.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Insets {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl Insets {
    pub const fn uniform(v: u32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

/// A single concrete asset the game needs, derived from `GameConfig`. This is the unit
/// the asset grid renders and the generation/processing/export stages operate on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssetDescriptor {
    /// Stable key, e.g. `symbol_h1`, `bg_base_landscape`, `reel_frame`.
    pub key: String,
    pub kind: AssetKind,
    /// Dist subdirectory (ASSETS.md §16), e.g. `backgrounds`, `reel_chrome`, `panels`.
    pub category: String,
    /// Back-reference to the ASSETS.md section this rule encodes, e.g. `§2`.
    pub spec_ref: String,
    pub production: Production,
    /// Author-px width/height. `None` for procedural/runtime assets with no fixed size.
    pub author_w: Option<u32>,
    pub author_h: Option<u32>,
    pub formats: Vec<Format>,
    pub nine_slice: Option<Insets>,
    /// Whether the delivery checklist (§18) treats this as required.
    pub required: bool,
    /// Human description carried into prompt generation (M2).
    pub description: String,
}
