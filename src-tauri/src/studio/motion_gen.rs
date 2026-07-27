//! Procedural motion engine. The cloud model DIRECTS — it assigns a motion PRIMITIVE
//! (bob, sway, breathe, spin, shake, pulse, glint…) with a few params to each moving bone
//! or slot — and this engine renders clean, seamlessly-looping, eased keyframes. LLMs can't
//! hand-key believable motion, but they reliably map a brief → "head bobs, cape sways, gem
//! glints" (the same SAM-regions → LLM-labels split that fixed cutting). All motion here is
//! smooth and loop-closed by construction, so the output is production-usable, not a
//! best-effort pile of guessed numbers.

use std::f64::consts::PI;

use serde::Deserialize;

use super::doc::{Clip, Curve, Key, StudioDoc, Timeline, TimelineTarget, EASE_IN_OUT};

/// The system prompt: the model returns a PLAN (primitive assignments), never keyframes.
pub const PLAN_SYSTEM: &str = "\
You are directing a looping animation for a premium slot-game symbol. You do NOT write \
keyframes — you assign ONE motion PRIMITIVE to each bone or slot that should move, and the \
engine generates smooth, seamlessly-looping keyframes. Be tasteful: a slot symbol is subtle, \
not a cartoon. Primitives:\n\
BONE: bob (gentle vertical float, amp = pixels) · sway (slow rotate rock, amp = degrees) · \
swing (bigger limb/cape/tail rotate, amp = degrees) · breathe (torso/root scale pulse, amp = \
fraction like 0.03) · spin (continuous rotation, amp = number of turns) · shake (a decaying \
impact jitter, amp = pixels).\n\
SLOT: pulse (alpha throb, amp = peak 0..1) · glint (colour shimmer — tints toward `color` hex \
and back, for gems/metal/glass).\n\
Params: amp (see above), cycles (oscillations over the clip — keep whole for a clean loop; \
1–3 for idle, more for energetic), phase (0..6.28 — give bones DEEPER in the chain a larger \
phase so motion flows outward as follow-through), color (hex, glint only).\n\
Use the brief to decide WHICH parts move and how strongly. Prefer 2–6 moves; err subtle. \
Set duration (seconds) and loop=true for idles.\n\
Output ONLY this JSON object: {\"duration\": number, \"loop\": boolean, \"moves\": [{\"target\": \
\"boneOrSlotName\", \"kind\": \"bob|sway|swing|breathe|spin|shake|pulse|glint\", \"amp\": \
number, \"cycles\": number, \"phase\": number, \"color\": \"rrggbb\"}]}";

#[derive(Debug, Deserialize)]
pub struct PlanDraft {
    #[serde(default)]
    pub duration: f64,
    #[serde(default, alias = "loop")]
    pub looping: Option<bool>,
    #[serde(default)]
    pub moves: Vec<MoveDraft>,
}

#[derive(Debug, Deserialize)]
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
}

fn ease() -> Curve {
    Curve::Bezier(EASE_IN_OUT.to_vec())
}
fn ease_n(comps: usize) -> Curve {
    Curve::Bezier(EASE_IN_OUT.iter().cycle().take(comps * 4).copied().collect())
}

/// One eased sine oscillation of amplitude `amp` about `base`. `cycles` is rounded to whole
/// periods so the last key equals the first — a seamless loop. Single-component keys.
fn wave(base: f64, amp: f64, cycles: f64, phase: f64, duration: f64) -> Vec<Key> {
    let cyc = cycles.round().max(1.0);
    let n = (cyc as usize) * 4;
    (0..=n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let v = base + amp * (2.0 * PI * cyc * f + phase).sin();
            Key { time: duration * f, v: vec![v], curve: ease() }
        })
        .collect()
}

fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|b| b as f64 / 255.0);
    Some([ch(0)?, ch(2)?, ch(4)?])
}

/// Render one directed move into a timeline, or `None` if the target/kind don't fit
/// (e.g. a bone primitive on a slot name). Amps are clamped to production-safe ranges.
pub fn render_move(doc: &StudioDoc, m: &MoveDraft, duration: f64) -> Option<Timeline> {
    let t = m.target.trim();
    let is_bone = doc.bones.iter().any(|b| b.name == t);
    let is_slot = doc.slots.iter().any(|s| s.name == t);
    let cycles = m.cycles.unwrap_or(1.0);
    let phase = m.phase.unwrap_or(0.0);
    let max_dim = doc.source.width.max(doc.source.height) as f64;

    let (target, keys) = match m.kind.trim().to_lowercase().as_str() {
        "bob" | "hover" | "float" | "bounce" if is_bone => {
            let amp = m.amp.unwrap_or(6.0).clamp(1.0, max_dim * 0.12);
            let keys = wave(0.0, amp, cycles, phase, duration)
                .into_iter()
                .map(|k| Key { v: vec![0.0, k.v[0]], ..k }) // translate: y only
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
                .map(|k| Key { v: vec![k.v[0], k.v[0]], ..k }) // uniform scale
                .collect();
            (TimelineTarget::BoneScale(t.to_string()), keys)
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
        _ => return None,
    };
    Some(Timeline { target, keys })
}

/// Render a full directed plan into a clip, dropping unknown targets and duplicate channels.
pub fn render_plan(doc: &StudioDoc, id: &str, name: &str, plan: PlanDraft) -> Clip {
    let duration = if plan.duration > 0.0 { plan.duration.clamp(0.4, 12.0) } else { 2.0 };
    let looping = plan.looping.unwrap_or(true);
    let mut timelines: Vec<Timeline> = Vec::new();
    for m in plan.moves.iter().take(24) {
        let Some(tl) = render_move(doc, m, duration) else { continue };
        if tl.keys.is_empty() || timelines.iter().any(|x| x.target == tl.target) {
            continue;
        }
        timelines.push(tl);
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
    fn wave_is_loop_closed_and_eased() {
        let k = wave(0.0, 10.0, 2.0, 0.0, 2.0);
        assert!((k.first().unwrap().v[0] - k.last().unwrap().v[0]).abs() < 1e-9, "start == end");
        assert!((k.first().unwrap().time - 0.0).abs() < 1e-9);
        assert!((k.last().unwrap().time - 2.0).abs() < 1e-9);
        assert!(matches!(k[0].curve, Curve::Bezier(_)), "eased, not linear");
        // Peaks reach the amplitude.
        assert!(k.iter().any(|key| (key.v[0] - 10.0).abs() < 1e-6));
    }

    #[test]
    fn primitives_pick_the_right_channel_and_target() {
        let doc = doc(); // seed → bone "body", slot "all"
        let sway = render_move(&doc, &MoveDraft {
            target: "body".into(), kind: "sway".into(),
            amp: Some(6.0), cycles: Some(2.0), phase: None, color: None,
        }, 2.0).unwrap();
        assert!(matches!(sway.target, TimelineTarget::BoneRotate(ref b) if b == "body"));

        let bob = render_move(&doc, &MoveDraft {
            target: "body".into(), kind: "bob".into(),
            amp: Some(8.0), cycles: Some(1.0), phase: None, color: None,
        }, 2.0).unwrap();
        assert!(matches!(bob.target, TimelineTarget::BoneTranslate(_)));
        assert!(bob.keys.iter().all(|k| k.v.len() == 2 && k.v[0] == 0.0)); // y-only

        let glint = render_move(&doc, &MoveDraft {
            target: "all".into(), kind: "glint".into(),
            amp: None, cycles: Some(2.0), phase: None, color: Some("ffcc00".into()),
        }, 2.0).unwrap();
        assert!(matches!(glint.target, TimelineTarget::SlotColor(_)));
        assert!(glint.keys.iter().all(|k| k.v.len() == 3));

        // A bone primitive on a slot name → dropped.
        assert!(render_move(&doc, &MoveDraft {
            target: "all".into(), kind: "sway".into(),
            amp: None, cycles: None, phase: None, color: None,
        }, 2.0).is_none());
    }

    #[test]
    fn plan_dedupes_and_bounds() {
        let doc = doc();
        let plan = PlanDraft {
            duration: 0.0, // → default 2.0
            looping: Some(true),
            moves: vec![
                MoveDraft { target: "body".into(), kind: "sway".into(), amp: Some(500.0), cycles: Some(1.0), phase: None, color: None },
                MoveDraft { target: "body".into(), kind: "sway".into(), amp: Some(3.0), cycles: Some(1.0), phase: None, color: None }, // dup channel
                MoveDraft { target: "ghost".into(), kind: "bob".into(), amp: None, cycles: None, phase: None, color: None }, // unknown
            ],
        };
        let clip = render_plan(&doc, "idle", "idle", plan);
        assert_eq!(clip.duration, 2.0);
        assert_eq!(clip.timelines.len(), 1, "dup channel + unknown target dropped");
        // Amp clamped: 500° → ≤30°.
        assert!(clip.timelines[0].keys.iter().all(|k| k.v[0].abs() <= 30.0 + 1e-9));
    }
}
