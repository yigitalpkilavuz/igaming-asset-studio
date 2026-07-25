//! Auto-rig: propose a bone skeleton for the cut parts. Structure comes from AI (gpt-4o
//! names the parent tree over the part list), geometry comes from Rust:
//! - adjacency = dilated-mask overlap between part pairs,
//! - pivot of a child bone = centroid of the overlap band with its parent part
//!   (that's where the joint physically is),
//! - elongated parts (PCA aspect > ~2.2) get their bone aimed down the principal axis
//!   with a real length, so limbs bend from the joint along the limb.
//!
//! Everything is computed on masks downscaled to ≤512 px for speed, then scaled back.

use std::collections::HashMap;

use image::GrayImage;
use serde::Deserialize;

use super::doc::{Bone, StudioDoc};

/// Analysis geometry for one part (source-pixel coordinates).
#[derive(Debug, Clone)]
pub struct PartGeo {
    pub id: String,
    pub area: f64,
    pub centroid: (f64, f64),
    /// Principal axis direction (unit, y-down) and elongation ratio (major/minor).
    pub axis: (f64, f64),
    pub aspect: f64,
    /// Max distance of a mask pixel from the centroid, projected on the axis.
    pub extent: f64,
}

/// Overlap band between two adjacent parts.
#[derive(Debug, Clone)]
pub struct Adjacency {
    pub a: String,
    pub b: String,
    pub band_centroid: (f64, f64),
    /// Overlap pixel count (source scale) — strength of the connection.
    pub strength: f64,
}

#[derive(Debug, Clone)]
pub struct RigAnalysis {
    pub parts: Vec<PartGeo>,
    pub adjacency: Vec<Adjacency>,
}

const ANALYSIS_MAX: u32 = 512;
const DILATE_R: usize = 3;

/// Analyze part masks (id, full-size mask) into geometry + adjacency.
pub fn analyze(masks: &[(String, GrayImage)]) -> Result<RigAnalysis, String> {
    if masks.is_empty() {
        return Err("no masks to analyze".into());
    }
    let (sw, sh) = masks[0].1.dimensions();
    let scale = (ANALYSIS_MAX as f64 / sw.max(sh).max(1) as f64).min(1.0);
    let (dw, dh) = (
        ((sw as f64 * scale).round() as u32).max(1),
        ((sh as f64 * scale).round() as u32).max(1),
    );
    let inv = 1.0 / scale;

    // Downscaled boolean masks + their dilated versions.
    let mut small: Vec<Vec<bool>> = Vec::new();
    let mut dilated: Vec<Vec<bool>> = Vec::new();
    for (_, m) in masks {
        let d = image::imageops::resize(m, dw, dh, image::imageops::FilterType::Nearest);
        let bits: Vec<bool> = d.pixels().map(|p| p.0[0] > 127).collect();
        let mut dil = bits.clone();
        dilate(&mut dil, dw as usize, dh as usize, DILATE_R);
        small.push(bits);
        dilated.push(dil);
    }

    // Per-part geometry (PCA over set pixels).
    let mut parts = Vec::new();
    for (i, (id, _)) in masks.iter().enumerate() {
        let bits = &small[i];
        let (mut sx, mut sy, mut n) = (0f64, 0f64, 0f64);
        for (k, &b) in bits.iter().enumerate() {
            if b {
                sx += (k as u32 % dw) as f64;
                sy += (k as u32 / dw) as f64;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return Err(format!("mask for \"{id}\" is empty"));
        }
        let (cx, cy) = (sx / n, sy / n);
        let (mut xx, mut xy, mut yy) = (0f64, 0f64, 0f64);
        for (k, &b) in bits.iter().enumerate() {
            if b {
                let dx = (k as u32 % dw) as f64 - cx;
                let dy = (k as u32 / dw) as f64 - cy;
                xx += dx * dx;
                xy += dx * dy;
                yy += dy * dy;
            }
        }
        (xx, xy, yy) = (xx / n, xy / n, yy / n);
        // Eigen decomposition of the 2×2 covariance.
        let tr = xx + yy;
        let det = xx * yy - xy * xy;
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
        let axis = if xy.abs() > 1e-9 {
            let v = (l1 - yy, xy);
            let len = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-9);
            (v.0 / len, v.1 / len)
        } else if xx >= yy {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        let aspect = (l1.max(1e-9) / l2.max(1e-9)).sqrt();
        let mut extent = 0f64;
        for (k, &b) in bits.iter().enumerate() {
            if b {
                let dx = (k as u32 % dw) as f64 - cx;
                let dy = (k as u32 / dw) as f64 - cy;
                extent = extent.max((dx * axis.0 + dy * axis.1).abs());
            }
        }
        parts.push(PartGeo {
            id: id.clone(),
            area: n * inv * inv,
            centroid: (cx * inv, cy * inv),
            axis,
            aspect,
            extent: extent * inv,
        });
    }

    // Pairwise adjacency via dilated overlap.
    let mut adjacency = Vec::new();
    for i in 0..masks.len() {
        for j in (i + 1)..masks.len() {
            let (mut sx, mut sy, mut n) = (0f64, 0f64, 0f64);
            for k in 0..small[i].len() {
                if dilated[i][k] && dilated[j][k] {
                    sx += (k as u32 % dw) as f64;
                    sy += (k as u32 / dw) as f64;
                    n += 1.0;
                }
            }
            if n > 3.0 {
                adjacency.push(Adjacency {
                    a: masks[i].0.clone(),
                    b: masks[j].0.clone(),
                    band_centroid: (sx / n * inv, sy / n * inv),
                    strength: n * inv * inv,
                });
            }
        }
    }

    Ok(RigAnalysis { parts, adjacency })
}

fn dilate(mask: &mut [bool], w: usize, h: usize, r: usize) {
    let src = mask.to_vec();
    let mut tmp = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = (lo..=hi).any(|xx| src[y * w + xx]);
        }
    }
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            mask[y * w + x] = (lo..=hi).any(|yy| tmp[yy * w + x]);
        }
    }
}

// ── Parent tree ────────────────────────────────────────────────────────────────

/// Heuristic tree: the largest part anchors to root; everything else hangs off its
/// strongest-overlap neighbour, BFS from the anchor. Disconnected parts anchor to root.
pub fn heuristic_tree(analysis: &RigAnalysis) -> HashMap<String, String> {
    let mut tree: HashMap<String, String> = HashMap::new();
    let Some(anchor) = analysis
        .parts
        .iter()
        .max_by(|a, b| a.area.partial_cmp(&b.area).unwrap())
    else {
        return tree;
    };
    tree.insert(anchor.id.clone(), "root".into());

    // Repeatedly attach the unattached part with the strongest link to an attached one.
    loop {
        let mut best: Option<(&Adjacency, String, String)> = None; // (adj, child, parent)
        for adj in &analysis.adjacency {
            let (att_a, att_b) = (tree.contains_key(&adj.a), tree.contains_key(&adj.b));
            let (child, parent) = match (att_a, att_b) {
                (false, true) => (adj.a.clone(), adj.b.clone()),
                (true, false) => (adj.b.clone(), adj.a.clone()),
                _ => continue,
            };
            if best.as_ref().is_none_or(|(b, _, _)| adj.strength > b.strength) {
                best = Some((adj, child, parent));
            }
        }
        match best {
            Some((_, child, parent)) => {
                tree.insert(child, parent);
            }
            None => break,
        }
    }
    // Disconnected leftovers.
    for p in &analysis.parts {
        tree.entry(p.id.clone()).or_insert_with(|| "root".into());
    }
    tree
}

const TREE_SYSTEM: &str = "\
You are rigging a 2D cutout character for skeletal animation. Given its parts (with area, \
centre position normalized 0..1, and which parts physically touch), output the bone parent \
tree. Rules:\n\
- Exactly one part is the anchor with parent \"root\" — the torso/body-like central mass.\n\
- Every other part's parent is the part it should move WITH and hinge FROM (an arm hangs \
off the torso, a hand off the arm, a held prop off the hand/arm, a head off the torso).\n\
- Parents must come from the part list or be \"root\". Prefer parents the part touches.\n\
Output ONLY JSON: {\"tree\": {\"<partId>\": \"<parentId or root>\", ...}} covering every part.";

#[derive(Deserialize)]
struct TreeDraft {
    #[serde(default)]
    tree: HashMap<String, String>,
}

/// Ask gpt-4o for the parent tree. Falls back cleanly on any error (caller decides).
pub async fn propose_tree_ai(
    api_key: &str,
    analysis: &RigAnalysis,
    source_w: u32,
    source_h: u32,
) -> Result<HashMap<String, String>, String> {
    let parts: Vec<serde_json::Value> = analysis
        .parts
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "area": p.area.round(),
                "cx": (p.centroid.0 / source_w as f64 * 100.0).round() / 100.0,
                "cy": (p.centroid.1 / source_h as f64 * 100.0).round() / 100.0,
            })
        })
        .collect();
    let touches: Vec<serde_json::Value> = analysis
        .adjacency
        .iter()
        .map(|a| serde_json::json!([a.a, a.b]))
        .collect();
    let user = serde_json::json!({ "parts": parts, "touching": touches }).to_string();

    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "system", "content": TREE_SYSTEM },
            { "role": "user", "content": user },
        ],
        "temperature": 0.2,
        "response_format": { "type": "json_object" },
    });
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("tree request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    if !status.is_success() {
        let trunc: String = text.chars().take(300).collect();
        return Err(format!("OpenAI error {status}: {trunc}"));
    }
    #[derive(Deserialize)]
    struct Resp {
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
    let parsed: Resp = serde_json::from_str(&text).map_err(|e| format!("bad response: {e}"))?;
    let content = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "no choices".to_string())?;
    let draft: TreeDraft =
        serde_json::from_str(&content).map_err(|e| format!("could not parse tree: {e}"))?;
    Ok(validate_tree(draft.tree, analysis))
}

/// Clamp an AI tree into a valid one: unknown parts dropped, unknown/self parents → root,
/// cycles broken by re-anchoring to root, missing parts filled from the heuristic.
pub fn validate_tree(
    mut tree: HashMap<String, String>,
    analysis: &RigAnalysis,
) -> HashMap<String, String> {
    let ids: Vec<&str> = analysis.parts.iter().map(|p| p.id.as_str()).collect();
    tree.retain(|k, _| ids.contains(&k.as_str()));
    for (k, v) in tree.iter_mut() {
        if v != "root" && (!ids.contains(&v.as_str()) || v == k) {
            *v = "root".into();
        }
    }
    let fallback = heuristic_tree(analysis);
    for id in &ids {
        if !tree.contains_key(*id) {
            tree.insert(
                (*id).to_string(),
                fallback.get(*id).cloned().unwrap_or_else(|| "root".into()),
            );
        }
    }
    // Break cycles: walk each node to root; on revisit, cut the current node to root.
    let keys: Vec<String> = tree.keys().cloned().collect();
    for start in keys {
        let mut seen = vec![start.clone()];
        let mut cur = start.clone();
        loop {
            let parent = tree.get(&cur).cloned().unwrap_or_else(|| "root".into());
            if parent == "root" {
                break;
            }
            if seen.contains(&parent) {
                tree.insert(parent.clone(), "root".into());
                break;
            }
            seen.push(parent.clone());
            cur = parent;
        }
    }
    tree
}

// ── Bone building ──────────────────────────────────────────────────────────────

/// Aspect ratio above which a part counts as elongated (limb/prop) and its bone gets a
/// direction + length.
const ELONGATED: f64 = 2.2;

/// Build the bone set from the analysis + tree, replacing `doc.bones`. Slots keep their
/// `bone == part_id` binding; clips survive because bone names don't change.
pub fn apply(mut doc: StudioDoc, analysis: &RigAnalysis, tree: &HashMap<String, String>) -> StudioDoc {
    let geo: HashMap<&str, &PartGeo> =
        analysis.parts.iter().map(|p| (p.id.as_str(), p)).collect();
    let band: HashMap<(&str, &str), (f64, f64)> = analysis
        .adjacency
        .iter()
        .flat_map(|a| {
            [
                ((a.a.as_str(), a.b.as_str()), a.band_centroid),
                ((a.b.as_str(), a.a.as_str()), a.band_centroid),
            ]
        })
        .collect();

    let (cx, cy) = (doc.source.width as f64 / 2.0, doc.source.height as f64 / 2.0);
    let mut bones = vec![Bone::new("root", None, cx, cy)];

    // Emit parts parents-first so the doc stays topologically ordered.
    let mut emitted: Vec<&str> = vec!["root"];
    let mut remaining: Vec<&super::doc::Part> = doc.parts.iter().collect();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|part| {
            let parent = tree.get(&part.id).map(|s| s.as_str()).unwrap_or("root");
            if !emitted.contains(&parent) {
                return true;
            }
            let Some(g) = geo.get(part.id.as_str()) else {
                return false; // no geometry (shouldn't happen) — skip
            };
            // Pivot: the joint = overlap band with the parent part; anchors use centroid.
            let (px, py) = if parent == "root" {
                g.centroid
            } else {
                band
                    .get(&(part.id.as_str(), parent))
                    .copied()
                    .unwrap_or(g.centroid)
            };
            let mut bone = Bone::new(part.id.clone(), Some(parent.to_string()), px, py);
            if g.aspect >= ELONGATED {
                // Aim down the principal axis, away from the pivot toward the mass.
                let to_mass = (g.centroid.0 - px, g.centroid.1 - py);
                let mut axis = g.axis;
                if axis.0 * to_mass.0 + axis.1 * to_mass.1 < 0.0 {
                    axis = (-axis.0, -axis.1);
                }
                // y-down axis → Spine CCW degrees.
                bone.rotation = (-axis.1).atan2(axis.0).to_degrees();
                let along = (g.centroid.0 - px) * axis.0 + (g.centroid.1 - py) * axis.1;
                bone.length = (along + g.extent).max(g.extent * 0.5).round();
            }
            emitted.push(part.id.as_str());
            bones.push(bone);
            false
        });
        if remaining.len() == before {
            // Tree references only emitted parts after validate_tree; this is a safety net.
            for part in remaining.drain(..) {
                if let Some(g) = geo.get(part.id.as_str()) {
                    bones.push(Bone::new(part.id.clone(), Some("root".into()), g.centroid.0, g.centroid.1));
                }
            }
        }
    }

    doc.bones = bones;
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{SourceRef, StudioDoc};

    /// Two touching rectangles: big "torso" (left) and long thin "arm" (right).
    fn masks() -> Vec<(String, GrayImage)> {
        let (w, h) = (400u32, 300u32);
        let torso = GrayImage::from_fn(w, h, |x, y| {
            image::Luma([if x < 200 && y > 40 && y < 260 { 255 } else { 0 }])
        });
        let arm = GrayImage::from_fn(w, h, |x, y| {
            image::Luma([if x >= 198 && x < 390 && y > 130 && y < 170 { 255 } else { 0 }])
        });
        vec![("torso".into(), torso), ("arm".into(), arm)]
    }

    #[test]
    fn analysis_finds_adjacency_and_elongation() {
        let a = analyze(&masks()).unwrap();
        assert_eq!(a.parts.len(), 2);
        let arm = a.parts.iter().find(|p| p.id == "arm").unwrap();
        let torso = a.parts.iter().find(|p| p.id == "torso").unwrap();
        assert!(torso.area > arm.area);
        assert!(arm.aspect > ELONGATED, "arm aspect {}", arm.aspect);
        // Arm axis is horizontal.
        assert!(arm.axis.0.abs() > 0.95, "axis {:?}", arm.axis);
        assert_eq!(a.adjacency.len(), 1);
        // Band sits near the seam x≈200, mid-height.
        let band = a.adjacency[0].band_centroid;
        assert!((band.0 - 200.0).abs() < 20.0, "band x {}", band.0);
        assert!((band.1 - 150.0).abs() < 20.0, "band y {}", band.1);
    }

    #[test]
    fn heuristic_anchors_largest_and_attaches_neighbours() {
        let a = analyze(&masks()).unwrap();
        let tree = heuristic_tree(&a);
        assert_eq!(tree["torso"], "root");
        assert_eq!(tree["arm"], "torso");
    }

    #[test]
    fn apply_builds_joint_pivot_and_aimed_bone() {
        let a = analyze(&masks()).unwrap();
        let tree = heuristic_tree(&a);
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 400, height: 300, sha256: "s".into() },
            0.0,
        );
        doc.parts = vec![
            crate::studio::doc::Part {
                id: "torso".into(), name: "torso".into(), prompts: vec![], bbox: None,
                mask_hash: None, completed_hash: None, completed_bbox: None, texture: Default::default(),
            },
            crate::studio::doc::Part {
                id: "arm".into(), name: "arm".into(), prompts: vec![], bbox: None,
                mask_hash: None, completed_hash: None, completed_bbox: None, texture: Default::default(),
            },
        ];
        let doc = apply(doc, &a, &tree);
        assert_eq!(doc.bones.len(), 3);
        let arm = doc.bones.iter().find(|b| b.name == "arm").unwrap();
        assert_eq!(arm.parent.as_deref(), Some("torso"));
        // Pivot at the shoulder seam, not the arm centre.
        assert!((arm.x - 200.0).abs() < 25.0, "pivot x {}", arm.x);
        // Aimed to the right (0°) with a real length reaching toward the hand.
        assert!(arm.rotation.abs() < 15.0, "rotation {}", arm.rotation);
        assert!(arm.length > 100.0, "length {}", arm.length);
        // Torso is the anchor: parented to root, no aim.
        let torso = doc.bones.iter().find(|b| b.name == "torso").unwrap();
        assert_eq!(torso.parent.as_deref(), Some("root"));
        assert_eq!(torso.rotation, 0.0);
    }

    #[test]
    fn validate_tree_breaks_cycles_and_fills_gaps() {
        let a = analyze(&masks()).unwrap();
        let mut bad = HashMap::new();
        bad.insert("torso".to_string(), "arm".to_string());
        bad.insert("arm".to_string(), "torso".to_string()); // cycle
        let fixed = validate_tree(bad, &a);
        // Cycle broken: at least one of them anchors to root, none dangling.
        assert!(fixed.values().any(|v| v == "root"));
        let mut unknown = HashMap::new();
        unknown.insert("arm".to_string(), "ghost".to_string());
        let fixed = validate_tree(unknown, &a);
        assert_eq!(fixed["arm"], "root");
        assert!(fixed.contains_key("torso")); // filled from heuristic
    }
}
