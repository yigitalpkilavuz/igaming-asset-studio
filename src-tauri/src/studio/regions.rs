//! Region-first auto cut: instead of an LLM GUESSING seed coordinates for SAM (vision
//! models are unreliable at precise positions), SAM first segments the whole image into
//! candidate regions from a point grid — real boundaries, no guessing — and the LLM only
//! LABELS them ("regions 3+7 = head"), which is a recognition task it is actually good
//! at. Part masks are unions of assigned regions; everything downstream (refine, cut,
//! fill, corrections) is unchanged.

use image::{GrayImage, Rgb, RgbImage, RgbaImage};
use serde::Deserialize;

use super::doc::Rect;
use super::matte;
use super::sam::{self, weights::SamModel};
use super::segment::{slugify, vision_json_multi};

/// One candidate region discovered by the grid sweep, at SOURCE dimensions.
pub struct Region {
    /// 1-based display id (drawn on the overlay, referenced by the labeler).
    pub id: u32,
    pub mask: GrayImage,
    pub area: u32,
    pub bbox: Rect,
}

/// A part proposed by the labeler: named group of region ids.
pub struct LabeledPart {
    pub id: String,
    pub name: String,
    pub region_ids: Vec<u32>,
}

const MAX_REGIONS: usize = 48;
const IOU_DEDUPE: f64 = 0.8;

/// Sweep a `grid`×`grid` point lattice over the opaque pixels, decode a SAM mask per
/// point (encoder is computed once and cached), dedupe by IoU, and return the surviving
/// regions largest-first. Runs blocking — call inside `spawn_blocking`.
pub fn discover(
    weights: &std::path::Path,
    model: SamModel,
    source: &RgbaImage,
    source_sha: &str,
    sam_dir: &std::path::Path,
    grid: u32,
) -> Result<Vec<Region>, String> {
    let (w, h) = source.dimensions();
    // SAM sees the art composited over neutral gray; alpha clips masks afterwards.
    let mut rgb = RgbImage::new(w, h);
    let mut alpha = GrayImage::new(w, h);
    for (x, y, p) in source.enumerate_pixels() {
        let a = p.0[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 128 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
        alpha.put_pixel(x, y, image::Luma([p.0[3]]));
    }
    let opaque_total: u64 = alpha.pixels().filter(|p| p.0[0] > 8).count() as u64;
    if opaque_total == 0 {
        return Err("source image is fully transparent".into());
    }

    // All decodes under one engine lock. Keep ALL 3 decoder candidates per point (best-
    // IoU-only collapses granularity toward whole-object blobs), filtered by the model's
    // own quality scores. Masks stay at SAM's resized dims until after dedupe.
    const MIN_IOU: f32 = 0.75;
    const MIN_STABILITY: f32 = 0.85;
    let candidates: Vec<(GrayImage, u64)> = sam::with_engine(weights, model, |engine| {
        let mut out: Vec<(GrayImage, u64)> = Vec::new();
        for gy in 0..grid {
            for gx in 0..grid {
                let nx = (gx as f64 + 0.5) / grid as f64;
                let ny = (gy as f64 + 0.5) / grid as f64;
                let px = (nx * (w - 1) as f64).round() as u32;
                let py = (ny * (h - 1) as f64).round() as u32;
                if alpha.get_pixel(px, py).0[0] <= 8 {
                    continue; // transparent — nothing to segment here
                }
                let cands =
                    engine.segment_candidates(source_sha, &rgb, &[(nx, ny, true)], Some(sam_dir))?;
                for (mask, iou, stability) in cands {
                    if iou < MIN_IOU || stability < MIN_STABILITY {
                        continue;
                    }
                    let area: u64 = mask.pixels().filter(|p| p.0[0] > 127).count() as u64;
                    out.push((mask, area));
                }
            }
        }
        Ok(out)
    })?;
    if candidates.is_empty() {
        return Err("no opaque grid points — is the source empty?".into());
    }

    // Scale-independent area limits, computed against the resized frame.
    let (rw, rh) = candidates[0].0.dimensions();
    let resized_total = (rw as u64) * (rh as u64);
    let min_area = (resized_total / 500).max(64); // ≥ 0.2% of the frame
    let max_area = resized_total * 92 / 100;

    // Largest-first greedy IoU dedupe.
    let mut sorted: Vec<&(GrayImage, u64)> = candidates
        .iter()
        .filter(|(_, a)| *a >= min_area && *a <= max_area)
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let mut kept: Vec<(Vec<bool>, u64)> = Vec::new();
    for (mask, area) in sorted {
        if kept.len() >= MAX_REGIONS {
            break;
        }
        let bits: Vec<bool> = mask.pixels().map(|p| p.0[0] > 127).collect();
        let dup = kept.iter().any(|(kb, ka)| {
            let inter = bits.iter().zip(kb).filter(|(a, b)| **a && **b).count() as u64;
            let union = area + ka - inter;
            union > 0 && (inter as f64 / union as f64) > IOU_DEDUPE
        });
        if !dup {
            kept.push((bits, *area));
        }
    }

    // Upscale survivors to source dims, clip to alpha, light cleanup, final bookkeeping.
    let mut regions = Vec::new();
    for (bits, _) in kept {
        let mut small = GrayImage::new(rw, rh);
        for (px, &b) in small.pixels_mut().zip(bits.iter()) {
            px.0[0] = if b { 255 } else { 0 };
        }
        let full = image::imageops::resize(&small, w, h, image::imageops::FilterType::Triangle);
        let mut mb: Vec<bool> = full
            .pixels()
            .zip(alpha.pixels())
            .map(|(p, a)| p.0[0] > 127 && a.0[0] > 8)
            .collect();
        matte::fill_holes(&mut mb, w as usize, h as usize, Some(((w * h) / 2000).max(64) as usize));
        let mut mask = GrayImage::new(w, h);
        let mut area = 0u32;
        for (px, &b) in mask.pixels_mut().zip(mb.iter()) {
            px.0[0] = if b {
                area += 1;
                255
            } else {
                0
            };
        }
        if area == 0 {
            continue;
        }
        let bbox = matte::mask_bbox(&mask).unwrap();
        regions.push(Region { id: 0, mask, area, bbox });
    }
    regions.sort_by(|a, b| b.area.cmp(&a.area));
    for (i, r) in regions.iter_mut().enumerate() {
        r.id = (i + 1) as u32;
    }
    if regions.len() < 2 {
        return Err("could not discover distinct regions — try manual selection".into());
    }
    Ok(regions)
}

// ── Numbered overlay for the labeler ───────────────────────────────────────────

const PALETTE: [[u8; 3]; 12] = [
    [230, 80, 80], [80, 160, 230], [90, 200, 120], [235, 180, 60],
    [180, 100, 220], [70, 200, 200], [235, 120, 40], [120, 130, 235],
    [200, 210, 70], [230, 100, 170], [100, 220, 170], [160, 160, 160],
];

/// 3×5 bitmap digits (rows of 3 bits, MSB left) — enough to number regions without a
/// font dependency.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b010, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

fn draw_number(img: &mut RgbaImage, n: u32, cx: i32, cy: i32, scale: i32) {
    let digits: Vec<u32> = n
        .to_string()
        .chars()
        .filter_map(|c| c.to_digit(10))
        .collect();
    let dw = 3 * scale + scale; // glyph + gap
    let total_w = dw * digits.len() as i32 - scale;
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = cx - total_w / 2;
    let y0 = cy - (5 * scale) / 2;
    // Backing plate for contrast.
    for y in (y0 - scale)..(y0 + 5 * scale + scale) {
        for x in (x0 - scale)..(x0 + total_w + scale) {
            if x >= 0 && y >= 0 && x < w && y < h {
                img.put_pixel(x as u32, y as u32, image::Rgba([12, 13, 16, 255]));
            }
        }
    }
    for (di, d) in digits.iter().enumerate() {
        let gx = x0 + di as i32 * dw;
        for (row, bits) in DIGITS[*d as usize].iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = gx + col * scale + sx;
                            let y = y0 + row as i32 * scale + sy;
                            if x >= 0 && y >= 0 && x < w && y < h {
                                img.put_pixel(x as u32, y as u32, image::Rgba([255, 255, 255, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Render the labeling overlay: dimmed source, each region tinted a palette colour,
/// its number drawn at the bbox centre. Returned as PNG bytes.
pub fn overlay_png(source: &RgbaImage, regions: &[Region]) -> Result<Vec<u8>, String> {
    let (w, h) = source.dimensions();
    let mut img = RgbaImage::new(w, h);
    // Dimmed source over mid gray so both dark and light art stays readable.
    for (x, y, p) in source.enumerate_pixels() {
        let a = p.0[3] as u32;
        let blend = |c: u8| (((c as u32 * a + 120 * (255 - a)) / 255) / 2 + 50) as u8;
        img.put_pixel(x, y, image::Rgba([blend(p.0[0]), blend(p.0[1]), blend(p.0[2]), 255]));
    }
    for r in regions {
        let col = PALETTE[((r.id - 1) as usize) % PALETTE.len()];
        for (x, y, m) in r.mask.enumerate_pixels() {
            if m.0[0] > 127 {
                let p = img.get_pixel_mut(x, y);
                for c in 0..3 {
                    p.0[c] = ((p.0[c] as u32 * 55 + col[c] as u32 * 45) / 100) as u8;
                }
            }
        }
    }
    for r in regions {
        let cx = r.bbox.x + r.bbox.w as i32 / 2;
        let cy = r.bbox.y + r.bbox.h as i32 / 2;
        let scale = ((r.bbox.w.min(r.bbox.h) as i32) / 24).clamp(3, 10);
        draw_number(&mut img, r.id, cx, cy, scale);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode overlay: {e}"))?;
    Ok(buf.into_inner())
}

// ── LLM labeling ───────────────────────────────────────────────────────────────

const LABEL_SYSTEM: &str = "\
You are labeling a cutout plan for 2D skeletal animation. You receive TWO images: first \
the ORIGINAL artwork (use it to recognize what each area actually IS — a thumb belongs \
to the hand, an engraving belongs to the plate it is carved on), second the SAME artwork \
divided into numbered, colored regions (the regions heavily overlap and over-segment — \
that is expected). Group the numbered regions into the FEWEST separately-moving parts \
that allow expressive motion. Rules:\n\
- Typically 3 to 6 parts; never more than 8. MERGE AGGRESSIVELY: fragments of the same \
physical piece, decorations painted ON a piece, and tiny slivers all belong to their \
surrounding part. When unsure, merge.\n\
- A part is only separate if it should MOVE independently: head, limbs, tail/wings, a \
held prop; for an object: the main body plus each dangling/swaying ornament. A pattern, \
face feature, or texture detail is NOT a part.\n\
- If a PLANNED MOTION is given, partition FOR that motion: every element the motion \
moves on its own must be its own part; everything the motion never separates stays \
merged into its neighbour.\n\
- Assign every region number to exactly ONE part (overlapping regions: give the number \
to the part it mostly belongs to).\n\
- Order parts BACKGROUND-most first, FOREGROUND-most last.\n\
- Each part: a short lowercase slug id (a-z, 0-9, underscore) and a display name.\n\
Output ONLY JSON: {\"parts\":[{\"id\":string,\"name\":string,\"regions\":[numbers]}]}";

#[derive(Deserialize)]
struct LabelDraft {
    #[serde(default)]
    parts: Vec<LabelPartDraft>,
}
#[derive(Deserialize)]
struct LabelPartDraft {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    regions: Vec<u32>,
}

/// Ask the vision model to group the numbered regions into parts. Sends the ORIGINAL
/// artwork first (for recognition) and the numbered overlay second (for the mapping).
pub async fn label(
    api_key: &str,
    original: &[u8],
    overlay: &[u8],
    region_count: usize,
    hint: &str,
    motion: &str,
) -> Result<Vec<LabeledPart>, String> {
    let mut user = String::new();
    if !hint.trim().is_empty() {
        user.push_str(&format!("Context: {}\n", hint.trim()));
    }
    if !motion.trim().is_empty() {
        user.push_str(&format!(
            "PLANNED MOTION: {}\nCut for this motion — each element it moves on its own \
becomes its own part; everything else stays merged.\n",
            motion.trim()
        ));
    }
    user.push_str(&format!(
        "First image: original artwork. Second image: {region_count} numbered regions \
(1-{region_count}). Group every region into parts."
    ));
    let content = vision_json_multi(api_key, LABEL_SYSTEM, &user, &[original, overlay]).await?;
    let draft: LabelDraft =
        serde_json::from_str(&content).map_err(|e| format!("could not parse labeling: {e}"))?;
    Ok(clamp_labels(draft, region_count))
}

/// Validate the draft: slugged unique ids, in-range region numbers, each region used once.
fn clamp_labels(draft: LabelDraft, region_count: usize) -> Vec<LabeledPart> {
    let mut seen_ids: Vec<String> = Vec::new();
    let mut used_regions: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in draft.parts {
        let slug = slugify(if p.id.trim().is_empty() { &p.name } else { &p.id });
        if slug.is_empty() || slug == "root" || slug == "all" {
            continue;
        }
        let mut id = slug.clone();
        let mut i = 2;
        while seen_ids.contains(&id) {
            id = format!("{slug}_{i}");
            i += 1;
        }
        let regions: Vec<u32> = p
            .regions
            .into_iter()
            .filter(|r| *r >= 1 && *r <= region_count as u32 && used_regions.insert(*r))
            .collect();
        if regions.is_empty() {
            continue;
        }
        let name = if p.name.trim().is_empty() { id.clone() } else { p.name.trim().to_string() };
        seen_ids.push(id.clone());
        out.push(LabeledPart { id, name, region_ids: regions });
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Union the assigned regions into one source-dims mask per part.
pub fn part_mask(regions: &[Region], ids: &[u32]) -> GrayImage {
    let (w, h) = regions[0].mask.dimensions();
    let mut out = GrayImage::new(w, h);
    for rid in ids {
        if let Some(r) = regions.iter().find(|r| r.id == *rid) {
            for (dst, src) in out.pixels_mut().zip(r.mask.pixels()) {
                if src.0[0] > 127 {
                    dst.0[0] = 255;
                }
            }
        }
    }
    out
}

/// Connectivity repair across ALL part masks: a part keeps its largest connected
/// component; every other fragment is reassigned to the neighbouring part it touches
/// most (a mislabeled thumb glued to a "gear" moves to the hand it borders; a stray
/// knob joins the arm it sits on). Fragments touching nothing stay where they are.
pub fn repair_parts(masks: &mut [(String, GrayImage)]) {
    if masks.len() < 2 {
        return;
    }
    let (w, h) = masks[0].1.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut bits: Vec<Vec<bool>> = masks
        .iter()
        .map(|(_, m)| m.pixels().map(|p| p.0[0] > 127).collect())
        .collect();

    for pi in 0..bits.len() {
        // Label this part's connected components (4-neighbour).
        let part = bits[pi].clone();
        let mut labels = vec![0u32; part.len()];
        let mut comps: Vec<Vec<usize>> = Vec::new();
        for start in 0..part.len() {
            if !part[start] || labels[start] != 0 {
                continue;
            }
            let id = comps.len() as u32 + 1;
            let mut comp = Vec::new();
            let mut stack = vec![start];
            labels[start] = id;
            while let Some(i) = stack.pop() {
                comp.push(i);
                let (x, y) = (i % wu, i / wu);
                let visit = |ni: usize, labels: &mut [u32], stack: &mut Vec<usize>| {
                    if part[ni] && labels[ni] == 0 {
                        labels[ni] = id;
                        stack.push(ni);
                    }
                };
                if x > 0 {
                    visit(i - 1, &mut labels, &mut stack);
                }
                if x + 1 < wu {
                    visit(i + 1, &mut labels, &mut stack);
                }
                if y > 0 {
                    visit(i - wu, &mut labels, &mut stack);
                }
                if y + 1 < hu {
                    visit(i + wu, &mut labels, &mut stack);
                }
            }
            comps.push(comp);
        }
        if comps.len() <= 1 {
            continue;
        }
        let main = comps
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| c.len())
            .map(|(i, _)| i)
            .unwrap();

        for (ci, comp) in comps.iter().enumerate() {
            if ci == main {
                continue;
            }
            // Who borders this fragment? Dilate it a little and count overlap per part.
            let mut frag = vec![false; part.len()];
            for &i in comp {
                frag[i] = true;
            }
            matte::dilate(&mut frag, wu, hu, 3);
            let mut best: Option<(usize, usize)> = None; // (part, overlap)
            for (oi, other) in bits.iter().enumerate() {
                if oi == pi {
                    continue;
                }
                let overlap = frag.iter().zip(other).filter(|(f, o)| **f && **o).count();
                if overlap > 0 && best.map(|(_, b)| overlap > b).unwrap_or(true) {
                    best = Some((oi, overlap));
                }
            }
            let threshold = (comp.len() / 50).max(12);
            if let Some((target, overlap)) = best {
                if overlap >= threshold {
                    for &i in comp {
                        bits[pi][i] = false;
                        bits[target][i] = true;
                    }
                }
            }
        }
    }

    for (mi, (_, mask)) in masks.iter_mut().enumerate() {
        for (px, &b) in mask.pixels_mut().zip(bits[mi].iter()) {
            px.0[0] = if b { 255 } else { 0 };
        }
    }
}

/// Polish a unioned part mask to click-path quality: morphological close heals the seams
/// between unioned regions, capped hole-fill kills speckle, and the guided filter snaps
/// the boundary onto the image's actual color edges (same treatment SAM clicks get).
pub fn polish_mask(mask: &GrayImage, rgb: &RgbImage, alpha: &GrayImage) -> GrayImage {
    let (w, h) = mask.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut bits: Vec<bool> = mask.pixels().map(|p| p.0[0] > 127).collect();
    matte::dilate(&mut bits, wu, hu, 2);
    matte::erode(&mut bits, wu, hu, 2);
    matte::fill_holes(&mut bits, wu, hu, Some(((w * h) / 2000).max(64) as usize));

    let soft: Vec<f32> = bits.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let r = ((w.max(h) as usize) / 200).clamp(4, 12);
    let snapped = matte::guided_filter_rgb(&soft, rgb, r, 1e-4);

    let mut out = GrayImage::new(w, h);
    for ((px, &v), a) in out.pixels_mut().zip(snapped.iter()).zip(alpha.pixels()) {
        px.0[0] = if v > 0.5 && a.0[0] > 8 { 255 } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: u32, w: u32, h: u32, r: Rect) -> Region {
        let mask = GrayImage::from_fn(w, h, |x, y| {
            let inside = (x as i32) >= r.x
                && (y as i32) >= r.y
                && (x as i32) < r.x + r.w as i32
                && (y as i32) < r.y + r.h as i32;
            image::Luma([if inside { 255 } else { 0 }])
        });
        let area = mask.pixels().filter(|p| p.0[0] > 127).count() as u32;
        Region { id, mask, area, bbox: r }
    }

    #[test]
    fn clamp_dedupes_ids_and_region_claims() {
        let draft = LabelDraft {
            parts: vec![
                LabelPartDraft { id: "head".into(), name: "Head".into(), regions: vec![1, 2] },
                LabelPartDraft { id: "head".into(), name: "Head 2".into(), regions: vec![2, 3] },
                LabelPartDraft { id: "".into(), name: "".into(), regions: vec![4] },
                LabelPartDraft { id: "ghost".into(), name: "Ghost".into(), regions: vec![99] },
            ],
        };
        let out = clamp_labels(draft, 5);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "head");
        assert_eq!(out[0].region_ids, vec![1, 2]);
        assert_eq!(out[1].id, "head_2");
        assert_eq!(out[1].region_ids, vec![3], "region 2 already claimed; 99 out of range");
    }

    #[test]
    fn part_mask_unions_regions() {
        let regions = vec![
            region(1, 40, 40, Rect { x: 0, y: 0, w: 10, h: 10 }),
            region(2, 40, 40, Rect { x: 20, y: 20, w: 10, h: 10 }),
        ];
        let m = part_mask(&regions, &[1, 2]);
        assert_eq!(m.get_pixel(5, 5).0[0], 255);
        assert_eq!(m.get_pixel(25, 25).0[0], 255);
        assert_eq!(m.get_pixel(15, 15).0[0], 0);
    }

    #[test]
    fn repair_moves_stray_fragment_to_touching_part() {
        let (w, h) = (60u32, 40u32);
        let rect = |r: Rect| {
            GrayImage::from_fn(w, h, |x, y| {
                let inside = (x as i32) >= r.x
                    && (y as i32) >= r.y
                    && (x as i32) < r.x + r.w as i32
                    && (y as i32) < r.y + r.h as i32;
                image::Luma([if inside { 255 } else { 0 }])
            })
        };
        // "hand" = big block. "gear" = its own block on the right PLUS a mislabeled
        // fragment glued to the hand (the thumb) far from the gear.
        let hand = rect(Rect { x: 2, y: 2, w: 20, h: 30 });
        let mut gear = rect(Rect { x: 40, y: 2, w: 15, h: 30 });
        for y in 5..15 {
            for x in 22..28 {
                gear.put_pixel(x, y, image::Luma([255])); // "thumb" — touches hand
            }
        }
        let mut masks = vec![("hand".to_string(), hand), ("gear".to_string(), gear)];
        repair_parts(&mut masks);
        // Thumb fragment moved into hand; gear keeps its own block.
        assert_eq!(masks[0].1.get_pixel(24, 10).0[0], 255, "thumb joined hand");
        assert_eq!(masks[1].1.get_pixel(24, 10).0[0], 0, "thumb left gear");
        assert_eq!(masks[1].1.get_pixel(45, 10).0[0], 255, "gear body intact");
        // A fragment touching nothing stays put.
        let mut lonely = vec![
            ("a".to_string(), rect(Rect { x: 2, y: 2, w: 10, h: 10 })),
            ("b".to_string(), {
                let mut m = rect(Rect { x: 40, y: 2, w: 10, h: 10 });
                m.put_pixel(30, 35, image::Luma([255])); // isolated speck, touches nobody
                m
            }),
        ];
        repair_parts(&mut lonely);
        assert_eq!(lonely[1].1.get_pixel(30, 35).0[0], 255, "isolated fragment stays");
    }

    #[test]
    fn overlay_renders_and_encodes() {
        let source = RgbaImage::from_pixel(80, 60, image::Rgba([90, 90, 100, 255]));
        let regions = vec![
            region(1, 80, 60, Rect { x: 2, y: 2, w: 30, h: 30 }),
            region(12, 80, 60, Rect { x: 40, y: 10, w: 35, h: 40 }),
        ];
        let png = overlay_png(&source, &regions).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (80, 60));
    }
}
