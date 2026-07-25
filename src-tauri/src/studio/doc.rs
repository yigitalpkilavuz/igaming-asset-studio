//! The canonical Animation Studio document. Persisted at `assets/<key>/studio/studio.json`.
//!
//! Coordinate convention: **all positions are source-image pixels, y-down, origin top-left**
//! (what the canvas editor works in). Rotations are Spine-convention degrees (CCW when the
//! image is viewed normally). Only the `spine42` emitter converts positions to Spine's
//! y-up, bone-local space — there is exactly one conversion site.
//!
//! Animation value semantics match Spine timelines so the editor shows what ships:
//! rotate/translate keys are OFFSETS from the setup pose, scale keys are MULTIPLIERS of the
//! setup scale, slot alpha is absolute 0..1.

use serde::{Deserialize, Serialize};
use specta::Type;

pub const DOC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StudioDoc {
    /// Document schema version, for future migrations.
    pub version: u32,
    pub source: SourceRef,
    /// Draw order = index (0 = drawn first, i.e. behind).
    pub parts: Vec<Part>,
    /// Flattened bone tree; parent by name; index 0 is always `root`.
    pub bones: Vec<Bone>,
    /// One slot per part (usually); array order mirrors `parts` draw order.
    pub slots: Vec<Slot>,
    /// Spine 4.2 physics constraints — the RUNTIME simulates these (dangle/sway with
    /// inertia), no keyframes involved.
    #[serde(default)]
    pub physics: Vec<PhysicsSpec>,
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub settings: StudioSettings,
    /// The planned animation, described BEFORE cutting — it steers the auto-cut
    /// (every element that moves on its own becomes its own part) and seeds the AI
    /// clip brief later. Seeded from the Blueprint's per-symbol animation note.
    #[serde(default)]
    pub motion_brief: String,
    pub updated_at: f64,
}

/// Which processed variation the studio source was snapshotted from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub variation_id: String,
    pub width: u32,
    pub height: u32,
    /// sha256 of `source.png` — gates the SAM embedding and inpaint caches.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    /// Slug id: `head`, `left_arm`… Also the atlas region + attachment name.
    pub id: String,
    pub name: String,
    /// SAM click prompts that produced the current mask (re-runnable).
    #[serde(default)]
    pub prompts: Vec<SamPrompt>,
    /// Bounding box of `cut.png` within the source image; `None` until cut.
    #[serde(default)]
    pub bbox: Option<Rect>,
    /// sha256 of `mask.png` — drives inpaint invalidation.
    #[serde(default)]
    pub mask_hash: Option<String>,
    /// Which `completed.<hash8>.png` is current, if inpainted.
    #[serde(default)]
    pub completed_hash: Option<String>,
    /// Bbox of the completed texture (it extends past `bbox` into the hidden band).
    #[serde(default)]
    pub completed_bbox: Option<Rect>,
    /// Which file feeds the atlas: the raw cut or the inpaint-completed texture.
    #[serde(default)]
    pub texture: PartTexture,
}

impl Part {
    /// The bbox of whichever texture currently feeds the atlas.
    pub fn effective_bbox(&self) -> Option<Rect> {
        match (&self.texture, self.completed_bbox) {
            (PartTexture::Completed, Some(bb)) => Some(bb),
            _ => self.bbox,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum PartTexture {
    #[default]
    Cut,
    Completed,
}

/// A SAM point prompt, normalized 0..1 over the source image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SamPrompt {
    pub x: f64,
    pub y: f64,
    pub label: SamLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SamLabel {
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A bone. `x`/`y` are the pivot in source pixels (y-down, world space — the emitter
/// derives parent-local transforms). `rotation` is Spine-convention CCW degrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Bone {
    pub name: String,
    /// `None` only for `root`.
    pub parent: Option<String>,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub length: f64,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl Bone {
    pub fn new(name: impl Into<String>, parent: Option<String>, x: f64, y: f64) -> Self {
        Self {
            name: name.into(),
            parent,
            x,
            y,
            rotation: 0.0,
            length: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    /// Slot name; `== part_id` by convention.
    pub name: String,
    pub bone: String,
    pub part_id: String,
    #[serde(default)]
    pub attachment: Attachment,
    /// Spine blend mode. `Additive` is the light-effect mode: black adds nothing, so FX
    /// layers generated on black become pure light.
    #[serde(default)]
    pub blend: BlendMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum BlendMode {
    #[default]
    Normal,
    Additive,
    Multiply,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum Attachment {
    /// Rigid quad of the part texture bound to the slot's bone.
    #[default]
    Region,
    /// Deformable mesh (Phase 7).
    Mesh(MeshData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MeshData {
    /// Vertex positions in source pixels, 2 floats per vertex.
    pub vertices: Vec<f64>,
    /// 0..1 within the part texture, 2 per vertex.
    pub uvs: Vec<f64>,
    pub triangles: Vec<u32>,
    pub hull: u32,
    /// Per-vertex bone influences; `None` = bind rigidly to the slot bone.
    #[serde(default)]
    pub weights: Option<Vec<VertexWeights>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VertexWeights {
    pub influences: Vec<BoneInfluence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BoneInfluence {
    pub bone: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    /// Animation name in the exported skeleton: `idle`, `win`, `expand`…
    pub name: String,
    /// Seconds.
    pub duration: f64,
    pub looping: bool,
    pub timelines: Vec<Timeline>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub target: TimelineTarget,
    /// Sorted by time ascending.
    pub keys: Vec<Key>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TimelineTarget {
    /// Degrees, offset from setup pose.
    BoneRotate(String),
    /// Pixels (source scale), offset from setup pose. `v = [x, y]`.
    BoneTranslate(String),
    /// Multiplier of setup scale. `v = [x, y]`.
    BoneScale(String),
    /// Absolute alpha 0..1 on a slot.
    SlotAlpha(String),
}

impl TimelineTarget {
    /// Number of value components a key on this timeline carries.
    pub fn components(&self) -> usize {
        match self {
            TimelineTarget::BoneRotate(_) | TimelineTarget::SlotAlpha(_) => 1,
            TimelineTarget::BoneTranslate(_) | TimelineTarget::BoneScale(_) => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub time: f64,
    /// 1 value (rotate/alpha) or 2 (translate/scale).
    pub v: Vec<f64>,
    /// Ease OUT of this key toward the next.
    #[serde(default)]
    pub curve: Curve,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum Curve {
    #[default]
    Linear,
    Stepped,
    /// Normalized unit-square handles, 4 floats `(cx1, cy1, cx2, cy2)` per value component
    /// (so 4 for rotate/alpha, 8 for translate/scale). The emitter denormalizes to Spine
    /// 4.x absolute time/value control points.
    Bezier(Vec<f64>),
}

/// Standard ease-in-out handles for one component.
pub const EASE_IN_OUT: [f64; 4] = [0.42, 0.0, 0.58, 1.0];

/// A Spine 4.2 physics constraint on one bone. Defaults make a pleasant rotational sway
/// for chain segments (crests, capes, pendants); the runtime computes the motion from how
/// the bone's parent moves — no keyframes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PhysicsSpec {
    pub bone: String,
    /// How much physics drives the bone's rotation (0..1).
    #[serde(default = "one")]
    pub rotate: f64,
    /// How much prior momentum carries over (0..1). Higher = floatier.
    #[serde(default = "half")]
    pub inertia: f64,
    /// Spring pull back toward the pose. Higher = stiffer.
    #[serde(default = "d_strength")]
    pub strength: f64,
    /// Velocity decay per step (0..1). Lower = wobbles longer.
    #[serde(default = "d_damping")]
    pub damping: f64,
    /// Constant sideways force (world units/sec).
    #[serde(default)]
    pub wind: f64,
    /// Constant downward force (world units/sec).
    #[serde(default)]
    pub gravity: f64,
    /// Blend between posed and physical result (0..1).
    #[serde(default = "one")]
    pub mix: f64,
}

fn one() -> f64 {
    1.0
}
fn half() -> f64 {
    0.5
}
fn d_strength() -> f64 {
    60.0
}
fn d_damping() -> f64 {
    0.85
}

impl PhysicsSpec {
    /// The default rotational sway for a bone. (The UI builds specs directly; this is
    /// the canonical preset used by tests and the runtime-validation harness.)
    #[allow(dead_code)]
    pub fn sway(bone: impl Into<String>) -> Self {
        Self {
            bone: bone.into(),
            rotate: 1.0,
            inertia: 0.5,
            strength: 60.0,
            damping: 0.85,
            wind: 0.0,
            gravity: 0.0,
            mix: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StudioSettings {
    /// Timeline display fps (playback is continuous; this is snapping granularity).
    pub fps: u32,
    /// Default inpaint band radius in px (0 = auto per part).
    pub band_radius: f64,
    pub onion_before: u32,
    pub onion_after: u32,
    /// Pixels of duplicated boundary a part keeps UNDER its covering neighbours when cut.
    /// Hides razor seams the moment parts move (the classic cutout-artist trick). 0 = off.
    #[serde(default = "d_seam_fringe")]
    pub seam_fringe: u32,
}

fn d_seam_fringe() -> u32 {
    3
}

impl Default for StudioSettings {
    fn default() -> Self {
        Self { fps: 30, band_radius: 0.0, onion_before: 2, onion_after: 2, seam_fringe: 3 }
    }
}

impl StudioDoc {
    /// Seed a fresh document for a newly imported source: the whole image as a single part
    /// on a `body` bone at the image centre, with a canned looping "idle" breathe clip.
    /// Immediately exportable — segmentation/rigging refine from here.
    pub fn seed(source: SourceRef, updated_at: f64) -> Self {
        let (w, h) = (source.width as f64, source.height as f64);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let part = Part {
            id: "all".into(),
            name: "All".into(),
            prompts: Vec::new(),
            bbox: Some(Rect { x: 0, y: 0, w: source.width, h: source.height }),
            mask_hash: None,
            completed_hash: None,
            completed_bbox: None,
            texture: PartTexture::Cut,
        };
        let root = Bone::new("root", None, cx, cy);
        let body = Bone::new("body", Some("root".into()), cx, cy);
        let slot = Slot {
            name: "all".into(),
            bone: "body".into(),
            part_id: "all".into(),
            attachment: Attachment::Region,
            blend: BlendMode::Normal,
        };
        Self {
            version: DOC_VERSION,
            source,
            parts: vec![part],
            bones: vec![root, body],
            slots: vec![slot],
            physics: Vec::new(),
            clips: vec![Self::breathe_idle("body")],
            settings: StudioSettings::default(),
            motion_brief: String::new(),
            updated_at,
        }
    }

    /// The canned breathe clip: 2 s loop, gentle scale swell on one bone.
    pub fn breathe_idle(bone: &str) -> Clip {
        let ease2: Vec<f64> = [EASE_IN_OUT, EASE_IN_OUT].concat();
        Clip {
            id: "idle".into(),
            name: "idle".into(),
            duration: 2.0,
            looping: true,
            timelines: vec![Timeline {
                target: TimelineTarget::BoneScale(bone.to_string()),
                keys: vec![
                    Key { time: 0.0, v: vec![1.0, 1.0], curve: Curve::Bezier(ease2.clone()) },
                    Key { time: 1.0, v: vec![1.015, 1.03], curve: Curve::Bezier(ease2) },
                    Key { time: 2.0, v: vec![1.0, 1.0], curve: Curve::Linear },
                ],
            }],
        }
    }

    /// Not used yet — the rig/animate phases look bones up by name constantly.
    #[allow(dead_code)]
    pub fn bone(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.name == name)
    }

    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }
}
