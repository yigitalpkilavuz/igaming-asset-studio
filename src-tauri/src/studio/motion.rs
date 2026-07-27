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

/// Ease-name → curve, shared by the in-between path (the draft path uses `motion_gen`).
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

fn default_component(target: &TimelineTarget, _i: usize) -> f64 {
    match target {
        TimelineTarget::BoneScale(_) => 1.0,
        TimelineTarget::SlotAlpha(_) | TimelineTarget::SlotColor(_) => 1.0, // full / white
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
        TimelineTarget::SlotColor(_) => {
            for c in v.iter_mut() {
                *c = c.clamp(0.0, 1.0);
            }
        }
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

/// Draft a whole clip from a text brief. The model DIRECTS — it assigns a motion primitive
/// to each bone/slot — and `motion_gen` renders clean, seamlessly-looping keyframes. (LLMs
/// can't hand-key believable motion; assigning primitives is the reliable half.)
pub async fn draft_clip(
    api_key: &str,
    doc: &StudioDoc,
    name: &str,
    brief: &str,
) -> Result<Clip, String> {
    let user = format!(
        "Skeleton:\n{}\n\nClip name: {name}\nBrief: {brief}",
        skeleton_summary(doc)
    );
    let content = chat_json(api_key, super::motion_gen::PLAN_SYSTEM, &user).await?;
    let plan: super::motion_gen::PlanDraft =
        serde_json::from_str(&content).map_err(|e| format!("could not parse motion plan: {e}"))?;
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    let id = if slug.is_empty() { "clip".to_string() } else { slug };
    let clip = super::motion_gen::render_plan(doc, &id, name, plan);
    if clip.timelines.is_empty() {
        return Err("the model assigned no usable motion — try a more specific brief".into());
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
        TimelineTarget::SlotColor(s) => (s, "color"),
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

    #[test]
    fn inbetweens_insert_only_inside_interval_into_existing_tracks() {
        let d = doc();
        let base = Clip {
            id: "win".into(),
            name: "win".into(),
            duration: 2.0,
            looping: false,
            timelines: vec![Timeline {
                target: TimelineTarget::BoneRotate("head".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0], curve: Curve::Linear },
                    Key { time: 2.0, v: vec![30.0], curve: Curve::Linear },
                ],
            }],
        };
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

    /// End-to-end: real masks → heuristic rig → REAL OpenAI draft → engine → keyframes.
    /// Prints the directed plan and graphs each track's value curve (smoothness + loop
    /// closure). Env-gated; makes ONE paid call. Run:
    ///   WF_STUDIO=/…/studio WF_OUT=/…/out cargo test --lib \
    ///     studio::motion::tests::draft_end_to_end -- --nocapture
    #[test]
    fn draft_end_to_end() {
        let (Some(studio), Some(out)) =
            (std::env::var_os("WF_STUDIO"), std::env::var_os("WF_OUT"))
        else {
            return;
        };
        let studio = std::path::PathBuf::from(studio);
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let brief = std::env::var("WF_BRIEF").unwrap_or_else(|_| {
            "a gentle idle: the whole symbol breathes, the head bobs, limbs and any cape sway \
             softly, small bright parts glint"
                .into()
        });

        let mut doc: StudioDoc =
            serde_json::from_slice(&std::fs::read(studio.join("studio.json")).unwrap()).unwrap();
        // Real rig from the masks (heuristic tree — no AI needed for the rig itself).
        let mut masks = Vec::new();
        for p in &doc.parts {
            if let Ok(m) = image::open(studio.join("parts").join(&p.id).join("mask.png")) {
                masks.push((p.id.clone(), m.to_luma8()));
            }
        }
        let analysis = crate::studio::rig::analyze(&masks).unwrap();
        let tree = crate::studio::rig::heuristic_tree(&analysis);
        doc = crate::studio::rig::apply(doc, &analysis, &tree);
        if doc.slots.is_empty() {
            doc.slots = doc
                .parts
                .iter()
                .map(|p| crate::studio::doc::Slot {
                    name: p.id.clone(),
                    bone: p.id.clone(),
                    part_id: p.id.clone(),
                    attachment: Default::default(),
                    blend: Default::default(),
                })
                .collect();
        }

        let api_key = crate::secrets::get(crate::secrets::OPENAI_KEY)
            .unwrap()
            .filter(|k| !k.is_empty())
            .expect("no OpenAI key in the dev secret store");
        let clip = tauri::async_runtime::block_on(draft_clip(&api_key, &doc, "idle", &brief)).unwrap();

        eprintln!(
            "=== plan → clip \"{}\"  dur={:.2}s  loop={}  ({} tracks) ===",
            clip.name, clip.duration, clip.looping, clip.timelines.len()
        );
        for tl in &clip.timelines {
            let (t, ch) = target_parts(&tl.target);
            let first = tl.keys.first().map(|k| k.v.clone()).unwrap_or_default();
            let last = tl.keys.last().map(|k| k.v.clone()).unwrap_or_default();
            let closed = first.iter().zip(&last).all(|(a, b)| (a - b).abs() < 1e-6);
            eprintln!("  {t:>16} · {ch:<9} {:>2} keys  loop-closed={closed}", tl.keys.len());
        }

        // Graph each track's value curve — clean waves/ramps and start==end confirm quality.
        fn line(img: &mut image::RgbaImage, x0: f64, y0: f64, x1: f64, y1: f64, c: [u8; 3]) {
            let (w, h) = img.dimensions();
            let steps = ((x1 - x0).abs().max((y1 - y0).abs())).ceil() as i32 + 1;
            for i in 0..=steps {
                let f = i as f64 / steps.max(1) as f64;
                let (x, y) = ((x0 + (x1 - x0) * f) as i32, (y0 + (y1 - y0) * f) as i32);
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    img.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
        let rows = clip.timelines.len().max(1) as u32;
        let (w, rh) = (360u32, 64u32);
        let mut img = image::RgbaImage::from_pixel(w, rh * rows, image::Rgba([18, 19, 23, 255]));
        let n = 120usize;
        for (i, tl) in clip.timelines.iter().enumerate() {
            let y0 = i as u32 * rh;
            for x in 0..w {
                img.put_pixel(x, y0, image::Rgba([40, 41, 46, 255])); // row separator
            }
            let samples: Vec<Vec<f64>> =
                (0..=n).map(|s| value_at(tl, clip.duration * s as f64 / n as f64)).collect();
            for c in 0..tl.target.components() {
                let vals: Vec<f64> = samples.iter().map(|s| s[c]).collect();
                let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                let span = (hi - lo).max(1e-6);
                let col = [[230, 120, 120], [120, 200, 230], [130, 220, 150]][c % 3];
                let mut prev: Option<(f64, f64)> = None;
                for (s, &v) in vals.iter().enumerate() {
                    let x = s as f64 / n as f64 * (w - 1) as f64;
                    let y = y0 as f64 + rh as f64 - 6.0 - (v - lo) / span * (rh as f64 - 12.0);
                    if let Some((px, py)) = prev {
                        line(&mut img, px, py, x, y, col);
                    }
                    prev = Some((x, y));
                }
            }
        }
        image::DynamicImage::ImageRgba8(img).save(out.join("motion_curves.png")).unwrap();
        eprintln!("wrote {}/motion_curves.png", out.display());
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
