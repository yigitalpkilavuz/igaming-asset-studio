//! Occlusion completion: when a part rotates away, whatever it covered must exist
//! underneath. For part P we inpaint ONLY the plausible continuation band
//! `Z_P = dilate(mask_P, r) ∩ union(masks of parts drawn ABOVE P)` via a gpt-image-2
//! masked edit, then composite with the ANTI-DRIFT rule: original cut pixels always win;
//! generated pixels appear only inside `Z_P`. Results are cached as
//! `completed.<hash8>.png` keyed by everything that affects the outcome (own mask, masks
//! above, prompt, band radius), so mask corrections invalidate exactly the parts whose
//! hidden band changed.

use std::path::Path;

use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::doc::{Rect, StudioDoc};
use super::{matte, sha256_hex, store};

const CHROMA: (u8, u8, u8) = (255, 0, 255);
const CHROMA_TOL: f32 = 0.18; // same as the processing pipeline
const CROP_PAD: u32 = 16;

// ── Planning ───────────────────────────────────────────────────────────────────

/// Everything needed to run one part's inpaint, computed up front (immutable, so jobs can
/// run concurrently).
pub struct InpaintJob {
    pub part_id: String,
    /// 8-hex cache key.
    pub hash: String,
    /// Region sent to the API, in source coords.
    pub crop: Rect,
    /// Cut pixels over magenta, crop-sized.
    pub base_png: Vec<u8>,
    /// Same-size PNG; transparent exactly over the zone (OpenAI edits contract).
    pub mask_png: Vec<u8>,
    /// Crop-local zone bitmap (where generated pixels are allowed).
    pub zone: GrayImage,
    pub prompt: String,
}

/// The band radius for a part: explicit setting, or 10% of the smaller bbox side.
fn band_radius(doc: &StudioDoc, bbox: Rect) -> u32 {
    if doc.settings.band_radius > 0.5 {
        return doc.settings.band_radius.round() as u32;
    }
    ((bbox.w.min(bbox.h) as f64) * 0.10).clamp(8.0, 48.0) as u32
}

/// `dilate(own, r) ∩ union(above)`, full source size. `None` when empty (unoccluded part).
pub fn compute_zone(own: &GrayImage, above: &[&GrayImage], r: u32) -> Option<GrayImage> {
    let (w, h) = own.dimensions();
    if above.is_empty() {
        return None;
    }
    // CRUMB HYGIENE: masks left over from imperfect part claiming litter tiny specks
    // across OTHER parts' territory. Dilating those specks bridges them into a zone
    // that can cover entire neighbouring parts — and the model then "completes" the
    // part by repainting whole arms and legs into it (observed on a real mascot: the
    // body's fill regrew the full character). Only the part's REAL silhouette may
    // seed the band, so drop connected components below 1% of the largest one.
    let cleaned = drop_specks(own);
    let mut dil: Vec<bool> = cleaned.pixels().map(|p| p.0[0] > 127).collect();
    dilate(&mut dil, w as usize, h as usize, r as usize);

    let mut any = false;
    let mut out = GrayImage::new(w, h);
    let mut zone_px = 0u64;
    let mut own_px = 0u64;
    for (i, px) in out.pixels_mut().enumerate() {
        if own.as_raw()[i] > 127 {
            own_px += 1;
        }
        let covered = above.iter().any(|m| m.as_raw()[i] > 127);
        // The band is everything near/within P that sits under a part drawn above it:
        // P's own mask pixels lost to overlap claiming AND the joint-overlap extension.
        // P's VISIBLE pixels are safe regardless — the composite draws them on top.
        let on = dil[i] && covered;
        if on {
            any = true;
            zone_px += 1;
            px.0[0] = 255;
        }
    }
    // SIZE GUARD: a band bigger than 60% of the part itself is not occlusion
    // completion any more — it is an invitation to repaint the neighbours. That much
    // hidden area means the masks are broken; surface it instead of generating junk.
    if zone_px > own_px.max(1) * 6 / 10 {
        return None;
    }
    any.then_some(out)
}

/// Keep only connected components ≥ 1% of the largest one (min 32 px) — the part's
/// true silhouette (possibly multi-piece), never claiming residue.
fn drop_specks(m: &GrayImage) -> GrayImage {
    let (w, h) = m.dimensions();
    let (w_us, h_us) = (w as usize, h as usize);
    let bits: Vec<bool> = m.pixels().map(|p| p.0[0] > 127).collect();
    let mut label = vec![0u32; bits.len()];
    let mut sizes: Vec<u64> = vec![0]; // sizes[0] unused
    let mut next = 1u32;
    let mut stack = Vec::new();
    for start in 0..bits.len() {
        if !bits[start] || label[start] != 0 {
            continue;
        }
        let id = next;
        next += 1;
        sizes.push(0);
        stack.push(start);
        label[start] = id;
        while let Some(i) = stack.pop() {
            sizes[id as usize] += 1;
            let (x, y) = (i % w_us, i / w_us);
            let mut push = |j: usize| {
                if bits[j] && label[j] == 0 {
                    label[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 { push(i - 1); }
            if x + 1 < w_us { push(i + 1); }
            if y > 0 { push(i - w_us); }
            if y + 1 < h_us { push(i + w_us); }
        }
    }
    let largest = sizes.iter().copied().max().unwrap_or(0);
    let floor = (largest / 100).max(32);
    let mut out = GrayImage::new(w, h);
    for (i, px) in out.pixels_mut().enumerate() {
        if label[i] != 0 && sizes[label[i] as usize] >= floor {
            px.0[0] = 255;
        }
    }
    out
}

use matte::dilate;

fn gray_bbox(m: &GrayImage) -> Option<Rect> {
    matte::mask_bbox(m)
}

fn union_bbox(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w as i32).max(b.x + b.w as i32);
    let y1 = (a.y + a.h as i32).max(b.y + b.h as i32);
    Rect { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
}

fn pad_clamp(r: Rect, pad: u32, w: u32, h: u32) -> Rect {
    let x0 = (r.x - pad as i32).max(0);
    let y0 = (r.y - pad as i32).max(0);
    let x1 = ((r.x + r.w as i32) + pad as i32).min(w as i32);
    let y1 = ((r.y + r.h as i32) + pad as i32).min(h as i32);
    Rect { x: x0, y: y0, w: (x1 - x0).max(1) as u32, h: (y1 - y0).max(1) as u32 }
}

fn inpaint_prompt(part_name: &str) -> String {
    format!(
        "This image shows only the {part_name} of a 2D game character, cut out on a flat \
magenta (#FF00FF) background — the character's other parts that covered it were removed. \
Fill ONLY the transparent regions of the mask: continue the {part_name}'s existing shapes, \
colours, shading, outline style and materials naturally, completing what was hidden behind \
the removed parts. Do not alter any visible pixel. Do not add other body parts, objects or \
text. Everything that is not the {part_name} stays flat magenta, with crisp edges and no \
halos."
    )
}

/// Everything [`plan_fill`] needs to build one fill job, doc- and file-agnostic.
/// `hash_extra` should uniquely capture the mask geometry (e.g. own mask PNG bytes
/// followed by the above-mask PNG bytes) — prompt and radius are appended internally.
pub struct FillSpec<'a> {
    /// The piece's author mask (studio) or its claimed pixels (layers catch-all).
    pub own_mask: &'a GrayImage,
    pub above_masks: &'a [&'a GrayImage],
    /// The piece's cut texture and its bbox in source coords.
    pub cut: &'a RgbaImage,
    pub cut_bbox: Rect,
    pub radius: u32,
    pub prompt: String,
    pub hash_extra: &'a [u8],
}

/// Build a fill job from an in-memory spec, or `None` if nothing occludes the piece.
pub fn plan_fill(id: &str, spec: &FillSpec) -> Result<Option<InpaintJob>, String> {
    let Some(zone) = compute_zone(spec.own_mask, spec.above_masks, spec.radius) else {
        return Ok(None);
    };

    // Cache key: mask geometry ‖ prompt ‖ radius ‖ zone-algorithm version.
    let mut hash_input = spec.hash_extra.to_vec();
    hash_input.extend_from_slice(b"zone-v2"); // crumb hygiene + size guard
    hash_input.extend_from_slice(spec.prompt.as_bytes());
    hash_input.extend_from_slice(&spec.radius.to_le_bytes());
    let hash = sha256_hex(&hash_input)[..8].to_string();

    // Crop region: piece visible ∪ zone, padded.
    let (sw, sh) = spec.own_mask.dimensions();
    let zone_bb = gray_bbox(&zone).unwrap();
    let crop = pad_clamp(union_bbox(spec.cut_bbox, zone_bb), CROP_PAD, sw, sh);

    // Base: the piece's cut pixels over magenta, crop-local.
    let bbox = spec.cut_bbox;
    let mut base_img =
        RgbaImage::from_pixel(crop.w, crop.h, image::Rgba([CHROMA.0, CHROMA.1, CHROMA.2, 255]));
    for (x, y, p) in spec.cut.enumerate_pixels() {
        if p.0[3] == 0 {
            continue;
        }
        let sx = bbox.x + x as i32 - crop.x;
        let sy = bbox.y + y as i32 - crop.y;
        if sx >= 0 && sy >= 0 && (sx as u32) < crop.w && (sy as u32) < crop.h {
            // Flatten over magenta (kill semi-transparency; the key pass restores alpha).
            let a = p.0[3] as u32;
            let blend = |c: u8, k: u8| ((c as u32 * a + k as u32 * (255 - a)) / 255) as u8;
            base_img.put_pixel(
                sx as u32,
                sy as u32,
                image::Rgba([blend(p.0[0], CHROMA.0), blend(p.0[1], CHROMA.1), blend(p.0[2], CHROMA.2), 255]),
            );
        }
    }

    // Crop-local zone + the API mask (transparent over the zone).
    let mut zone_crop = GrayImage::new(crop.w, crop.h);
    let mut mask_img = RgbaImage::from_pixel(crop.w, crop.h, image::Rgba([255, 255, 255, 255]));
    for y in 0..crop.h {
        for x in 0..crop.w {
            let gx = (crop.x + x as i32) as u32;
            let gy = (crop.y + y as i32) as u32;
            if gx < sw && gy < sh && zone.get_pixel(gx, gy).0[0] > 127 {
                zone_crop.put_pixel(x, y, image::Luma([255]));
                mask_img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
    }

    Ok(Some(InpaintJob {
        part_id: id.to_string(),
        hash,
        crop,
        base_png: encode_png_rgba(&base_img)?,
        mask_png: encode_png_rgba(&mask_img)?,
        zone: zone_crop,
        prompt: spec.prompt.clone(),
    }))
}

/// Build the inpaint job for one part, or `None` if nothing occludes it.
/// Loads masks from disk; pure aside from reads (testable with a temp base).
pub fn plan(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
    part_id: &str,
) -> Result<Option<InpaintJob>, String> {
    let idx = doc
        .parts
        .iter()
        .position(|p| p.id == part_id)
        .ok_or_else(|| format!("unknown part \"{part_id}\""))?;
    let part = &doc.parts[idx];
    let bbox = part
        .bbox
        .ok_or_else(|| format!("part \"{part_id}\" is not cut yet"))?;

    let load_mask = |id: &str| -> Result<GrayImage, String> {
        image::open(store::part_dir(base, game_id, asset_key, id).join("mask.png"))
            .map(|i| i.to_luma8())
            .map_err(|e| format!("open mask for {id}: {e}"))
    };
    let own = load_mask(part_id)?;
    let mut above_imgs: Vec<GrayImage> = Vec::new();
    let mut hash_extra = std::fs::read(
        store::part_dir(base, game_id, asset_key, part_id).join("mask.png"),
    )
    .map_err(|e| format!("read own mask: {e}"))?;
    for p in doc.parts.iter().skip(idx + 1) {
        let path = store::part_dir(base, game_id, asset_key, &p.id).join("mask.png");
        if path.is_file() {
            hash_extra.extend_from_slice(
                &std::fs::read(&path).map_err(|e| format!("read mask for {}: {e}", p.id))?,
            );
            above_imgs.push(load_mask(&p.id)?);
        }
    }
    // Unoccluded → no job, before touching cut.png (a top-most part may not be cut yet).
    let radius = band_radius(doc, bbox);
    let above_refs: Vec<&GrayImage> = above_imgs.iter().collect();
    if compute_zone(&own, &above_refs, radius).is_none() {
        return Ok(None);
    }
    let cut = image::open(store::part_dir(base, game_id, asset_key, part_id).join("cut.png"))
        .map_err(|e| format!("open cut for {part_id}: {e}"))?
        .to_rgba8();

    plan_fill(
        part_id,
        &FillSpec {
            own_mask: &own,
            above_masks: &above_refs,
            cut: &cut,
            cut_bbox: bbox,
            radius,
            prompt: inpaint_prompt(&part.name),
            hash_extra: &hash_extra,
        },
    )
}

// ── Compositing ────────────────────────────────────────────────────────────────

/// Anti-drift merge: generated pixels only inside the zone, original cut pixels drawn on
/// top wherever they exist. Returns the completed texture and its bbox in source coords.
pub fn composite(
    job: &InpaintJob,
    cut: &RgbaImage,
    cut_bbox: Rect,
    generated_keyed: &RgbaImage,
) -> Result<(RgbaImage, Rect), String> {
    let (cw, ch) = (job.crop.w, job.crop.h);
    if generated_keyed.dimensions() != (cw, ch) {
        return Err("generated image has wrong dimensions after resize".into());
    }
    let mut out = RgbaImage::new(cw, ch);
    // 1. Generated, clipped to the zone.
    for y in 0..ch {
        for x in 0..cw {
            if job.zone.get_pixel(x, y).0[0] > 127 {
                out.put_pixel(x, y, *generated_keyed.get_pixel(x, y));
            }
        }
    }
    // 2. Original cut pixels on top (they always win; their antialiased edges blend over
    //    the generated band naturally).
    for (x, y, p) in cut.enumerate_pixels() {
        if p.0[3] == 0 {
            continue;
        }
        let dx = cut_bbox.x + x as i32 - job.crop.x;
        let dy = cut_bbox.y + y as i32 - job.crop.y;
        if dx < 0 || dy < 0 || dx as u32 >= cw || dy as u32 >= ch {
            continue;
        }
        let (dx, dy) = (dx as u32, dy as u32);
        let under = *out.get_pixel(dx, dy);
        out.put_pixel(dx, dy, over(*p, under));
    }
    // Trim to content.
    let alpha = GrayImage::from_fn(cw, ch, |x, y| image::Luma([out.get_pixel(x, y).0[3]]));
    let bb = gray_bbox(&alpha).ok_or_else(|| "completed texture is empty".to_string())?;
    let trimmed =
        image::imageops::crop_imm(&out, bb.x as u32, bb.y as u32, bb.w, bb.h).to_image();
    let source_bb = Rect { x: job.crop.x + bb.x, y: job.crop.y + bb.y, w: bb.w, h: bb.h };
    Ok((trimmed, source_bb))
}

/// Standard source-over for one pixel.
fn over(top: image::Rgba<u8>, under: image::Rgba<u8>) -> image::Rgba<u8> {
    let ta = top.0[3] as f32 / 255.0;
    let ua = under.0[3] as f32 / 255.0;
    let oa = ta + ua * (1.0 - ta);
    if oa <= 0.0 {
        return image::Rgba([0, 0, 0, 0]);
    }
    let ch = |t: u8, u: u8| {
        (((t as f32 * ta + u as f32 * ua * (1.0 - ta)) / oa).round() as u8).min(255)
    };
    image::Rgba([
        ch(top.0[0], under.0[0]),
        ch(top.0[1], under.0[1]),
        ch(top.0[2], under.0[2]),
        (oa * 255.0).round() as u8,
    ])
}

// ── Execution ──────────────────────────────────────────────────────────────────

/// Run one job through gpt-image-2 and composite (anti-drift: original pixels win).
/// Returns the completed texture and its source-coord bbox; the caller persists it.
pub async fn execute_fill(
    api_key: &str,
    job: &InpaintJob,
    cut: &RgbaImage,
    cut_bbox: Rect,
) -> Result<(RgbaImage, Rect), String> {
    let size = crate::providers::openai_image::edit_size(job.crop.w, job.crop.h);
    let out_png = crate::providers::openai_image::edit_image(
        api_key,
        &job.base_png,
        &job.prompt,
        &size,
        Some(&job.mask_png),
    )
    .await?;

    // Back to crop dims, key out the magenta, composite with the original.
    let gen = image::load_from_memory(&out_png).map_err(|e| format!("decode inpaint: {e}"))?;
    let gen = image::imageops::resize(
        &gen.to_rgba8(),
        job.crop.w,
        job.crop.h,
        image::imageops::FilterType::CatmullRom,
    );
    let keyed_png =
        crate::processing::chromakey::chroma_key(&encode_png_rgba(&gen)?, CHROMA, CHROMA_TOL)?;
    let keyed = image::load_from_memory(&keyed_png)
        .map_err(|e| format!("decode keyed: {e}"))?
        .to_rgba8();

    composite(job, cut, cut_bbox, &keyed)
}

/// Run one job through gpt-image-2 and write `completed.<hash8>.png`.
/// Returns the completed texture's source-coord bbox.
pub async fn run_job(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    api_key: &str,
    job: &InpaintJob,
    cut_bbox: Rect,
) -> Result<Rect, String> {
    let cut = image::open(
        store::part_dir(base, game_id, asset_key, &job.part_id).join("cut.png"),
    )
    .map_err(|e| format!("open cut: {e}"))?
    .to_rgba8();

    let (completed, bbox) = execute_fill(api_key, job, &cut, cut_bbox).await?;
    let path = store::part_dir(base, game_id, asset_key, &job.part_id)
        .join(format!("completed.{}.png", job.hash));
    store::write_file(&path, &encode_png_rgba(&completed)?)?;
    Ok(bbox)
}

// ── Status ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InpaintStatus {
    pub part_id: String,
    pub state: InpaintState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum InpaintState {
    /// Nothing occludes this part — no inpaint needed.
    Clear,
    /// Occluded, no completed texture yet.
    Pending,
    /// Completed texture matches the current masks.
    Fresh,
    /// Masks changed since the completed texture was generated.
    Stale,
}

/// Per-part inpaint state (recomputes the expected hash from the masks on disk).
pub fn status(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
) -> Vec<InpaintStatus> {
    doc.parts
        .iter()
        .filter(|p| p.bbox.is_some() && p.mask_hash.is_some())
        .map(|p| {
            let state = match plan(base, game_id, asset_key, doc, &p.id) {
                Ok(None) => InpaintState::Clear,
                Ok(Some(job)) => match &p.completed_hash {
                    Some(h) if *h == job.hash => InpaintState::Fresh,
                    Some(_) => InpaintState::Stale,
                    None => InpaintState::Pending,
                },
                Err(_) => InpaintState::Pending,
            };
            InpaintStatus { part_id: p.id.clone(), state }
        })
        .collect()
}

fn encode_png_rgba(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{Part, PartTexture, SourceRef};

    /// The real-world failure: crumbs left in the body mask scattered across the
    /// arm's territory turned the fill zone into "the whole arm", and the model
    /// repainted the limb into the body texture. Crumbs must not seed the band.
    #[test]
    fn crumbs_do_not_grow_the_zone_into_neighbour_parts() {
        let (w, h) = (200u32, 200u32);
        // Body: solid 80×180 block on the left…
        let mut body = GrayImage::from_fn(w, h, |x, _| image::Luma([if x < 80 { 255 } else { 0 }]));
        // …plus residue specks sprinkled inside the arm's territory (x 120–180).
        for i in 0..30u32 {
            let (x, y) = (120 + (i * 7) % 60, 10 + (i * 13) % 180);
            body.put_pixel(x, y, image::Luma([255]));
        }
        // Arm: solid 60×180 block on the right, drawn ABOVE the body.
        let arm = GrayImage::from_fn(w, h, |x, _| image::Luma([if (120..180).contains(&x) { 255 } else { 0 }]));

        // The body's true silhouette (x<80, dilated to <92) never touches the arm
        // (x≥120): with the crumbs ignored there is NO occlusion band at all.
        assert!(
            compute_zone(&body, &[&arm], 12).is_none(),
            "crumb-seeded fill must not cover the arm"
        );

        // Control: a body that genuinely reaches under the arm keeps its band.
        let touching =
            GrayImage::from_fn(w, h, |x, _| image::Luma([if x < 130 { 255 } else { 0 }]));
        let zone = compute_zone(&touching, &[&arm], 12).expect("real joint keeps its band");
        let band: Vec<u32> = zone
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0[0] > 127)
            .map(|(x, _, _)| x)
            .collect();
        assert!(band.iter().all(|&x| (120..143).contains(&x)), "band hugs the true overlap");
    }

    /// A band bigger than 60% of the part itself means the masks are broken —
    /// refuse to plan a fill instead of inviting a full repaint.
    #[test]
    fn oversized_zones_are_refused() {
        let (w, h) = (100u32, 100u32);
        let own = GrayImage::from_fn(w, h, |x, y| {
            image::Luma([if x < 40 && y < 40 { 255 } else { 0 }])
        });
        // An occluder covering almost everything, including all of `own`.
        let above = GrayImage::from_fn(w, h, |_, _| image::Luma([255]));
        assert!(compute_zone(&own, &[&above], 12).is_none(), "zone > 60% of part must refuse");
    }

    fn rect_mask(w: u32, h: u32, r: Rect) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let inside = (x as i32) >= r.x
                && (y as i32) >= r.y
                && (x as i32) < r.x + r.w as i32
                && (y as i32) < r.y + r.h as i32;
            image::Luma([if inside { 255 } else { 0 }])
        })
    }

    #[test]
    fn zone_is_band_under_covering_parts_only() {
        let (w, h) = (100u32, 80u32);
        // Own part: left half. Above part: a strip overlapping the middle.
        let own = rect_mask(w, h, Rect { x: 0, y: 0, w: 50, h: 80 });
        let above = rect_mask(w, h, Rect { x: 40, y: 30, w: 40, h: 20 });
        let zone = compute_zone(&own, &[&above], 8).unwrap();
        // Inside the band: just right of the own edge, under the above strip.
        assert_eq!(zone.get_pixel(53, 40).0[0], 255);
        // Own pixels hidden under the above strip are part of the band too.
        assert_eq!(zone.get_pixel(45, 40).0[0], 255);
        // Own visible pixels (not covered) are not in the band.
        assert_eq!(zone.get_pixel(30, 40).0[0], 0);
        // Not outside the covering part.
        assert_eq!(zone.get_pixel(53, 60).0[0], 0);
        // Not beyond the dilation radius.
        assert_eq!(zone.get_pixel(70, 40).0[0], 0);
        // Unoccluded part → None.
        assert!(compute_zone(&own, &[], 8).is_none());
        let far = rect_mask(w, h, Rect { x: 90, y: 0, w: 10, h: 10 });
        assert!(compute_zone(&own, &[&far], 8).is_none());
    }

    #[test]
    fn plan_and_composite_preserve_originals_and_fill_zone_only() {
        let base = std::env::temp_dir().join(format!("wf_inpaint_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (gid, key) = ("g", "sym");
        let (w, h) = (100u32, 80u32);

        // Doc: part "back" (left half) under part "front" (middle strip).
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: w, height: h, sha256: "s".into() },
            0.0,
        );
        let mk = |id: &str, bbox: Rect| Part {
            id: id.into(),
            name: id.into(),
            prompts: vec![],
            bbox: Some(bbox),
            mask_hash: Some("m".into()),
            completed_hash: None,
            completed_bbox: None,
            texture: PartTexture::Cut,
        };
        let back_bbox = Rect { x: 0, y: 0, w: 40, h: 80 }; // claimed pixels (front took overlap)
        doc.parts = vec![
            mk("back", back_bbox),
            mk("front", Rect { x: 40, y: 30, w: 40, h: 20 }),
        ];

        // Masks on disk: back's FULL mask extends to x<50 (overlap 40..50 hidden).
        let write = |name: &str, img: &GrayImage| {
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(img.clone())
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            store::write_file(
                &store::part_dir(&base, gid, key, name).join("mask.png"),
                &buf.into_inner(),
            )
            .unwrap();
        };
        write("back", &rect_mask(w, h, Rect { x: 0, y: 0, w: 50, h: 80 }));
        write("front", &rect_mask(w, h, Rect { x: 40, y: 30, w: 40, h: 20 }));

        // back's cut.png: solid blue at its claimed bbox.
        let cut = RgbaImage::from_pixel(back_bbox.w, back_bbox.h, image::Rgba([0, 0, 200, 255]));
        store::write_file(
            &store::part_dir(&base, gid, key, "back").join("cut.png"),
            &encode_png_rgba(&cut).unwrap(),
        )
        .unwrap();

        let job = plan(&base, gid, key, &doc, "back").unwrap().expect("occluded → job");
        // Front (top-most) has nothing above it → no job.
        assert!(plan(&base, gid, key, &doc, "front").unwrap().is_none());

        // Deterministic hash: replanning yields the same key.
        let job2 = plan(&base, gid, key, &doc, "back").unwrap().unwrap();
        assert_eq!(job.hash, job2.hash);

        // Fake a "generated" result: solid green everywhere.
        let gen = RgbaImage::from_pixel(job.crop.w, job.crop.h, image::Rgba([0, 200, 0, 255]));
        let (completed, bb) = composite(&job, &cut, back_bbox, &gen).unwrap();

        // Original pixels survive verbatim…
        let px = completed.get_pixel((10 - bb.x) as u32, (10 - bb.y) as u32);
        assert_eq!(px.0, [0, 0, 200, 255], "original cut pixel preserved");
        // …generated fills only the hidden band (x 40..50 under front strip, y 30..50).
        let gx = (45 - bb.x) as u32;
        let gy = (40 - bb.y) as u32;
        assert_eq!(completed.get_pixel(gx, gy).0, [0, 200, 0, 255], "band filled");
        // Outside the zone and outside the cut → transparent. Source (45, 65): within the
        // trimmed texture, in neither the cut (x ≥ 40) nor the band (front only covers
        // y 30..50, and dilation r is far smaller than the 15px gap).
        let ox = (45 - bb.x) as u32;
        let oy = (65 - bb.y) as u32;
        assert_eq!(completed.get_pixel(ox, oy).0[3], 0, "outside zone stays empty");

        // Status: back pending, front clear.
        let st = status(&base, gid, key, &doc);
        assert_eq!(
            st.iter().find(|s| s.part_id == "back").unwrap().state,
            InpaintState::Pending
        );
        assert_eq!(
            st.iter().find(|s| s.part_id == "front").unwrap().state,
            InpaintState::Clear
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
