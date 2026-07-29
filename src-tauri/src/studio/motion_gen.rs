//! Procedural motion engine. The cloud model DIRECTS — it assigns a motion PRIMITIVE
//! (bob, sway, breathe, spin, shake, pulse, glint…) with a few params to each moving bone
//! or slot — and this engine renders clean, seamlessly-looping, eased keyframes. LLMs can't
//! hand-key believable motion, but they reliably map a brief → "head bobs, cape sways, gem
//! glints" (the same SAM-regions → LLM-labels split that fixed cutting). All motion here is
//! smooth and loop-closed by construction, so the output is production-usable, not a
//! best-effort pile of guessed numbers.

use std::f64::consts::PI;

use serde::Deserialize;

use super::doc::{
    Attachment, Clip, Curve, Key, MeshData, StudioDoc, Timeline, TimelineTarget, EASE_IN_OUT,
};

/// The system prompt: the model returns a PLAN (primitive assignments), never keyframes.
pub const PLAN_SYSTEM: &str = "\
You are directing a looping animation for a premium slot-game symbol. You do NOT write \
keyframes — you assign ONE motion PRIMITIVE to each bone or slot that should move, and the \
engine generates smooth, seamlessly-looping keyframes. Be tasteful: a slot symbol is subtle, \
not a cartoon.\n\
You are SHOWN the symbol image — look at it and ground every choice in what you SEE: glint \
metal/gems/glass/jewels, ripple cloth/flags/flame/hair/liquid, blink eyes, sway loose or \
hanging parts (capes, tails, chains), breathe the torso/body. Bridge the visible materials to \
the named bones/slots listed below; match material and role, not just the name.\n\
Primitives:\n\
BONE: bob (gentle vertical float, amp = pixels) · sway (slow rotate rock, amp = degrees) · \
swing (bigger limb/cape/tail rotate, amp = degrees) · breathe (torso/root scale pulse, amp = \
fraction like 0.03) · shift (a slow side-to-side weight-shift of the body/root, amp = pixels — \
the anchor of a lively idle) · spin (continuous rotation, amp = number of turns) · shake (a \
decaying impact jitter, amp = pixels).\n\
SLOT: pulse (alpha throb, amp = peak 0..1) · glint (colour shimmer — tints toward `color` hex \
and back, for gems/metal/glass; use cycles=1 for a single win FLASH) · ripple (soft \
jelly/cloth/flame wobble, amp = pixels — ONLY for a slot whose entry has \"mesh\": true) · \
blink/swap (briefly show an alternate attachment named by `to` — a wink, a mouth shape — ONLY \
for a slot whose \"attachments\" lists more than one).\n\
EVENT BEATS — for a NON-idle clip (win, win_big, anticipation, bonus, expand): set loop=false \
and use these ONE-SHOT beats (with `at` = 0..1 for when the hit lands): pop (bone squash → \
stretch → overshoot → settle, amp = stretch fraction ~0.15) · anticipate (bone winds back then \
releases, amp = degrees) · impact (bone shake hit, amp = pixels) · reveal (bone grows from \
scale `amp` with overshoot — for expand). A great win = anticipate (early `at`) + pop (later \
`at`) + a glint flash; give the pop a touch of anticipation lead.\n\
Params: amp (see above), cycles (oscillations over the clip — keep whole for a clean loop; \
1–3 for idle, more for energetic), phase (0..6.28 — give bones DEEPER in the chain a larger \
phase so motion flows outward as follow-through), color (hex, glint only), to (attachment \
name, swap/blink only), at (0..1, beats only — when the beat peaks).\n\
For an IDLE, build a LIVING stance, not a bobbing prop: ANCHOR with a shift (and/or breathe) on \
the body/root, then layer subtle secondary motion OUTWARD — head, limbs, cape/tail sway that \
lags with follow-through. A few coordinated moves read far better than many independent ones; \
keep idles 3–5s so they don't obviously repeat.\n\
Use the brief to decide WHICH parts move and how strongly. Prefer 2–6 moves; err subtle for \
idles, be punchy for events. Set duration (seconds); loop=true for idles, loop=false for \
event reactions.\n\
Output ONLY this JSON object: {\"duration\": number, \"loop\": boolean, \"moves\": [{\"target\": \
\"boneOrSlotName\", \"kind\": \
\"bob|sway|swing|breathe|shift|spin|shake|pulse|glint|ripple|blink|pop|anticipate|impact|reveal\", \
\"amp\": number, \"cycles\": number, \"phase\": number, \"at\": number, \"color\": \"rrggbb\", \"to\": \
\"attachmentName\"}]}";

#[derive(Debug, Deserialize)]
pub struct PlanDraft {
    #[serde(default)]
    pub duration: f64,
    #[serde(default, alias = "loop")]
    pub looping: Option<bool>,
    #[serde(default)]
    pub moves: Vec<MoveDraft>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MoveDraft {
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub amp: Option<f64>,
    #[serde(default)]
    pub cycles: Option<f64>,
    #[serde(default)]
    pub phase: Option<f64>,
    #[serde(default)]
    pub color: Option<String>,
    /// Target attachment name for a swap/blink (an alternate on the slot); defaults to the
    /// first alternate.
    #[serde(default)]
    pub to: Option<String>,
    /// When (0..1 of the clip) a one-shot BEAT peaks (pop/anticipate/flash/impact/reveal).
    #[serde(default)]
    pub at: Option<f64>,
}

fn ease() -> Curve {
    Curve::Bezier(EASE_IN_OUT.to_vec())
}
fn ease_n(comps: usize) -> Curve {
    Curve::Bezier(EASE_IN_OUT.iter().cycle().take(comps * 4).copied().collect())
}

/// One eased, loop-closed oscillation about `base` with peak `amp`. The shape is a fundamental
/// plus small 2nd/3rd harmonics (normalized so `amp` stays the true peak) — a hand-animated feel
/// rather than a bare sine — and `cycles` is whole so the last key equals the first (seamless).
fn wave(base: f64, amp: f64, cycles: f64, phase: f64, duration: f64) -> Vec<Key> {
    let cyc = cycles.round().max(1.0);
    let n = (cyc as usize) * 6; // denser so the richer shape reads smoothly
    let shape = |x: f64| x.sin() + 0.18 * (2.0 * x + 0.7).sin() + 0.09 * (3.0 * x + 1.9).sin();
    // Peak of |shape| over a full period, so dividing keeps `amp` the true amplitude.
    let peak = (0..360)
        .map(|d| shape((d as f64).to_radians()).abs())
        .fold(0.0_f64, f64::max)
        .max(1e-6);
    (0..=n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let v = base + amp * shape(2.0 * PI * cyc * f + phase) / peak;
            Key { time: duration * f, v: vec![v], curve: ease() }
        })
        .collect()
}

/// Deterministic 0..1 hash of a name — used to desync per-bone phase so an idle doesn't move in
/// lockstep (a tell of procedural motion).
fn hash01(s: &str) -> f64 {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    (h % 997) as f64 / 997.0
}

fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|b| b as f64 / 255.0);
    Some([ch(0)?, ch(2)?, ch(4)?])
}

/// The mesh a slot deforms, if it has one (deform primitives need a mesh attachment).
fn slot_mesh<'a>(doc: &'a StudioDoc, slot_name: &str) -> Option<&'a MeshData> {
    doc.slots
        .iter()
        .find(|s| s.name == slot_name)
        .and_then(|s| match &s.attachment {
            Attachment::Mesh(m) => Some(m),
            _ => None,
        })
}

/// How many attachments a slot can show: base + alternates (0 if the slot is unknown).
fn slot_attachment_count(doc: &StudioDoc, slot_name: &str) -> usize {
    doc.slots
        .iter()
        .find(|s| s.name == slot_name)
        .map_or(0, |s| 1 + s.alternates.len())
}

/// Index of an attachment name within a slot's `[part_id, ...alternates]`.
fn attachment_index(doc: &StudioDoc, slot_name: &str, att: &str) -> Option<usize> {
    let s = doc.slots.iter().find(|s| s.name == slot_name)?;
    std::iter::once(s.part_id.as_str())
        .chain(s.alternates.iter().map(|a| a.as_str()))
        .position(|n| n == att)
}

/// A soft mesh wobble: a wave that travels across the mesh in UV space and cycles in time.
/// Offsets are along +y (source px), whole `cycles` so the deform loops seamlessly.
fn ripple_keys(mesh: &MeshData, amp: f64, cycles: f64, phase: f64, duration: f64) -> Vec<Key> {
    let vcount = mesh.vertices.len() / 2;
    let cyc = cycles.round().max(1.0);
    let n = (cyc as usize) * 8;
    const SPATIAL: f64 = 1.2; // waves across the part
    (0..=n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let mut v = Vec::with_capacity(vcount * 2);
            for vi in 0..vcount {
                let u = mesh.uvs.get(vi * 2).copied().unwrap_or(0.0);
                let dy = amp * (2.0 * PI * (SPATIAL * u + cyc * f) + phase).sin();
                v.push(0.0); // dx
                v.push(dy); // dy (source px, y-down)
            }
            Key { time: duration * f, v, curve: Curve::Linear }
        })
        .collect()
}

/// A blink/swap: mostly the base attachment, with brief windows showing attachment `idx`,
/// repeated `cycles` times. Stepped (attachment swaps are instantaneous); loops on the base.
fn blink_keys(idx: f64, cycles: f64, duration: f64) -> Vec<Key> {
    let cyc = cycles.round().max(1.0);
    let dwell = (duration / cyc * 0.12).clamp(0.04, 0.16);
    let mut keys = vec![Key { time: 0.0, v: vec![0.0], curve: Curve::Stepped }];
    for c in 0..cyc as usize {
        let center = (c as f64 + 0.5) / cyc * duration;
        let s = (center - dwell / 2.0).max(0.0);
        let e = (center + dwell / 2.0).min(duration);
        keys.push(Key { time: s, v: vec![idx], curve: Curve::Stepped });
        keys.push(Key { time: e, v: vec![0.0], curve: Curve::Stepped });
    }
    keys
}

/// Render one directed move into a timeline, or `None` if the target/kind don't fit
/// (e.g. a bone primitive on a slot name). Amps are clamped to production-safe ranges.
pub fn render_move(doc: &StudioDoc, m: &MoveDraft, duration: f64) -> Option<Timeline> {
    let t = m.target.trim();
    let is_bone = doc.bones.iter().any(|b| b.name == t);
    let is_slot = doc.slots.iter().any(|s| s.name == t);
    let cycles = m.cycles.unwrap_or(1.0);
    // A small per-target phase offset desyncs the oscillators so bones aren't in lockstep.
    let phase = m.phase.unwrap_or(0.0) + hash01(t) * 0.7;
    let max_dim = doc.source.width.max(doc.source.height) as f64;

    let (target, keys) = match m.kind.trim().to_lowercase().as_str() {
        "bob" | "hover" | "float" | "bounce" if is_bone => {
            let amp = m.amp.unwrap_or(6.0).clamp(1.0, max_dim * 0.12);
            let keys = wave(0.0, amp, cycles, phase, duration)
                .into_iter()
                .map(|k| Key { time: k.time, v: vec![0.0, k.v[0]], curve: ease_n(2) }) // translate: y only
                .collect();
            (TimelineTarget::BoneTranslate(t.to_string()), keys)
        }
        "sway" | "rock" | "tilt" | "nod" if is_bone => {
            let amp = m.amp.unwrap_or(4.0).clamp(0.5, 30.0);
            (TimelineTarget::BoneRotate(t.to_string()), wave(0.0, amp, cycles, phase, duration))
        }
        "swing" if is_bone => {
            let amp = m.amp.unwrap_or(15.0).clamp(0.5, 45.0);
            (TimelineTarget::BoneRotate(t.to_string()), wave(0.0, amp, cycles, phase, duration))
        }
        "breathe" | "throb" if is_bone => {
            let amp = m.amp.unwrap_or(0.03).clamp(0.005, 0.15);
            let keys = wave(1.0, amp, cycles, phase, duration)
                .into_iter()
                .map(|k| Key { time: k.time, v: vec![k.v[0], k.v[0]], curve: ease_n(2) }) // uniform scale
                .collect();
            (TimelineTarget::BoneScale(t.to_string()), keys)
        }
        "shift" | "weight" | "settle" if is_bone => {
            // A living weight-shift: a slow side-to-side drift of the body with a gentle vertical
            // dip at each extreme (elliptical) — the anchor a good idle is built around.
            let amp = m.amp.unwrap_or(4.0).clamp(1.0, max_dim * 0.06);
            let cyc = cycles.round().max(1.0);
            let n = (cyc as usize) * 8;
            let keys = (0..=n)
                .map(|i| {
                    let f = i as f64 / n as f64;
                    let a = 2.0 * PI * cyc * f + phase;
                    let x = amp * a.sin();
                    let y = amp * 0.35 * (1.0 - (2.0 * a).cos()) / 2.0; // dips down at the extremes
                    Key { time: duration * f, v: vec![x, y], curve: ease_n(2) }
                })
                .collect();
            (TimelineTarget::BoneTranslate(t.to_string()), keys)
        }
        "spin" if is_bone => {
            let turns = m.amp.or(m.cycles).unwrap_or(1.0).round().max(1.0);
            let keys = vec![
                Key { time: 0.0, v: vec![0.0], curve: Curve::Linear },
                Key { time: duration, v: vec![360.0 * turns], curve: Curve::Linear },
            ];
            (TimelineTarget::BoneRotate(t.to_string()), keys)
        }
        "shake" | "shiver" | "recoil" if is_bone => {
            let amp = m.amp.unwrap_or(8.0).clamp(1.0, max_dim * 0.12);
            let n = 12usize;
            let keys = (0..=n)
                .map(|i| {
                    let f = i as f64 / n as f64;
                    let v = amp * (1.0 - f) * (2.0 * PI * 3.0 * f).sin(); // decaying jitter → 0
                    Key { time: duration * f, v: vec![v, 0.0], curve: Curve::Linear }
                })
                .collect();
            (TimelineTarget::BoneTranslate(t.to_string()), keys)
        }
        "ripple" | "wobble" | "jelly" | "jiggle" if is_slot => {
            let mesh = slot_mesh(doc, t)?; // only a mesh slot can deform
            let amp = m.amp.unwrap_or(6.0).clamp(1.0, max_dim * 0.08);
            (TimelineTarget::SlotDeform(t.to_string()), ripple_keys(mesh, amp, cycles, phase, duration))
        }
        "blink" | "swap" | "wink" if is_slot && slot_attachment_count(doc, t) > 1 => {
            // Which alternate: `to` names it, else the first alternate (index 1).
            let idx = m
                .to
                .as_deref()
                .and_then(|nm| attachment_index(doc, t, nm))
                .unwrap_or(1) as f64;
            (TimelineTarget::SlotAttachment(t.to_string()), blink_keys(idx, cycles, duration))
        }
        "pulse" | "glow" | "blink" if is_slot => {
            let hi = m.amp.unwrap_or(1.0).clamp(0.05, 1.0);
            let lo = (hi - 0.4).max(0.0);
            let keys = wave((hi + lo) / 2.0, (hi - lo) / 2.0, cycles, phase, duration);
            (TimelineTarget::SlotAlpha(t.to_string()), keys)
        }
        "glint" | "shimmer" | "flash" | "tint" if is_slot => {
            let col = m.color.as_deref().and_then(parse_hex).unwrap_or([1.0, 0.85, 0.5]);
            let cyc = cycles.round().max(1.0);
            let n = (cyc as usize) * 2;
            let keys = (0..=n)
                .map(|i| {
                    let mix = if i % 2 == 0 { 0.0 } else { 1.0 }; // white ↔ colour ping-pong
                    Key {
                        time: duration * i as f64 / n as f64,
                        v: (0..3).map(|c| 1.0 + (col[c] - 1.0) * mix).collect(),
                        curve: ease_n(3),
                    }
                })
                .collect();
            (TimelineTarget::SlotColor(t.to_string()), keys)
        }
        // ── One-shot event BEATS (non-looping envelopes — win / anticipation / expand) ──
        // `at` (0..1) is when the beat peaks; `amp` its intensity.
        "pop" | "punch" if is_bone => {
            let a = m.at.unwrap_or(0.35).clamp(0.15, 0.82);
            let s = m.amp.unwrap_or(0.15).clamp(0.03, 0.4); // stretch fraction
            let tm = |f: f64| (a + f).clamp(0.0, 1.0) * duration;
            // A parent bone's NON-UNIFORM scale shears/distorts every child part (the classic
            // mascot "body deformity"). So squash-&-stretch is only safe on a LEAF bone; a bone
            // with children pops UNIFORMLY (both axes together) — the whole figure bounces as one.
            let has_children = doc.bones.iter().any(|b| b.parent.as_deref() == Some(t));
            let keys = if has_children {
                // Uniform bounce: crouch → pop overshoot → small undershoot → settle. No shear.
                vec![
                    Key { time: 0.0, v: vec![1.0, 1.0], curve: ease_n(2) },
                    Key { time: tm(-0.12), v: vec![1.0 - 0.35 * s, 1.0 - 0.35 * s], curve: ease_n(2) },
                    Key { time: tm(0.0), v: vec![1.0 + s, 1.0 + s], curve: ease_n(2) },
                    Key { time: tm(0.16), v: vec![1.0 - 0.12 * s, 1.0 - 0.12 * s], curve: ease_n(2) },
                    Key { time: tm(0.34), v: vec![1.0, 1.0], curve: Curve::Linear },
                ]
            } else {
                // True squash & stretch (wider+shorter → taller+narrower), volume-preserving.
                vec![
                    Key { time: 0.0, v: vec![1.0, 1.0], curve: ease_n(2) },
                    Key { time: tm(-0.12), v: vec![1.0 + 0.6 * s, 1.0 - 0.6 * s], curve: ease_n(2) },
                    Key { time: tm(0.0), v: vec![1.0 - s, 1.0 + s], curve: ease_n(2) },
                    Key { time: tm(0.13), v: vec![1.0 + 0.3 * s, 1.0 - 0.3 * s], curve: ease_n(2) },
                    Key { time: tm(0.30), v: vec![1.0, 1.0], curve: Curve::Linear },
                ]
            };
            (TimelineTarget::BoneScale(t.to_string()), keys)
        }
        "anticipate" | "windup" if is_bone => {
            let a = m.at.unwrap_or(0.4).clamp(0.15, 0.8);
            let amp = m.amp.unwrap_or(12.0).clamp(2.0, 45.0);
            let tm = |f: f64| (a + f).clamp(0.0, 1.0) * duration;
            let keys = vec![
                Key { time: 0.0, v: vec![0.0], curve: ease() },
                Key { time: tm(0.0), v: vec![-amp], curve: ease() }, // wind back
                Key { time: tm(0.15), v: vec![amp * 0.4], curve: ease() }, // release overshoot
                Key { time: tm(0.35), v: vec![0.0], curve: Curve::Linear }, // settle
            ];
            (TimelineTarget::BoneRotate(t.to_string()), keys)
        }
        "impact" | "hit" if is_bone => {
            let a = m.at.unwrap_or(0.25).clamp(0.0, 0.7);
            let amp = m.amp.unwrap_or(8.0).clamp(1.0, max_dim * 0.12);
            let n = 10usize;
            let keys = (0..=n)
                .map(|j| {
                    let f = j as f64 / n as f64;
                    let time = (a + 0.4 * f).clamp(0.0, 1.0) * duration;
                    let vv = amp * (1.0 - f) * (2.0 * PI * 3.0 * f).sin(); // decaying jitter → 0
                    Key { time, v: vec![vv, 0.0], curve: Curve::Linear }
                })
                .collect();
            (TimelineTarget::BoneTranslate(t.to_string()), keys)
        }
        "reveal" | "appear" if is_bone => {
            let start = m.amp.unwrap_or(0.5).clamp(0.1, 0.95); // scale to grow FROM
            let keys = vec![
                Key { time: 0.0, v: vec![start, start], curve: ease_n(2) },
                Key { time: 0.65 * duration, v: vec![1.08, 1.08], curve: ease_n(2) }, // overshoot
                Key { time: duration, v: vec![1.0, 1.0], curve: Curve::Linear },
            ];
            (TimelineTarget::BoneScale(t.to_string()), keys)
        }
        _ => return None,
    };
    Some(Timeline { target, keys })
}

/// Named event clips are one-shot REACTIONS (win pop, anticipation build) — not looping idles.
fn is_event(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "win"
            | "win_big"
            | "winbig"
            | "bigwin"
            | "big_win"
            | "mega_win"
            | "megawin"
            | "mega"
            | "jackpot"
            | "anticipation"
            | "anticipate"
            | "bonus"
            | "feature"
            | "trigger"
            | "expand"
            | "grow"
            | "reveal"
            | "appear"
            | "intro"
            | "land"
            | "drop"
            | "scatter"
            | "celebrate"
            | "taunt"
            | "cheer"
            | "happy"
            | "hit"
    )
}

/// How deep a bone sits in its parent chain (root = 0).
fn chain_depth(doc: &StudioDoc, bone: &str) -> u32 {
    let mut d = 0;
    let mut cur = bone.to_string();
    while let Some(b) = doc.bones.iter().find(|b| b.name == cur) {
        match &b.parent {
            Some(p) => {
                d += 1;
                cur = p.clone();
            }
            None => break,
        }
        if d > 16 {
            break;
        }
    }
    d
}

/// Render a full directed plan into a clip, dropping unknown targets and duplicate channels.
/// Applies deterministic safety nets the model shouldn't have to get right: physics-driven bones
/// are left to their constraint (a keyframed sway would fight it), and bone moves the model left
/// phase-less get a depth-based lag so motion flows OUTWARD down the chain (follow-through).
pub fn render_plan(doc: &StudioDoc, id: &str, name: &str, plan: PlanDraft) -> Clip {
    let duration = if plan.duration > 0.0 { plan.duration.clamp(0.4, 12.0) } else { 2.0 };
    // Idles loop; named event clips are one-shot unless the model says otherwise.
    let looping = plan.looping.unwrap_or(!is_event(name));
    let physics: std::collections::HashSet<&str> =
        doc.physics.iter().map(|p| p.bone.as_str()).collect();
    let mut timelines: Vec<Timeline> = Vec::new();
    for m in plan.moves.iter().take(24) {
        let target = m.target.trim();
        let is_bone = doc.bones.iter().any(|b| b.name == target);
        if is_bone && physics.contains(target) {
            continue; // the runtime constraint already animates this bone
        }
        let mut mv = m.clone();
        if is_bone && mv.phase.is_none() {
            mv.phase = Some(chain_depth(doc, target) as f64 * 0.6);
        }
        let Some(tl) = render_move(doc, &mv, duration) else { continue };
        if tl.keys.is_empty() || timelines.iter().any(|x| x.target == tl.target) {
            continue;
        }
        timelines.push(tl);
    }

    // Deterministic fallback: an event brief that yielded no usable motion still gets a real
    // reaction — pop the primary body bone and flash the primary slot.
    if timelines.is_empty() && is_event(name) {
        let body = doc
            .bones
            .iter()
            .find(|b| b.parent.as_deref() == Some("root"))
            .or_else(|| doc.bones.iter().find(|b| b.parent.is_some()));
        let beat = |target: String, kind: &str| MoveDraft {
            target,
            kind: kind.to_string(),
            amp: None,
            cycles: None,
            phase: None,
            color: None,
            to: None,
            at: None,
        };
        if let Some(b) = body {
            if let Some(tl) = render_move(doc, &beat(b.name.clone(), "pop"), duration) {
                timelines.push(tl);
            }
        }
        if let Some(s) = doc.slots.first() {
            if let Some(tl) = render_move(doc, &beat(s.name.clone(), "glint"), duration) {
                timelines.push(tl);
            }
        }
    }

    Clip { id: id.to_string(), name: name.to_string(), duration, looping, timelines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{SourceRef, StudioDoc};

    fn doc() -> StudioDoc {
        StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 400, height: 400, sha256: "s".into() },
            0.0,
        )
    }

    #[test]
    fn is_event_splits_reactions_from_idles() {
        // One-shot reactions (and their aliases) → event beats, loop=false.
        for n in ["win", "Win_Big", "mega_win", "anticipation", "bonus", "expand", "reveal", "land",
                  "scatter", "celebrate"] {
            assert!(is_event(n), "{n} should be a one-shot event");
        }
        // Ambient states → looping idles.
        for n in ["idle", "idle_alt", "wild", "hover"] {
            assert!(!is_event(n), "{n} should loop, not one-shot");
        }
    }

    #[test]
    fn pop_is_uniform_on_a_parent_but_squashes_a_leaf() {
        use crate::studio::doc::Bone;
        let mut d = doc(); // seed already has root + body
        d.bones.push(Bone::new("head", Some("body".into()), 300.0, 180.0)); // body now has a child
        d.bones.push(Bone::new("coin", Some("root".into()), 100.0, 100.0)); // leaf: no children
        let pop = |target: &str| MoveDraft {
            target: target.into(), kind: "pop".into(), amp: Some(0.2),
            cycles: None, phase: None, color: None, to: None, at: Some(0.4),
        };
        // A parent bone's non-uniform scale would SHEAR its child parts, so a parent pops uniformly.
        let body = render_move(&d, &pop("body"), 1.5).unwrap();
        for k in &body.keys {
            assert!((k.v[0] - k.v[1]).abs() < 1e-9, "parent pop must be uniform (no child shear): {:?}", k.v);
        }
        assert!(body.keys.iter().any(|k| k.v[0] > 1.05), "…but it still pops (scales up)");
        // A leaf part has no children to distort → true squash & stretch (anti-correlated axes).
        let coin = render_move(&d, &pop("coin"), 1.5).unwrap();
        assert!(
            coin.keys.iter().any(|k| (k.v[0] - k.v[1]).abs() > 0.05),
            "leaf pop squashes & stretches"
        );
    }

    #[test]
    fn multi_component_primitives_carry_correctly_sized_curves() {
        // Regression: the 2-component remaps (shift/bob/breathe) once inherited wave's 1-component
        // (4-handle) easing curve, which the spine emitter rejects ("4 handles, expected 8").
        let mut d = doc(); // seed already has root + body
        d.bones.push(crate::studio::doc::Bone::new("head", Some("body".into()), 300.0, 180.0));
        for (kind, target) in [("shift", "body"), ("bob", "head"), ("breathe", "body")] {
            let tl = render_move(
                &d,
                &MoveDraft {
                    target: target.into(), kind: kind.into(), amp: Some(5.0),
                    cycles: Some(2.0), phase: None, color: None, to: None, at: None,
                },
                2.0,
            )
            .unwrap();
            let comps = tl.target.components().unwrap();
            for k in &tl.keys {
                assert_eq!(k.v.len(), comps, "{kind}: value arity");
                if let Curve::Bezier(h) = &k.curve {
                    assert_eq!(h.len(), comps * 4, "{kind}: bezier handle count must match components");
                }
            }
        }
    }

    #[test]
    fn wave_is_loop_closed_and_eased() {
        let k = wave(0.0, 10.0, 2.0, 0.0, 2.0);
        assert!((k.first().unwrap().v[0] - k.last().unwrap().v[0]).abs() < 1e-9, "start == end");
        assert!((k.first().unwrap().time - 0.0).abs() < 1e-9);
        assert!((k.last().unwrap().time - 2.0).abs() < 1e-9);
        assert!(matches!(k[0].curve, Curve::Bezier(_)), "eased, not linear");
        // Amplitude is normalized: the peak sits at ≈ amp, never blows past it.
        let peak = k.iter().map(|key| key.v[0].abs()).fold(0.0_f64, f64::max);
        assert!(peak > 8.0 && peak < 10.5, "peak ≈ amp: {peak}");
    }

    #[test]
    fn wave_has_organic_texture_not_a_bare_sine() {
        let k = wave(0.0, 10.0, 1.0, 0.0, 1.0);
        let n = (k.len() - 1) as f64;
        let dev = k
            .iter()
            .enumerate()
            .map(|(i, key)| (key.v[0] - 10.0 * (2.0 * PI * i as f64 / n).sin()).abs())
            .fold(0.0_f64, f64::max);
        assert!(dev > 0.3, "harmonic texture departs from a bare sine: {dev}");
    }

    #[test]
    fn shift_is_an_elliptical_loop_closed_weight_drift() {
        let d = doc();
        let tl = render_move(&d, &MoveDraft {
            target: "body".into(), kind: "shift".into(), amp: Some(6.0),
            cycles: Some(1.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(tl.target, TimelineTarget::BoneTranslate(_)));
        assert!(tl.keys.iter().all(|k| k.v.len() == 2));
        // Drifts to BOTH sides (a side-to-side weight shift), and loops seamlessly.
        assert!(tl.keys.iter().any(|k| k.v[0] > 1.0), "shifts right");
        assert!(tl.keys.iter().any(|k| k.v[0] < -1.0), "shifts left");
        assert!(
            tl.keys.first().unwrap().v.iter().zip(&tl.keys.last().unwrap().v).all(|(a, b)| (a - b).abs() < 1e-6),
            "loop closes"
        );
    }

    #[test]
    fn primitives_pick_the_right_channel_and_target() {
        let doc = doc(); // seed → bone "body", slot "all"
        let sway = render_move(&doc, &MoveDraft {
            target: "body".into(), kind: "sway".into(),
            amp: Some(6.0), cycles: Some(2.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(sway.target, TimelineTarget::BoneRotate(ref b) if b == "body"));

        let bob = render_move(&doc, &MoveDraft {
            target: "body".into(), kind: "bob".into(),
            amp: Some(8.0), cycles: Some(1.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(bob.target, TimelineTarget::BoneTranslate(_)));
        assert!(bob.keys.iter().all(|k| k.v.len() == 2 && k.v[0] == 0.0)); // y-only

        let glint = render_move(&doc, &MoveDraft {
            target: "all".into(), kind: "glint".into(),
            amp: None, cycles: Some(2.0), phase: None, color: Some("ffcc00".into()), to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(glint.target, TimelineTarget::SlotColor(_)));
        assert!(glint.keys.iter().all(|k| k.v.len() == 3));

        // A bone primitive on a slot name → dropped.
        assert!(render_move(&doc, &MoveDraft {
            target: "all".into(), kind: "sway".into(),
            amp: None, cycles: None, phase: None, color: None, to: None, at: None,
        }, 2.0).is_none());
    }

    #[test]
    fn plan_dedupes_and_bounds() {
        let doc = doc();
        let plan = PlanDraft {
            duration: 0.0, // → default 2.0
            looping: Some(true),
            moves: vec![
                MoveDraft { target: "body".into(), kind: "sway".into(), amp: Some(500.0), cycles: Some(1.0), phase: None, color: None, to: None, at: None },
                MoveDraft { target: "body".into(), kind: "sway".into(), amp: Some(3.0), cycles: Some(1.0), phase: None, color: None, to: None, at: None }, // dup channel
                MoveDraft { target: "ghost".into(), kind: "bob".into(), amp: None, cycles: None, phase: None, color: None, to: None, at: None }, // unknown
            ],
        };
        let clip = render_plan(&doc, "idle", "idle", plan);
        assert_eq!(clip.duration, 2.0);
        assert_eq!(clip.timelines.len(), 1, "dup channel + unknown target dropped");
        // Amp clamped: 500° → ≤30°.
        assert!(clip.timelines[0].keys.iter().all(|k| k.v[0].abs() <= 30.0 + 1e-9));
    }

    #[test]
    fn render_plan_skips_physics_bones_and_lags_chains() {
        let mut d = doc(); // seed: root, body
        d.bones.push(crate::studio::doc::Bone::new("tail", Some("body".into()), 200.0, 300.0));
        d.bones.push(crate::studio::doc::Bone::new("tail_seg2", Some("tail".into()), 200.0, 350.0));
        d.physics = vec![crate::studio::doc::PhysicsSpec::sway("tail_seg2")];
        let plan = PlanDraft {
            duration: 2.0,
            looping: Some(true),
            moves: vec![
                MoveDraft { target: "tail".into(), kind: "sway".into(), amp: Some(6.0), cycles: Some(1.0), phase: None, color: None, to: None, at: None },
                MoveDraft { target: "tail_seg2".into(), kind: "sway".into(), amp: Some(6.0), cycles: Some(1.0), phase: None, color: None, to: None, at: None },
            ],
        };
        let clip = render_plan(&d, "idle", "idle", plan);
        // The physics bone is left to its constraint; only "tail" survives.
        assert_eq!(clip.timelines.len(), 1);
        assert!(matches!(clip.timelines[0].target, TimelineTarget::BoneRotate(ref b) if b == "tail"));
        // "tail" is depth 2 → follow-through phase (2×0.6 rad) shifts the wave off sine-zero.
        let first = clip.timelines[0].keys.first().unwrap().v[0];
        assert!(first.abs() > 0.1, "follow-through phase shifts the start: {first}");
    }

    #[test]
    fn pop_beat_squashes_then_stretches_and_rests() {
        let d = doc();
        let tl = render_move(&d, &MoveDraft {
            target: "body".into(), kind: "pop".into(), amp: Some(0.2),
            cycles: None, phase: None, color: None, to: None, at: Some(0.4),
        }, 2.0).unwrap();
        assert!(matches!(tl.target, TimelineTarget::BoneScale(_)));
        // Anti-correlated squash (wider+shorter) then stretch (taller+narrower).
        assert!(tl.keys.iter().any(|k| k.v[0] > 1.0 && k.v[1] < 1.0), "crouch");
        assert!(tl.keys.iter().any(|k| k.v[0] < 1.0 && k.v[1] > 1.0), "stretch");
        // Begins and ends at rest; keys are time-ordered.
        assert_eq!(tl.keys.first().unwrap().v, vec![1.0, 1.0]);
        assert_eq!(tl.keys.last().unwrap().v, vec![1.0, 1.0]);
        for w in tl.keys.windows(2) {
            assert!(w[1].time >= w[0].time, "beat keys are time-ordered");
        }
    }

    #[test]
    fn event_clip_is_one_shot_with_a_fallback_reaction() {
        let d = doc(); // seed: bone "body", slot "all"
        let clip = render_plan(&d, "win", "win", PlanDraft { duration: 1.5, looping: None, moves: vec![] });
        assert!(!clip.looping, "event clips play once, not loop");
        assert!(!clip.timelines.is_empty(), "empty win brief still gets a reaction");
        assert!(
            clip.timelines.iter().any(|t| matches!(t.target, TimelineTarget::BoneScale(_))),
            "fallback pops the body"
        );
    }

    /// Seed doc whose "all" slot is a 3-vertex mesh AND carries an alternate attachment.
    fn mesh_alt_doc() -> StudioDoc {
        let mut d = doc();
        d.slots[0].attachment = Attachment::Mesh(MeshData {
            vertices: vec![0.0, 0.0, 100.0, 0.0, 50.0, 100.0],
            uvs: vec![0.0, 0.0, 1.0, 0.0, 0.5, 1.0],
            triangles: vec![0, 1, 2],
            hull: 3,
            weights: None,
        });
        d.slots[0].alternates = vec!["all_closed".into()];
        d
    }

    #[test]
    fn ripple_makes_a_loop_closed_deform_on_a_mesh_slot() {
        let d = mesh_alt_doc();
        let tl = render_move(&d, &MoveDraft {
            target: "all".into(), kind: "ripple".into(),
            amp: Some(6.0), cycles: Some(2.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(tl.target, TimelineTarget::SlotDeform(ref s) if s == "all"));
        // 3 verts → 6 values/key; wave travels in y (x stays 0); loop-closed.
        assert!(tl.keys.iter().all(|k| k.v.len() == 6));
        assert!(tl.keys.iter().all(|k| k.v[0] == 0.0 && k.v[2] == 0.0 && k.v[4] == 0.0));
        let (first, last) = (&tl.keys[0].v, &tl.keys.last().unwrap().v);
        assert!(first.iter().zip(last).all(|(a, b)| (a - b).abs() < 1e-9), "ripple loops");
    }

    #[test]
    fn ripple_on_a_region_slot_is_dropped() {
        let d = doc(); // "all" is a Region, not a mesh
        assert!(render_move(&d, &MoveDraft {
            target: "all".into(), kind: "ripple".into(),
            amp: None, cycles: None, phase: None, color: None, to: None, at: None,
        }, 2.0).is_none());
    }

    #[test]
    fn blink_swaps_to_the_alternate_and_back() {
        let d = mesh_alt_doc(); // "all" → alternate "all_closed" at index 1
        let tl = render_move(&d, &MoveDraft {
            target: "all".into(), kind: "blink".into(),
            amp: None, cycles: Some(1.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(tl.target, TimelineTarget::SlotAttachment(ref s) if s == "all"));
        assert_eq!(tl.keys.first().unwrap().v[0], 0.0, "starts on base");
        assert_eq!(tl.keys.last().unwrap().v[0], 0.0, "ends on base (loops)");
        assert!(tl.keys.iter().any(|k| k.v[0] == 1.0), "shows the alternate");
        assert!(tl.keys.iter().all(|k| matches!(k.curve, Curve::Stepped)));
    }

    #[test]
    fn blink_without_alternates_falls_back_to_alpha_pulse() {
        let d = doc(); // "all" has no alternates
        let tl = render_move(&d, &MoveDraft {
            target: "all".into(), kind: "blink".into(),
            amp: None, cycles: Some(2.0), phase: None, color: None, to: None, at: None,
        }, 2.0).unwrap();
        assert!(matches!(tl.target, TimelineTarget::SlotAlpha(_)), "blink → pulse fallback");
    }

    #[test]
    fn plan_with_ripple_and_blink_emits_valid_spine() {
        let mut d = mesh_alt_doc();
        // The alternate needs a real part for emit (skin registration + timeline name).
        d.parts.push(crate::studio::doc::Part {
            id: "all_closed".into(), name: "all_closed".into(), prompts: vec![],
            bbox: Some(crate::studio::doc::Rect { x: 0, y: 0, w: 100, h: 100 }),
            mask_hash: None, completed_hash: None, completed_bbox: None,
            texture: Default::default(), deformable: false, attachment_only: true,
        });
        let plan = PlanDraft {
            duration: 2.0,
            looping: Some(true),
            moves: vec![
                MoveDraft { target: "all".into(), kind: "ripple".into(), amp: Some(6.0), cycles: Some(2.0), phase: None, color: None, to: None, at: None },
                MoveDraft { target: "all".into(), kind: "blink".into(), amp: None, cycles: Some(1.0), phase: None, color: None, to: None, at: None },
            ],
        };
        let clip = render_plan(&d, "idle", "idle", plan);
        assert_eq!(clip.timelines.len(), 2, "ripple + blink both rendered");
        d.clips = vec![clip];
        // The director's plan → a skeleton the real runtime accepts (deform + attachment).
        let v = crate::studio::spine42::emit(&d).unwrap();
        assert!(v["animations"]["idle"]["attachments"]["default"]["all"]["all"]["deform"].is_array());
        assert!(v["animations"]["idle"]["slots"]["all"]["attachment"].is_array());
    }
}
