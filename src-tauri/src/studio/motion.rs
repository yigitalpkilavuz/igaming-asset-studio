//! AI motion: text → draft keyframe clip, and AI in-betweens between two poses.
//! Follows the `assistant.rs` pattern exactly — gpt-4o in JSON mode returns a lenient
//! draft, and a deterministic clamp pipeline (`apply_clip_draft`) makes it safe: unknown
//! targets dropped, values bounded, times sorted into range, loop closure forced.
//! The prompt teaches a motion VOCABULARY in words (breathe, sway, pulse…) — the model
//! composes; nothing is hardcoded.

use serde::Deserialize;
use serde_json::json;

use super::doc::{Clip, Curve, Key, StudioDoc, Timeline, TimelineTarget};

const CHAT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const MODEL: &str = "gpt-4o";

// ── Skeleton summary (deterministic, compact) ─────────────────────────────────

fn skeleton_summary(doc: &StudioDoc) -> serde_json::Value {
    let depth = |name: &str| {
        let mut d = 0;
        let mut cur = name.to_string();
        while let Some(b) = doc.bone(&cur) {
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
    };
    json!({
        "canvas": { "width": doc.source.width, "height": doc.source.height },
        "fps": doc.settings.fps,
        "bones": doc.bones.iter().map(|b| json!({
            "name": b.name,
            "parent": b.parent,
            "length": b.length.round(),
            "depth": depth(&b.name),
        })).collect::<Vec<_>>(),
        "slots": doc.slots.iter().map(|s| &s.name).collect::<Vec<_>>(),
    })
}

const CHANNEL_SEMANTICS: &str = "\
Channels and value semantics (per key, `value` is an array):\n\
- rotate: [degrees] OFFSET from the rest pose, positive = counter-clockwise. Target = bone.\n\
- translate: [x, y] pixel offset from rest; x right, y DOWN. Target = bone.\n\
- scale: [x, y] multiplier of the rest scale (1 = rest). Target = bone.\n\
- alpha: [0..1] slot opacity. Target = slot.\n\
`ease` is the ease OUT of a key toward the next: linear | in | out | inOut | stepped.";

const VOCABULARY: &str = "\
Motion vocabulary to compose from (subtle by default — this is a slot symbol, not a cartoon):\n\
breathe = slow torso/root scale 1.00→1.03 with inOut; sway = gentle rotate ±2–4° on a core \
bone; tilt = head rotate ±5–10°; pulse = quick scale pop 1→1.05→1; bounce = translate y dip \
with a slight squash (scale x up, y down) at the bottom; hover = slow translate y ±4–8; \
swing = limb rotate ±8–25° hinging at its joint; shiver = fast small alternating rotates; \
glow = slot alpha 0.65→1; flare = scale + alpha surge together; recoil = small opposite \
motion before the main action (anticipation).";

const PRINCIPLES: &str = "\
Animation principles: ease in/out by default; arcs, anticipation and follow-through; \
children lag their parents by 1–3 frames (offset key times slightly down the chain); \
overshoot slightly then settle on dramatic actions; keep loops seamless (end pose = start \
pose). Typical budget: 2–6 tracks, 3–5 keys per track.";

// ── Draft contract ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ClipDraft {
    #[serde(default)]
    duration: f64,
    #[serde(default, alias = "loop")]
    looping: Option<bool>,
    #[serde(default)]
    timelines: Vec<TrackDraft>,
}
#[derive(Deserialize)]
struct TrackDraft {
    #[serde(default)]
    target: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    keys: Vec<KeyDraft>,
}
#[derive(Deserialize)]
struct KeyDraft {
    #[serde(default)]
    time: f64,
    #[serde(default)]
    value: Vec<f64>,
    #[serde(default)]
    ease: String,
}

fn ease_to_curve(ease: &str, comps: usize) -> Curve {
    let h: Option<[f64; 4]> = match ease {
        "in" => Some([0.42, 0.0, 1.0, 1.0]),
        "out" => Some([0.0, 0.0, 0.58, 1.0]),
        "inOut" | "in_out" | "ease" => Some([0.42, 0.0, 0.58, 1.0]),
        "stepped" => return Curve::Stepped,
        _ => None, // linear
    };
    match h {
        Some(h) => Curve::Bezier(h.iter().cycle().take(4 * comps).copied().collect()),
        None => Curve::Linear,
    }
}

/// Validate/clamp a raw model draft into a safe [`Clip`].
fn apply_clip_draft(
    doc: &StudioDoc,
    id: &str,
    name: &str,
    draft: ClipDraftInput,
) -> Clip {
    let duration = if draft.duration > 0.0 { draft.duration.clamp(0.3, 10.0) } else { 2.0 };
    let looping = draft.looping;
    let max_dim = doc.source.width.max(doc.source.height) as f64;
    let translate_cap = max_dim * 0.15;

    let bone_ok = |n: &str| doc.bones.iter().any(|b| b.name == n);
    let slot_ok = |n: &str| doc.slots.iter().any(|s| s.name == n);

    let mut timelines: Vec<Timeline> = Vec::new();
    for track in draft.timelines.into_iter().take(24) {
        let target = match track.channel.as_str() {
            "rotate" if bone_ok(&track.target) => TimelineTarget::BoneRotate(track.target.clone()),
            "translate" if bone_ok(&track.target) => {
                TimelineTarget::BoneTranslate(track.target.clone())
            }
            "scale" if bone_ok(&track.target) => TimelineTarget::BoneScale(track.target.clone()),
            "alpha" if slot_ok(&track.target) => TimelineTarget::SlotAlpha(track.target.clone()),
            _ => continue, // unknown target/channel — dropped
        };
        // Don't duplicate a track that already made it in.
        if timelines
            .iter()
            .any(|tl| tl.target == target)
        {
            continue;
        }
        let comps = target.components();
        let mut keys: Vec<Key> = Vec::new();
        for k in track.keys.into_iter().take(12) {
            let time = (k.time).clamp(0.0, duration);
            let mut v: Vec<f64> = (0..comps)
                .map(|i| k.value.get(i).copied().unwrap_or_else(|| default_component(&target, i)))
                .collect();
            clamp_values(&target, &mut v, translate_cap);
            let curve = ease_to_curve(k.ease.as_str(), comps);
            // Dedupe on identical (snapped-ish) times: last write wins.
            if let Some(existing) = keys.iter_mut().find(|e| (e.time - time).abs() < 1e-3) {
                existing.v = v;
                existing.curve = curve;
            } else {
                keys.push(Key { time, v, curve });
            }
        }
        if keys.is_empty() {
            continue;
        }
        keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
        if looping {
            close_loop(&mut keys, duration);
        }
        timelines.push(Timeline { target, keys });
    }

    Clip { id: id.to_string(), name: name.to_string(), duration, looping, timelines }
}

/// A normalized draft input (what both the AI path and tests feed the clamp).
struct ClipDraftInput {
    duration: f64,
    looping: bool,
    timelines: Vec<TrackDraft>,
}

fn default_component(target: &TimelineTarget, _i: usize) -> f64 {
    match target {
        TimelineTarget::BoneScale(_) => 1.0,
        TimelineTarget::SlotAlpha(_) => 1.0,
        _ => 0.0,
    }
}

fn clamp_values(target: &TimelineTarget, v: &mut [f64], translate_cap: f64) {
    match target {
        TimelineTarget::BoneRotate(_) => {
            v[0] = v[0].clamp(-60.0, 60.0);
        }
        TimelineTarget::BoneTranslate(_) => {
            for c in v.iter_mut() {
                *c = c.clamp(-translate_cap, translate_cap);
            }
        }
        TimelineTarget::BoneScale(_) => {
            for c in v.iter_mut() {
                *c = c.clamp(0.7, 1.4);
            }
        }
        TimelineTarget::SlotAlpha(_) => {
            v[0] = v[0].clamp(0.0, 1.0);
        }
    }
}

/// Seamless loop: the track's final pose must equal its first.
fn close_loop(keys: &mut Vec<Key>, duration: f64) {
    let Some(first) = keys.first().cloned() else { return };
    match keys.last_mut() {
        Some(last) if (last.time - duration).abs() < 1e-3 => {
            last.v = first.v;
        }
        _ => keys.push(Key { time: duration, v: first.v, curve: Curve::Linear }),
    }
}

// ── OpenAI plumbing ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: Msg,
}
#[derive(Deserialize)]
struct Msg {
    content: String,
}

async fn chat_json(api_key: &str, system: &str, user: &str) -> Result<String, String> {
    let body = json!({
        "model": MODEL,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": 0.7,
        "response_format": { "type": "json_object" },
    });
    let resp = reqwest::Client::new()
        .post(CHAT_ENDPOINT)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("motion request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        let trunc: String = text.chars().take(300).collect();
        return Err(format!("OpenAI error {status}: {trunc}"));
    }
    let parsed: ChatResp = serde_json::from_str(&text).map_err(|e| format!("bad response: {e}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "no choices".to_string())
}

/// Draft a whole clip from a text brief.
pub async fn draft_clip(
    api_key: &str,
    doc: &StudioDoc,
    name: &str,
    brief: &str,
) -> Result<Clip, String> {
    let system = format!(
        "You are a senior 2D character animator keyframing a Spine skeleton for a premium \
dark-fantasy slot game symbol. You receive the skeleton and a motion brief; you output a \
keyframe clip as JSON.\n\n{CHANNEL_SEMANTICS}\n\n{VOCABULARY}\n\n{PRINCIPLES}\n\n\
Output ONLY this JSON object:\n\
{{\"duration\": seconds, \"loop\": boolean, \"timelines\": [{{\"target\": \"boneOrSlotName\", \
\"channel\": \"rotate\"|\"translate\"|\"scale\"|\"alpha\", \"keys\": [{{\"time\": seconds, \
\"value\": [numbers], \"ease\": \"linear\"|\"in\"|\"out\"|\"inOut\"|\"stepped\"}}]}}]}}"
    );
    let user = format!(
        "Skeleton:\n{}\n\nClip name: {name}\nBrief: {brief}",
        skeleton_summary(doc)
    );
    let content = chat_json(api_key, &system, &user).await?;
    let draft: ClipDraft =
        serde_json::from_str(&content).map_err(|e| format!("could not parse motion draft: {e}"))?;
    let looping = draft.looping.unwrap_or(true);
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    let id = if slug.is_empty() { "clip".to_string() } else { slug };
    let clip = apply_clip_draft(
        doc,
        &id,
        name,
        ClipDraftInput { duration: draft.duration, looping, timelines: draft.timelines },
    );
    if clip.timelines.is_empty() {
        return Err("the model produced no usable tracks — try rephrasing the brief".into());
    }
    Ok(clip)
}

// ── In-betweens ────────────────────────────────────────────────────────────────

/// Linear evaluation of a timeline at time t (pose sampling for the prompt).
fn value_at(tl: &Timeline, t: f64) -> Vec<f64> {
    let keys = &tl.keys;
    if keys.is_empty() {
        return vec![default_component(&tl.target, 0); tl.target.components()];
    }
    if t <= keys[0].time {
        return keys[0].v.clone();
    }
    if t >= keys[keys.len() - 1].time {
        return keys[keys.len() - 1].v.clone();
    }
    for w in keys.windows(2) {
        if t >= w[0].time && t <= w[1].time {
            let f = if w[1].time > w[0].time { (t - w[0].time) / (w[1].time - w[0].time) } else { 0.0 };
            return w[0]
                .v
                .iter()
                .zip(&w[1].v)
                .map(|(a, b)| a + (b - a) * f)
                .collect();
        }
    }
    keys[0].v.clone()
}

fn target_parts(t: &TimelineTarget) -> (&str, &'static str) {
    match t {
        TimelineTarget::BoneRotate(b) => (b, "rotate"),
        TimelineTarget::BoneTranslate(b) => (b, "translate"),
        TimelineTarget::BoneScale(b) => (b, "scale"),
        TimelineTarget::SlotAlpha(s) => (s, "alpha"),
    }
}

#[derive(Deserialize)]
struct InbetweenDraft {
    #[serde(default)]
    keys: Vec<InbetweenKey>,
}
#[derive(Deserialize)]
struct InbetweenKey {
    #[serde(default)]
    target: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    time: f64,
    #[serde(default)]
    value: Vec<f64>,
    #[serde(default)]
    ease: String,
}

/// Insert AI breakdown keys strictly between two times of an existing clip.
/// Returns the updated clip. Pure clamp logic is in [`apply_inbetweens`].
pub async fn inbetween(
    api_key: &str,
    doc: &StudioDoc,
    clip: &Clip,
    from: f64,
    to: f64,
    count: u32,
) -> Result<Clip, String> {
    if to - from < 0.05 {
        return Err("the interval is too short for in-betweens".into());
    }
    let poses: Vec<serde_json::Value> = clip
        .timelines
        .iter()
        .map(|tl| {
            let (target, channel) = target_parts(&tl.target);
            json!({
                "target": target,
                "channel": channel,
                "poseA": value_at(tl, from),
                "poseB": value_at(tl, to),
            })
        })
        .collect();
    let system = format!(
        "You are a senior 2D animator adding BREAKDOWN keyframes between two poses of a Spine \
clip. Do NOT merely interpolate linearly — add natural arcs, anticipation, overshoot and \
follow-through appropriate to the motion. Children lag parents slightly.\n\n\
{CHANNEL_SEMANTICS}\n\nOutput ONLY: {{\"keys\": [{{\"target\": string, \"channel\": string, \
\"time\": seconds, \"value\": [numbers], \"ease\": \"linear\"|\"in\"|\"out\"|\"inOut\"}}]}}.\n\
Every key time must be STRICTLY between the two pose times. Add about {count} keys per \
track where they help; skip tracks that should interpolate plainly."
    );
    let user = format!(
        "Pose A at t={from:.3}s, pose B at t={to:.3}s. Tracks:\n{}",
        serde_json::Value::Array(poses)
    );
    let content = chat_json(api_key, &system, &user).await?;
    let draft: InbetweenDraft =
        serde_json::from_str(&content).map_err(|e| format!("could not parse in-betweens: {e}"))?;
    Ok(apply_inbetweens(doc, clip, from, to, draft.keys))
}

/// Clamp + insert in-between keys into a copy of the clip.
fn apply_inbetweens(
    doc: &StudioDoc,
    clip: &Clip,
    from: f64,
    to: f64,
    keys: Vec<InbetweenKey>,
) -> Clip {
    let mut out = clip.clone();
    let max_dim = doc.source.width.max(doc.source.height) as f64;
    let cap = max_dim * 0.15;
    let eps = (to - from) * 0.02;

    for k in keys.into_iter().take(48) {
        // Only into EXISTING tracks (in-betweens refine, they don't invent structure).
        let Some(tl) = out.timelines.iter_mut().find(|tl| {
            let (target, channel) = target_parts(&tl.target);
            target == k.target && channel == k.channel
        }) else {
            continue;
        };
        let time = k.time.clamp(from + eps, to - eps);
        let comps = tl.target.components();
        let mut v: Vec<f64> = (0..comps)
            .map(|i| k.value.get(i).copied().unwrap_or_else(|| default_component(&tl.target, i)))
            .collect();
        clamp_values(&tl.target, &mut v, cap);
        let curve = ease_to_curve(k.ease.as_str(), comps);
        if let Some(existing) = tl.keys.iter_mut().find(|e| (e.time - time).abs() < 1e-3) {
            existing.v = v;
            existing.curve = curve;
        } else {
            tl.keys.push(Key { time, v, curve });
            tl.keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::SourceRef;

    fn doc() -> StudioDoc {
        let mut d = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 1000, height: 800, sha256: "s".into() },
            0.0,
        );
        d.bones.push(crate::studio::doc::Bone::new("head", Some("body".into()), 500.0, 200.0));
        d
    }

    fn track(target: &str, channel: &str, keys: Vec<(f64, Vec<f64>, &str)>) -> TrackDraft {
        TrackDraft {
            target: target.into(),
            channel: channel.into(),
            keys: keys
                .into_iter()
                .map(|(time, value, ease)| KeyDraft { time, value, ease: ease.into() })
                .collect(),
        }
    }

    #[test]
    fn clamp_drops_unknown_targets_and_bounds_values() {
        let d = doc();
        let clip = apply_clip_draft(
            &d,
            "win",
            "win",
            ClipDraftInput {
                duration: 3.0,
                looping: false,
                timelines: vec![
                    track("head", "rotate", vec![(0.0, vec![0.0], "inOut"), (1.5, vec![900.0], "out"), (99.0, vec![-10.0], "linear")]),
                    track("ghost_bone", "rotate", vec![(0.0, vec![5.0], "linear")]),
                    track("all", "alpha", vec![(0.0, vec![2.5], "linear")]),
                    track("head", "scale", vec![(0.0, vec![5.0, 0.01], "in")]),
                ],
            },
        );
        assert_eq!(clip.timelines.len(), 3); // ghost_bone dropped
        let rot = &clip.timelines[0];
        assert!(matches!(&rot.target, TimelineTarget::BoneRotate(b) if b == "head"));
        assert_eq!(rot.keys[1].v[0], 60.0); // ±60 clamp
        assert_eq!(rot.keys[2].time, 3.0); // time clamped to duration
        let alpha = &clip.timelines[1];
        assert_eq!(alpha.keys[0].v[0], 1.0); // alpha 0..1
        let scale = &clip.timelines[2];
        assert_eq!(scale.keys[0].v, vec![1.4, 0.7]); // scale clamp
        // Ease mapping produced bezier out-curves where a next key exists.
        assert!(matches!(rot.keys[0].curve, Curve::Bezier(_)));
    }

    #[test]
    fn loop_closure_appends_or_overwrites_final_key() {
        let d = doc();
        let clip = apply_clip_draft(
            &d,
            "idle",
            "idle",
            ClipDraftInput {
                duration: 2.0,
                looping: true,
                timelines: vec![
                    track("head", "rotate", vec![(0.0, vec![3.0], "inOut"), (1.0, vec![-3.0], "inOut")]),
                    track("body", "scale", vec![(0.0, vec![1.0, 1.0], "inOut"), (2.0, vec![1.2, 1.2], "linear")]),
                ],
            },
        );
        // Track 1: last key appended at duration with the first value.
        let rot = &clip.timelines[0];
        assert_eq!(rot.keys.len(), 3);
        assert_eq!(rot.keys[2].time, 2.0);
        assert_eq!(rot.keys[2].v, vec![3.0]);
        // Track 2: existing key AT duration gets overwritten to the first value.
        let sc = &clip.timelines[1];
        assert_eq!(sc.keys.len(), 2);
        assert_eq!(sc.keys[1].v, vec![1.0, 1.0]);
    }

    #[test]
    fn translate_cap_scales_with_canvas() {
        let d = doc(); // max dim 1000 → cap 150
        let clip = apply_clip_draft(
            &d,
            "c",
            "c",
            ClipDraftInput {
                duration: 1.0,
                looping: false,
                timelines: vec![track("head", "translate", vec![(0.0, vec![9999.0, -9999.0], "linear")])],
            },
        );
        assert_eq!(clip.timelines[0].keys[0].v, vec![150.0, -150.0]);
    }

    #[test]
    fn inbetweens_insert_only_inside_interval_into_existing_tracks() {
        let d = doc();
        let base = apply_clip_draft(
            &d,
            "win",
            "win",
            ClipDraftInput {
                duration: 2.0,
                looping: false,
                timelines: vec![track("head", "rotate", vec![(0.0, vec![0.0], "inOut"), (2.0, vec![30.0], "linear")])],
            },
        );
        let updated = apply_inbetweens(
            &d,
            &base,
            0.0,
            2.0,
            vec![
                InbetweenKey { target: "head".into(), channel: "rotate".into(), time: -5.0, value: vec![-8.0], ease: "out".into() },
                InbetweenKey { target: "head".into(), channel: "rotate".into(), time: 1.0, value: vec![40.0], ease: "inOut".into() },
                InbetweenKey { target: "nope".into(), channel: "rotate".into(), time: 1.0, value: vec![1.0], ease: "linear".into() },
            ],
        );
        let keys = &updated.timelines[0].keys;
        assert_eq!(keys.len(), 4); // 2 originals + 2 inserted (unknown track dropped)
        // The out-of-range time got clamped INSIDE the interval, not to its edge value 0.
        assert!(keys[1].time > 0.0 && keys[1].time < keys[2].time);
        assert_eq!(keys[2].v, vec![40.0]);
        // Original endpoints untouched.
        assert_eq!(keys[0].time, 0.0);
        assert_eq!(keys[3].v, vec![30.0]);
    }

    #[test]
    fn value_at_interpolates_linearly() {
        let tl = Timeline {
            target: TimelineTarget::BoneTranslate("b".into()),
            keys: vec![
                Key { time: 0.0, v: vec![0.0, 0.0], curve: Curve::Linear },
                Key { time: 2.0, v: vec![10.0, -20.0], curve: Curve::Linear },
            ],
        };
        assert_eq!(value_at(&tl, 1.0), vec![5.0, -10.0]);
        assert_eq!(value_at(&tl, -1.0), vec![0.0, 0.0]);
        assert_eq!(value_at(&tl, 9.0), vec![10.0, -20.0]);
    }
}
