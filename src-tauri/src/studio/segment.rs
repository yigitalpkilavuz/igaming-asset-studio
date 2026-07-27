//! Segmentation: gpt-4o vision proposes named parts with SAM seed points; masks are
//! refined deterministically (threshold ∩ source alpha, keep prompted components, close);
//! `cut_parts` lifts ORIGINAL pixels by mask with z-order exclusive claiming — the
//! anti-drift core: no pixel is ever redrawn, parts reassemble into the exact source.

use std::collections::HashMap;
use std::path::Path;

use image::GrayImage;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::doc::{Bone, Rect, SamLabel, SamPrompt, Slot, StudioDoc};
use super::{matte, sha256_hex, store};

// ── AI part proposals (gpt-4o vision) ──────────────────────────────────────────

const CHAT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// Vision model for proposals/labeling — settable from AppSettings at command entry.
static VISION_MODEL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub(crate) fn set_vision_model(model: &str) {
    if !model.trim().is_empty() {
        *VISION_MODEL.lock().unwrap() = Some(model.trim().to_string());
    }
}

fn vision_model() -> String {
    VISION_MODEL
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "gpt-4o".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PartProposal {
    /// Slug id (also bone/slot/region name), e.g. `head`, `left_arm`.
    pub id: String,
    pub name: String,
    /// SAM seed prompts, normalized 0..1.
    pub points: Vec<SamPrompt>,
}

const PROPOSE_SYSTEM: &str = "\
You are planning a cutout for 2D skeletal animation of ONE slot-game symbol or character \
image. Split it into the FEWEST separately-moving parts that allow EXPRESSIVE motion — \
err toward more parts when the art has visible ornament, because anything not cut apart \
can never move on its own. A full-body mascot character typically splits into: head, \
torso, each arm, each leg, plus props/cape/tail as their own pieces. An object or emblem \
symbol (a mask, chalice, medallion, weapon…) splits into its main body PLUS every ornament \
that could plausibly move independently: each plume/feather cluster, ribbons, dangling \
charms or chains, wings, a centerpiece gem that could glint — typically 4 to 7 parts. \
Only 2 parts is right when the subject truly has no independent ornament. Rules:\n\
- 2 to 10 parts, ONLY parts actually visible. Background-most part FIRST, foreground-most LAST.\n\
- Each part: a short lowercase slug id (a-z, 0-9, underscore), a display name, and 1-4 `points` \
placed WELL INSIDE the part's visible pixels (centred on its mass, away from edges), plus \
optionally 1-2 negative points just outside it on a neighbouring part it must not include.\n\
- Points are normalized 0..1 (x right, y down). label is \"positive\" or \"negative\".\n\
Output ONLY JSON: {\"parts\":[{\"id\":string,\"name\":string,\"points\":[{\"x\":number,\"y\":number,\
\"label\":\"positive\"|\"negative\"}]}]}";

#[derive(Serialize)]
struct VisionReq<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
    temperature: f32,
    response_format: serde_json::Value,
}
#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: serde_json::Value,
}
#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: RespMsg,
}
#[derive(Deserialize)]
struct RespMsg {
    content: String,
}

#[derive(Deserialize)]
struct ProposalDraft {
    #[serde(default)]
    parts: Vec<PartDraft>,
}
#[derive(Deserialize)]
struct PartDraft {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    points: Vec<PointDraft>,
}
#[derive(Deserialize)]
struct PointDraft {
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    label: String,
}

/// One gpt-4o vision round-trip: system + (text, image) → the model's JSON string.
/// Shared by the studio part proposer and the layers depth-band proposer.
pub(crate) async fn vision_json(
    api_key: &str,
    system: &str,
    user_text: &str,
    source_png: &[u8],
) -> Result<String, String> {
    vision_json_multi(api_key, system, user_text, &[source_png]).await
}

/// Like [`vision_json`] but with several image attachments (e.g. original + overlay).
pub(crate) async fn vision_json_multi(
    api_key: &str,
    system: &str,
    user_text: &str,
    images: &[&[u8]],
) -> Result<String, String> {
    use base64::Engine;
    let mut content_parts = vec![serde_json::json!({ "type": "text", "text": user_text })];
    for img in images {
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(img)
        );
        content_parts.push(serde_json::json!({
            "type": "image_url", "image_url": { "url": data_url }
        }));
    }
    let model = vision_model();
    let body = VisionReq {
        model: &model,
        messages: vec![
            Msg { role: "system", content: serde_json::json!(system) },
            Msg { role: "user", content: serde_json::Value::Array(content_parts) },
        ],
        temperature: 0.3,
        response_format: serde_json::json!({ "type": "json_object" }),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(CHAT_ENDPOINT)
        .bearer_auth(api_key)
        .json(&body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("vision request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        let trunc: String = text.chars().take(400).collect();
        return Err(format!("OpenAI vision error {status}: {trunc}"));
    }
    let parsed: ChatResp =
        serde_json::from_str(&text).map_err(|e| format!("bad vision response: {e}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "vision returned no choices".to_string())
}

/// Ask gpt-4o vision for a part plan. `hint` carries art direction (game brief/subject).
pub async fn propose_parts(
    api_key: &str,
    source_png: &[u8],
    hint: &str,
) -> Result<Vec<PartProposal>, String> {
    let user_text = if hint.trim().is_empty() {
        "Plan the animation cutout parts for this symbol.".to_string()
    } else {
        format!("Context: {hint}\nPlan the animation cutout parts for this symbol.")
    };
    let content = vision_json(api_key, PROPOSE_SYSTEM, &user_text, source_png).await?;
    let draft: ProposalDraft = serde_json::from_str(&content)
        .map_err(|e| format!("could not parse part plan: {e}"))?;
    Ok(clamp_proposals(draft))
}

/// Validate/clamp a raw model draft into safe proposals (mirrors `assistant::apply_draft`).
fn clamp_proposals(draft: ProposalDraft) -> Vec<PartProposal> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for p in draft.parts {
        let slug = slugify(if p.id.trim().is_empty() { &p.name } else { &p.id });
        if slug.is_empty() || slug == "root" || slug == "all" {
            continue;
        }
        let mut id = slug.clone();
        let mut i = 2;
        while seen.contains(&id) {
            id = format!("{slug}_{i}");
            i += 1;
        }
        let points: Vec<SamPrompt> = p
            .points
            .iter()
            .take(6)
            .map(|pt| SamPrompt {
                x: (pt.x as f64).clamp(0.0, 1.0),
                y: (pt.y as f64).clamp(0.0, 1.0),
                label: if pt.label == "negative" { SamLabel::Negative } else { SamLabel::Positive },
            })
            .collect();
        if !points.iter().any(|pt| pt.label == SamLabel::Positive) {
            continue; // a part we can't seed is useless
        }
        let name = if p.name.trim().is_empty() { id.clone() } else { p.name.trim().to_string() };
        seen.push(id.clone());
        out.push(PartProposal { id, name, points });
        if out.len() >= 10 {
            break;
        }
    }
    out
}

pub(crate) fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for ch in s.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_us = false;
        } else if !prev_us && !out.is_empty() {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

// Mask refinement lives in `matte::refine` (the studio passes `hole_cap_px: None` —
// SAM speckle on character art is never a real opening).
pub use super::matte::mask_bbox;

// ── Cutting ────────────────────────────────────────────────────────────────────

/// Lift every masked part out of the original source. Overlapping pixels are claimed by
/// the TOP-most part (highest doc index); lower parts lose them (Phase 4 inpaints what
/// they hid). Rebuilds a static skeleton — root + one bone per part at its claimed-pixel
/// centroid — and prunes clips whose targets vanished. The degenerate seed part (`all`)
/// is dropped once real masked parts exist.
pub fn cut_parts(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    mut doc: StudioDoc,
) -> Result<StudioDoc, String> {
    let source = image::open(store::source_path(base, game_id, asset_key))
        .map_err(|e| format!("open source failed: {e}"))?
        .to_rgba8();
    let (w, h) = source.dimensions();

    // Load masks for every part that has one.
    let mut masks: HashMap<String, GrayImage> = HashMap::new();
    for part in &doc.parts {
        let path = store::part_dir(base, game_id, asset_key, &part.id).join("mask.png");
        if path.is_file() {
            let m = image::open(&path)
                .map_err(|e| format!("open mask for {}: {e}", part.id))?
                .to_luma8();
            if m.dimensions() != (w, h) {
                return Err(format!(
                    "mask for \"{}\" is {}×{}, expected source dims {w}×{h}",
                    part.id,
                    m.width(),
                    m.height()
                ));
            }
            masks.insert(part.id.clone(), m);
        }
    }
    if masks.is_empty() {
        return Err("no part masks yet — segment parts before cutting".into());
    }
    // Snapshot pre-cut state so authored rigging survives the re-cut.
    let prev_hashes: HashMap<String, Option<String>> = doc
        .parts
        .iter()
        .map(|p| (p.id.clone(), p.mask_hash.clone()))
        .collect();
    let prev_slots: HashMap<String, Slot> =
        doc.slots.iter().map(|s| (s.part_id.clone(), s.clone())).collect();
    // FX layers (bbox but no mask — their texture was generated, not cut) pass through
    // untouched; everything else needs a mask to survive.
    let is_fx = |p: &super::doc::Part| p.mask_hash.is_none() && p.bbox.is_some();
    doc.parts.retain(|p| masks.contains_key(&p.id) || is_fx(p));

    // Exclusive top-wins claim + seam fringe + feather (+ sliver rescue): shared matte
    // machinery. Masks are passed in draw order (FX layers own no pixels and are skipped);
    // fully-occluded parts yield no piece and are dropped below.
    let ordered_masks: Vec<(String, GrayImage)> = doc
        .parts
        .iter()
        .filter_map(|p| masks.get(&p.id).map(|m| (p.id.clone(), m.clone())))
        .collect();
    let pieces = matte::exclusive_cut(
        &source,
        &ordered_masks,
        &matte::CutOptions {
            seam_fringe: doc.settings.seam_fringe,
            rescue_unclaimed: true,
        },
    )?;
    let by_id: HashMap<&str, &matte::CutPiece> =
        pieces.iter().map(|p| (p.id.as_str(), p)).collect();

    let mut centroids: HashMap<String, (f64, f64)> = HashMap::new();
    let mut kept_parts = Vec::new();
    for mut part in doc.parts.drain(..) {
        if !masks.contains_key(&part.id) {
            kept_parts.push(part); // FX layer — texture untouched by cutting
            continue;
        }
        let Some(piece) = by_id.get(part.id.as_str()) else {
            continue; // fully occluded by parts above — drop it
        };
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(piece.image.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| format!("encode cut for {}: {e}", part.id))?;
        store::write_file(
            &store::part_dir(base, game_id, asset_key, &part.id).join("cut.png"),
            &png.into_inner(),
        )?;

        let mask_png = {
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(masks[&part.id].clone())
                .write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| format!("encode mask for {}: {e}", part.id))?;
            buf.into_inner()
        };
        part.bbox = Some(piece.bbox);
        part.mask_hash = Some(sha256_hex(&mask_png));
        part.completed_hash = None;
        part.completed_bbox = None;
        part.texture = super::doc::PartTexture::Cut;
        centroids.insert(part.id.clone(), piece.centroid);
        kept_parts.push(part);
    }
    doc.parts = kept_parts;
    if doc.parts.is_empty() {
        return Err("all parts ended up empty after overlap resolution".into());
    }

    // ── Skeleton: PRESERVE authored rigging across re-cuts ────────────────────
    // Existing bones keep their pivots, aims and chains. Only parts that VANISHED lose
    // their anchor bone (plus its now-orphaned descendants); only NEW parts get a fresh
    // centroid anchor. Meshes survive when the part's cut is byte-identical.
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let part_ids: std::collections::HashSet<&str> =
        doc.parts.iter().map(|p| p.id.as_str()).collect();

    let mut bones = doc.bones.clone();
    if !bones.iter().any(|b| b.name == "root") {
        bones.insert(0, Bone::new("root", None, cx, cy));
    }
    // Doom the anchors of removed parts (unless a surviving part shares the bone)…
    let mut doomed: std::collections::HashSet<String> = prev_slots
        .iter()
        .filter(|(pid, _)| !part_ids.contains(pid.as_str()))
        .map(|(_, s)| s.bone.clone())
        .collect();
    for (pid, s) in &prev_slots {
        if part_ids.contains(pid.as_str()) {
            doomed.remove(&s.bone);
        }
    }
    // …and everything hanging under them.
    let mut grew = true;
    while grew {
        grew = false;
        let sweep: Vec<String> = bones
            .iter()
            .filter(|b| {
                !doomed.contains(&b.name)
                    && b.parent.as_deref().is_some_and(|p| doomed.contains(p))
            })
            .map(|b| b.name.clone())
            .collect();
        for name in sweep {
            doomed.insert(name);
            grew = true;
        }
    }
    bones.retain(|b| !doomed.contains(&b.name));
    // Fresh anchors for parts that don't have one yet.
    for part in &doc.parts {
        let anchor = prev_slots
            .get(&part.id)
            .map(|s| s.bone.clone())
            .unwrap_or_else(|| part.id.clone());
        if !bones.iter().any(|b| b.name == anchor) {
            // FX layers have no claimed centroid — anchor at their bbox centre.
            let (px, py) = centroids.get(&part.id).copied().unwrap_or_else(|| {
                let bb = part.bbox.unwrap_or(Rect { x: 0, y: 0, w, h });
                (bb.x as f64 + bb.w as f64 / 2.0, bb.y as f64 + bb.h as f64 / 2.0)
            });
            bones.push(Bone::new(anchor, Some("root".into()), px, py));
        }
    }
    doc.bones = bones;

    // Slots in draw (parts) order, bindings preserved; a changed cut resets the
    // attachment to Region (mesh vertices/UVs reference the old texture).
    doc.slots = doc
        .parts
        .iter()
        .map(|part| {
            let mut slot = prev_slots.get(&part.id).cloned().unwrap_or_else(|| Slot {
                name: part.id.clone(),
                bone: part.id.clone(),
                part_id: part.id.clone(),
                attachment: super::doc::Attachment::Region,
                blend: super::doc::BlendMode::Normal,
            });
            let changed = prev_hashes
                .get(&part.id)
                .map(|h| h != &part.mask_hash)
                .unwrap_or(true);
            if changed {
                slot.attachment = super::doc::Attachment::Region;
            }
            if !doc.bones.iter().any(|b| b.name == slot.bone) {
                slot.bone = "root".into(); // safety net; anchors were ensured above
            }
            slot
        })
        .collect();

    // Prune physics on bones that no longer exist.
    let bone_names_p: Vec<&str> = doc.bones.iter().map(|b| b.name.as_str()).collect();
    doc.physics.retain(|p| bone_names_p.contains(&p.bone.as_str()));

    // Prune clips whose targets no longer exist; keep at least a root idle.
    let bone_names: Vec<&str> = doc.bones.iter().map(|b| b.name.as_str()).collect();
    let slot_names: Vec<&str> = doc.slots.iter().map(|s| s.name.as_str()).collect();
    for clip in &mut doc.clips {
        clip.timelines.retain(|tl| match &tl.target {
            super::doc::TimelineTarget::BoneRotate(b)
            | super::doc::TimelineTarget::BoneTranslate(b)
            | super::doc::TimelineTarget::BoneScale(b) => bone_names.contains(&b.as_str()),
            super::doc::TimelineTarget::SlotAlpha(s)
            | super::doc::TimelineTarget::SlotColor(s) => slot_names.contains(&s.as_str()),
        });
    }
    doc.clips.retain(|c| !c.timelines.is_empty());
    if doc.clips.is_empty() {
        doc.clips.push(StudioDoc::breathe_idle("root"));
    }

    store::write_doc(base, game_id, asset_key, &doc)?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{Part, PartTexture, SourceRef};
    use image::RgbaImage;

    #[test]
    fn slugify_and_clamp() {
        let draft = ProposalDraft {
            parts: vec![
                PartDraft {
                    id: "Left Arm!".into(),
                    name: "Left arm".into(),
                    points: vec![PointDraft { x: 0.3, y: 1.4, label: "positive".into() }],
                },
                PartDraft {
                    id: "root".into(), // reserved
                    name: "Root".into(),
                    points: vec![PointDraft { x: 0.5, y: 0.5, label: "positive".into() }],
                },
                PartDraft {
                    id: "eyes".into(), // no positive point → dropped
                    name: "".into(),
                    points: vec![PointDraft { x: 0.1, y: 0.1, label: "negative".into() }],
                },
            ],
        };
        let out = clamp_proposals(draft);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "left_arm");
        assert!((out[0].points[0].y - 1.0).abs() < 1e-6); // clamped
    }

    #[test]
    fn refine_keeps_prompted_component_and_respects_alpha() {
        // Two blobs; alpha covers everything except a border.
        let (w, h) = (60u32, 40u32);
        let raw = GrayImage::from_fn(w, h, |x, y| {
            let a = x < 20 && y < 20; // blob A (top-left)
            let b = x > 40 && y > 25; // blob B (bottom-right)
            image::Luma([if a || b { 255 } else { 0 }])
        });
        let alpha = GrayImage::from_pixel(w, h, image::Luma([255]));
        let refined = matte::refine(&raw, Some(&alpha), &[(10, 10)], None);
        assert_eq!(refined.get_pixel(10, 10).0[0], 255, "blob A kept");
        assert_eq!(refined.get_pixel(50, 30).0[0], 0, "blob B dropped");
    }

    #[test]
    fn refine_falls_back_to_largest_component() {
        let (w, h) = (60u32, 40u32);
        let raw = GrayImage::from_fn(w, h, |x, _| image::Luma([if x < 30 { 255 } else { 0 }]));
        let refined = matte::refine(&raw, None, &[(59, 39)], None); // positive misses the blob
        assert_eq!(refined.get_pixel(15, 20).0[0], 255);
    }

    /// The Phase-2 acceptance test: cuts reassemble pixel-identical to the source
    /// (within the union of masks), and overlaps go to the top part.
    #[test]
    fn cut_parts_reassembles_source_exactly() {
        let base = std::env::temp_dir().join(format!("wf_studio_cut_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "sym");
        let (w, h) = (80u32, 60u32);

        // Source: gradient so pixel identity is meaningful; fully opaque.
        let source = RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 3) as u8, (y * 4) as u8, ((x + y) % 251) as u8, 255])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        store::write_file(&store::source_path(&base, gid, key), &png.into_inner()).unwrap();

        // Two overlapping masks: "back" covers x<50, "front" covers x>=30 (overlap 30..50).
        let write_mask = |id: &str, pred: &dyn Fn(u32, u32) -> bool| {
            let m = GrayImage::from_fn(w, h, |x, y| image::Luma([if pred(x, y) { 255 } else { 0 }]));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(m)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            store::write_file(
                &store::part_dir(&base, gid, key, id).join("mask.png"),
                &buf.into_inner(),
            )
            .unwrap();
        };
        write_mask("back", &|x, _| x < 50);
        write_mask("front", &|x, _| x >= 30);

        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        let mk = |id: &str| Part {
            id: id.into(),
            name: id.into(),
            prompts: vec![],
            bbox: None,
            mask_hash: None,
            completed_hash: None,
            completed_bbox: None,
            texture: PartTexture::Cut,
            deformable: false,
        };
        doc.parts = vec![mk("back"), mk("front")]; // front = higher index = on top

        let doc = cut_parts(&base, gid, key, doc).unwrap();

        // Seed part "all" dropped (no mask); both real parts kept, bboxes set.
        assert_eq!(doc.parts.len(), 2);
        let back = doc.part("back").unwrap();
        let front = doc.part("front").unwrap();
        // Back keeps a duplicated fringe UNDER front (default 3px + 1px feather): its bbox
        // reaches past the x=30 seam. Front has nothing above it → no fringe, only the
        // 1px feather on its left edge.
        let bb_back = back.bbox.unwrap();
        assert_eq!((bb_back.x, bb_back.y, bb_back.h), (0, 0, 60));
        assert!(
            (33..=35).contains(&bb_back.w),
            "back fringe extends under front: w={}",
            bb_back.w
        );
        let bb_front = front.bbox.unwrap();
        assert!((29..=30).contains(&bb_front.x), "front x={}", bb_front.x);

        // Reassemble with real alpha compositing in draw order: the duplicated fringe and
        // feathering sample the same source pixels, so the stack still matches the source
        // (up to u8 rounding at the soft seam).
        let mut recomposed = RgbaImage::new(w, h);
        for part in &doc.parts {
            let bb = part.bbox.unwrap();
            let cut = image::open(store::part_dir(&base, gid, key, &part.id).join("cut.png"))
                .unwrap()
                .to_rgba8();
            for (x, y, p) in cut.enumerate_pixels() {
                let (dx, dy) = (bb.x as u32 + x, bb.y as u32 + y);
                let under = *recomposed.get_pixel(dx, dy);
                let ta = p.0[3] as f32 / 255.0;
                let ua = under.0[3] as f32 / 255.0;
                let oa = ta + ua * (1.0 - ta);
                if oa <= 0.0 {
                    continue;
                }
                let ch = |t: u8, u: u8| {
                    (((t as f32 * ta + u as f32 * ua * (1.0 - ta)) / oa).round() as u8).min(255)
                };
                recomposed.put_pixel(
                    dx,
                    dy,
                    image::Rgba([
                        ch(p.0[0], under.0[0]),
                        ch(p.0[1], under.0[1]),
                        ch(p.0[2], under.0[2]),
                        (oa * 255.0).round() as u8,
                    ]),
                );
            }
        }
        for (x, y, want) in source.enumerate_pixels() {
            let got = recomposed.get_pixel(x, y);
            for c in 0..4 {
                assert!(
                    (got.0[c] as i16 - want.0[c] as i16).abs() <= 3,
                    "reassembly diverged at ({x},{y}) ch{c}: got {} want {}",
                    got.0[c],
                    want.0[c]
                );
            }
        }

        // Skeleton rebuilt: root + 2 bones, slots in z order, idle clip retargeted to root.
        assert_eq!(doc.bones.len(), 3);
        assert_eq!(doc.slots[0].name, "back");
        assert_eq!(doc.slots[1].name, "front");
        assert!(doc.clips.iter().any(|c| c.name == "idle"));

        // ── Re-cut preserves the authored rig ─────────────────────────────────
        let mut rigged = doc.clone();
        // User adjusts the front pivot, adds a chain + physics under it.
        let fi = rigged.bones.iter().position(|b| b.name == "front").unwrap();
        rigged.bones[fi].x = 44.0;
        rigged.bones[fi].rotation = 30.0;
        rigged
            .bones
            .push(crate::studio::doc::Bone::new("front_seg2", Some("front".into()), 70.0, 30.0));
        rigged.physics = vec![crate::studio::doc::PhysicsSpec::sway("front_seg2")];
        crate::studio::store::write_doc(&base, gid, key, &rigged).unwrap();

        let recut = cut_parts(&base, gid, key, rigged).unwrap();
        let front = recut.bones.iter().find(|b| b.name == "front").unwrap();
        assert_eq!(front.x, 44.0, "adjusted pivot survives the re-cut");
        assert_eq!(front.rotation, 30.0);
        assert!(recut.bones.iter().any(|b| b.name == "front_seg2"), "chain survives");
        assert_eq!(recut.physics.len(), 1, "physics survives");

        // Removing a part's mask kills its anchor + chain + physics on re-cut.
        std::fs::remove_file(store::part_dir(&base, gid, key, "front").join("mask.png")).unwrap();
        let shrunk = cut_parts(&base, gid, key, recut).unwrap();
        assert!(shrunk.part("front").is_none());
        assert!(!shrunk.bones.iter().any(|b| b.name == "front" || b.name == "front_seg2"));
        assert!(shrunk.physics.is_empty());
        assert!(shrunk.bones.iter().any(|b| b.name == "back"), "unrelated rig intact");

        let _ = std::fs::remove_dir_all(&base);
    }
}
