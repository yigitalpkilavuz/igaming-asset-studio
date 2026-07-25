//! Rig-agnostic matte machinery shared by the animation studio's part cutting and the
//! parallax layer splitter: binary morphology, SAM-mask refinement, and the exclusive
//! top-wins cut (fringe + feather + optional sliver rescue). Nothing in here knows about
//! docs, skeletons or files — callers own persistence and bookkeeping.

use image::{GrayImage, RgbaImage};

use super::doc::Rect;

/// One piece lifted out of the source by [`exclusive_cut`].
pub struct CutPiece {
    pub id: String,
    /// Tight bbox of the piece in source coords.
    pub bbox: Rect,
    /// Mean of the piece's exclusively-claimed pixels (source coords).
    pub centroid: (f64, f64),
    /// RGBA texture, cropped to `bbox`.
    pub image: RgbaImage,
}

pub struct CutOptions {
    /// Duplicated band (px) extending UNDER pieces drawn above — hidden at rest, hides
    /// the razor seam the moment pieces move.
    pub seam_fringe: u32,
    /// Adopt unclaimed opaque source pixels into the nearest piece (multi-source BFS).
    /// Pointless when a catch-all bottom mask claims everything.
    pub rescue_unclaimed: bool,
}

/// Lift every mask out of `source` with exclusive pixel ownership: overlapping pixels are
/// claimed by the LAST mask in the slice (slice order = draw order, last = front). Each
/// piece's matte is its claim plus a `seam_fringe` band under higher pieces, softened 1 px
/// so interior edges are anti-aliased. Fringe and feather sample the SAME source pixels,
/// so the reassembled stack matches the source. Masks whose claim ends up empty (fully
/// occluded, or covering only transparent source) yield no piece.
pub fn exclusive_cut(
    source: &RgbaImage,
    masks: &[(String, GrayImage)],
    opts: &CutOptions,
) -> Result<Vec<CutPiece>, String> {
    let (w, h) = source.dimensions();
    let n = (w * h) as usize;
    for (id, m) in masks {
        if m.dimensions() != (w, h) {
            return Err(format!(
                "mask for \"{id}\" is {}×{}, expected source dims {w}×{h}",
                m.width(),
                m.height()
            ));
        }
    }
    if masks.is_empty() {
        return Err("no masks to cut".into());
    }

    // Exclusive claim, front-most (highest index) wins.
    let mut claim = vec![u32::MAX; n];
    for (pi, (_, mask)) in masks.iter().enumerate().rev() {
        for (i, p) in mask.pixels().enumerate() {
            if p.0[0] > 127 && claim[i] == u32::MAX {
                claim[i] = pi as u32;
            }
        }
    }

    // Rescue unclaimed art: opaque source pixels no mask covered adopt the NEAREST
    // claimed piece — the reassembled stack never silently loses pixels.
    if opts.rescue_unclaimed {
        let (wu, hu) = (w as usize, h as usize);
        let mut near = claim.clone();
        let mut queue: std::collections::VecDeque<usize> = near
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != u32::MAX)
            .map(|(i, _)| i)
            .collect();
        while let Some(i) = queue.pop_front() {
            let label = near[i];
            let (x, y) = (i % wu, i / wu);
            let mut visit = |ni: usize| {
                if near[ni] == u32::MAX {
                    near[ni] = label;
                    queue.push_back(ni);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < wu {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - wu);
            }
            if y + 1 < hu {
                visit(i + wu);
            }
        }
        for (i, c) in claim.iter_mut().enumerate() {
            if *c == u32::MAX && near[i] != u32::MAX {
                let (x, y) = ((i % wu) as u32, (i / wu) as u32);
                if source.get_pixel(x, y).0[3] > 8 {
                    *c = near[i];
                }
            }
        }
    }

    let fringe = opts.seam_fringe as usize;
    let mut pieces = Vec::new();
    for (pi, (id, _)) in masks.iter().enumerate() {
        let mut own = vec![false; n];
        let (mut sx, mut sy, mut count) = (0f64, 0f64, 0u64);
        for (i, c) in claim.iter().enumerate() {
            if *c == pi as u32 {
                own[i] = true;
                sx += ((i as u32) % w) as f64;
                sy += ((i as u32) / w) as f64;
                count += 1;
            }
        }
        if count == 0 {
            continue; // fully occluded by pieces above
        }

        // Fringe: extend under HIGHER-drawn neighbours only (their pixels cover ours).
        let mut matte_bits = own.clone();
        if fringe > 0 {
            let mut dil = own.clone();
            dilate(&mut dil, w as usize, h as usize, fringe);
            for (i, c) in claim.iter().enumerate() {
                if dil[i] && *c != u32::MAX && *c > pi as u32 {
                    matte_bits[i] = true;
                }
            }
        }
        let matte = soften(&matte_bits, w as usize, h as usize);

        let mut cut = RgbaImage::new(w, h);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
        for (i, &m) in matte.iter().enumerate() {
            if m <= 0.004 {
                continue;
            }
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            let src = *source.get_pixel(x, y);
            let a = (src.0[3] as f32 * m).round() as u8;
            if a == 0 {
                continue;
            }
            cut.put_pixel(x, y, image::Rgba([src.0[0], src.0[1], src.0[2], a]));
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        if x0 > x1 {
            continue; // matte covered only transparent source pixels
        }
        let bbox = Rect { x: x0 as i32, y: y0 as i32, w: x1 - x0 + 1, h: y1 - y0 + 1 };
        let image =
            image::imageops::crop_imm(&cut, bbox.x as u32, bbox.y as u32, bbox.w, bbox.h)
                .to_image();
        pieces.push(CutPiece {
            id: id.clone(),
            bbox,
            centroid: (sx / count as f64, sy / count as f64),
            image,
        });
    }
    Ok(pieces)
}

// ── Mask refinement ────────────────────────────────────────────────────────────

/// Deterministic cleanup of a raw SAM mask (already at SOURCE dims, 0/255):
/// binarize → ∩ source alpha → keep connected components containing a positive point
/// (largest if none does) → fill holes (≤ `hole_cap_px` if given; scenery keeps real
/// see-through gaps like arches and canopy holes) → morphological close (r=2).
pub fn refine(
    raw: &GrayImage,
    alpha: Option<&GrayImage>,
    positives_px: &[(u32, u32)],
    hole_cap_px: Option<u32>,
) -> GrayImage {
    let (w, h) = raw.dimensions();
    let mut mask: Vec<bool> = raw.pixels().map(|p| p.0[0] > 127).collect();
    if let Some(a) = alpha {
        debug_assert_eq!(a.dimensions(), (w, h));
        for (m, ap) in mask.iter_mut().zip(a.pixels()) {
            *m = *m && ap.0[0] > 8;
        }
    }

    // Connected components (4-neighbour), labels 1.., 0 = background.
    let mut labels = vec![0u32; mask.len()];
    let mut sizes: Vec<usize> = vec![0]; // sizes[label]
    let mut next = 1u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        let label = next;
        next += 1;
        let mut size = 0usize;
        stack.push(start);
        labels[start] = label;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            let mut visit = |nx: u32, ny: u32| {
                let ni = (ny * w + nx) as usize;
                if mask[ni] && labels[ni] == 0 {
                    labels[ni] = label;
                    stack.push(ni);
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if x + 1 < w {
                visit(x + 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if y + 1 < h {
                visit(x, y + 1);
            }
        }
        sizes.push(size);
    }

    // Which labels survive?
    let mut keep: Vec<bool> = vec![false; sizes.len()];
    let mut any = false;
    for &(px, py) in positives_px {
        if px < w && py < h {
            let l = labels[(py * w + px) as usize];
            if l != 0 {
                keep[l as usize] = true;
                any = true;
            }
        }
    }
    if !any {
        // No positive landed inside — keep the largest component instead.
        if let Some((l, _)) = sizes.iter().enumerate().skip(1).max_by_key(|(_, s)| **s) {
            keep[l] = true;
        }
    }
    let mut kept: Vec<bool> = labels
        .iter()
        .map(|&l| l != 0 && keep[l as usize])
        .collect();

    fill_holes(&mut kept, w as usize, h as usize, hole_cap_px.map(|c| c as usize));

    // Morphological close, radius 2 (separable box dilate then erode).
    dilate(&mut kept, w as usize, h as usize, 2);
    erode(&mut kept, w as usize, h as usize, 2);
    // Re-clip to alpha (close can bleed past the silhouette).
    if let Some(a) = alpha {
        for (m, ap) in kept.iter_mut().zip(a.pixels()) {
            *m = *m && ap.0[0] > 8;
        }
    }

    let mut out = GrayImage::new(w, h);
    for (px, &m) in out.pixels_mut().zip(kept.iter()) {
        px.0[0] = if m { 255 } else { 0 };
    }
    out
}

/// Tight bbox of set pixels; `None` for an empty mask.
pub fn mask_bbox(mask: &GrayImage) -> Option<Rect> {
    let (w, h) = mask.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    let mut any = false;
    for (x, y, p) in mask.enumerate_pixels() {
        if p.0[0] > 127 {
            any = true;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    any.then(|| Rect { x: x0 as i32, y: y0 as i32, w: x1 - x0 + 1, h: y1 - y0 + 1 })
}

// ── Edge-aware mask refinement (guided filter) ─────────────────────────────────

/// O(N) sliding-window box mean (independent of radius).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; src.len()];
    for y in 0..h {
        let row = y * w;
        let mut sum = 0f32;
        for x in 0..=r.min(w - 1) {
            sum += src[row + x];
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[row + x] = sum / (hi - lo + 1) as f32;
            if x + r + 1 < w {
                sum += src[row + x + r + 1];
            }
            if x >= r {
                sum -= src[row + x - r];
            }
        }
    }
    let mut out = vec![0f32; src.len()];
    for x in 0..w {
        let mut sum = 0f32;
        for y in 0..=r.min(h - 1) {
            sum += tmp[y * w + x];
        }
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            out[y * w + x] = sum / (hi - lo + 1) as f32;
            if y + r + 1 < h {
                sum += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                sum -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

/// Color-guided filter (He et al.): refine a SOFT mask so its edges snap onto the guide
/// image's color edges. This is what turns SAM's upscaled 1024-res boundary (soft and
/// stair-stepped) into a full-resolution edge that follows the actual silhouette.
/// `soft` in 0..1, RGB guide at the same dims; returns the filtered mask, clamped 0..1.
pub fn guided_filter_rgb(
    soft: &[f32],
    guide: &image::RgbImage,
    r: usize,
    eps: f32,
) -> Vec<f32> {
    let (w, h) = (guide.width() as usize, guide.height() as usize);
    debug_assert_eq!(soft.len(), w * h);
    let n = w * h;

    // Guide channels normalized 0..1.
    let mut cr = vec![0f32; n];
    let mut cg = vec![0f32; n];
    let mut cb = vec![0f32; n];
    for (i, p) in guide.pixels().enumerate() {
        cr[i] = p.0[0] as f32 / 255.0;
        cg[i] = p.0[1] as f32 / 255.0;
        cb[i] = p.0[2] as f32 / 255.0;
    }

    let mul = |a: &[f32], b: &[f32]| -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x * y).collect()
    };

    let m_r = box_mean(&cr, w, h, r);
    let m_g = box_mean(&cg, w, h, r);
    let m_b = box_mean(&cb, w, h, r);
    let m_p = box_mean(soft, w, h, r);

    let m_rp = box_mean(&mul(&cr, soft), w, h, r);
    let m_gp = box_mean(&mul(&cg, soft), w, h, r);
    let m_bp = box_mean(&mul(&cb, soft), w, h, r);

    let m_rr = box_mean(&mul(&cr, &cr), w, h, r);
    let m_rg = box_mean(&mul(&cr, &cg), w, h, r);
    let m_rb = box_mean(&mul(&cr, &cb), w, h, r);
    let m_gg = box_mean(&mul(&cg, &cg), w, h, r);
    let m_gb = box_mean(&mul(&cg, &cb), w, h, r);
    let m_bb = box_mean(&mul(&cb, &cb), w, h, r);

    let mut ar = vec![0f32; n];
    let mut ag = vec![0f32; n];
    let mut ab = vec![0f32; n];
    let mut b = vec![0f32; n];
    for i in 0..n {
        // Covariances.
        let vrr = m_rr[i] - m_r[i] * m_r[i] + eps;
        let vrg = m_rg[i] - m_r[i] * m_g[i];
        let vrb = m_rb[i] - m_r[i] * m_b[i];
        let vgg = m_gg[i] - m_g[i] * m_g[i] + eps;
        let vgb = m_gb[i] - m_g[i] * m_b[i];
        let vbb = m_bb[i] - m_b[i] * m_b[i] + eps;
        let crp = m_rp[i] - m_r[i] * m_p[i];
        let cgp = m_gp[i] - m_g[i] * m_p[i];
        let cbp = m_bp[i] - m_b[i] * m_p[i];

        // Solve (Σ + εI) a = cov(I,p) via the symmetric 3×3 adjugate.
        let d00 = vgg * vbb - vgb * vgb;
        let d01 = vrb * vgb - vrg * vbb;
        let d02 = vrg * vgb - vrb * vgg;
        let det = vrr * d00 + vrg * d01 + vrb * d02;
        if det.abs() < 1e-12 {
            b[i] = m_p[i];
            continue;
        }
        let inv = 1.0 / det;
        let d11 = vrr * vbb - vrb * vrb;
        let d12 = vrb * vrg - vrr * vgb;
        let d22 = vrr * vgg - vrg * vrg;
        ar[i] = (d00 * crp + d01 * cgp + d02 * cbp) * inv;
        ag[i] = (d01 * crp + d11 * cgp + d12 * cbp) * inv;
        ab[i] = (d02 * crp + d12 * cgp + d22 * cbp) * inv;
        b[i] = m_p[i] - ar[i] * m_r[i] - ag[i] * m_g[i] - ab[i] * m_b[i];
    }

    let ma_r = box_mean(&ar, w, h, r);
    let ma_g = box_mean(&ag, w, h, r);
    let ma_b = box_mean(&ab, w, h, r);
    let mb = box_mean(&b, w, h, r);

    (0..n)
        .map(|i| (ma_r[i] * cr[i] + ma_g[i] * cg[i] + ma_b[i] * cb[i] + mb[i]).clamp(0.0, 1.0))
        .collect()
}

// ── Binary morphology ──────────────────────────────────────────────────────────

/// Fill enclosed holes: flood the complement from the border; unreached = interior hole.
/// With `cap`, only holes of at most `cap` pixels are filled — larger ones are treated as
/// real see-through openings and kept.
pub(crate) fn fill_holes(mask: &mut [bool], w: usize, h: usize, cap: Option<usize>) {
    let mut outside = vec![false; mask.len()];
    let mut stack: Vec<usize> = Vec::new();
    let push = |i: usize, outside: &mut [bool], stack: &mut Vec<usize>| {
        if !mask[i] && !outside[i] {
            outside[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        push(x, &mut outside, &mut stack);
        push((h - 1) * w + x, &mut outside, &mut stack);
    }
    for y in 0..h {
        push(y * w, &mut outside, &mut stack);
        push(y * w + w - 1, &mut outside, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        if x > 0 {
            push(i - 1, &mut outside, &mut stack);
        }
        if x + 1 < w {
            push(i + 1, &mut outside, &mut stack);
        }
        if y > 0 {
            push(i - w, &mut outside, &mut stack);
        }
        if y + 1 < h {
            push(i + w, &mut outside, &mut stack);
        }
    }
    match cap {
        None => {
            for (i, m) in mask.iter_mut().enumerate() {
                if !*m && !outside[i] {
                    *m = true;
                }
            }
        }
        Some(cap) => {
            // Label each hole component; fill only the small ones.
            let mut seen = vec![false; mask.len()];
            let mut comp: Vec<usize> = Vec::new();
            for start in 0..mask.len() {
                if mask[start] || outside[start] || seen[start] {
                    continue;
                }
                comp.clear();
                let mut stack = vec![start];
                seen[start] = true;
                while let Some(i) = stack.pop() {
                    comp.push(i);
                    let (x, y) = (i % w, i / w);
                    let visit = |ni: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                        if !mask[ni] && !outside[ni] && !seen[ni] {
                            seen[ni] = true;
                            stack.push(ni);
                        }
                    };
                    if x > 0 {
                        visit(i - 1, &mut seen, &mut stack);
                    }
                    if x + 1 < w {
                        visit(i + 1, &mut seen, &mut stack);
                    }
                    if y > 0 {
                        visit(i - w, &mut seen, &mut stack);
                    }
                    if y + 1 < h {
                        visit(i + w, &mut seen, &mut stack);
                    }
                }
                if comp.len() <= cap {
                    for &i in &comp {
                        mask[i] = true;
                    }
                }
            }
        }
    }
}

pub(crate) fn dilate(mask: &mut [bool], w: usize, h: usize, r: usize) {
    box_pass(mask, w, h, r, true);
}

pub(crate) fn erode(mask: &mut [bool], w: usize, h: usize, r: usize) {
    box_pass(mask, w, h, r, false);
}

/// Separable box morphology: horizontal then vertical sweep.
fn box_pass(mask: &mut [bool], w: usize, h: usize, r: usize, grow: bool) {
    let src = mask.to_vec();
    let hit = |v: bool| if grow { v } else { !v };
    let mut tmp = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            let any = (lo..=hi).any(|xx| hit(src[y * w + xx]));
            tmp[y * w + x] = if grow { any } else { !any };
        }
    }
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            let any = (lo..=hi).any(|yy| hit(tmp[yy * w + x]));
            mask[y * w + x] = if grow { any } else { !any };
        }
    }
}

/// 3×3 box soften of a binary matte → 0..1 coverage (1-px anti-aliased edge).
pub(crate) fn soften(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(1);
            let hi = (x + 1).min(w - 1);
            let mut sum = 0f32;
            for xx in lo..=hi {
                sum += mask[y * w + xx] as u8 as f32;
            }
            tmp[y * w + x] = sum / (hi - lo + 1) as f32;
        }
    }
    let mut out = vec![0f32; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(1);
            let hi = (y + 1).min(h - 1);
            let mut sum = 0f32;
            for yy in lo..=hi {
                sum += tmp[yy * w + x];
            }
            out[y * w + x] = sum / (hi - lo + 1) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn exclusive_cut_stack_reassembles_source() {
        // Opaque gradient source; two overlapping masks + full coverage.
        let (w, h) = (64u32, 48u32);
        let source = RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 5) as u8, 128, 255])
        });
        let masks = vec![
            ("back".to_string(), rect_mask(w, h, Rect { x: 0, y: 0, w, h })),
            ("front".to_string(), rect_mask(w, h, Rect { x: 20, y: 10, w: 24, h: 20 })),
        ];
        let pieces = exclusive_cut(
            &source,
            &masks,
            &CutOptions { seam_fringe: 3, rescue_unclaimed: false },
        )
        .unwrap();
        assert_eq!(pieces.len(), 2);

        // Recompose back→front; must match the source within feather tolerance.
        let mut canvas = RgbaImage::new(w, h);
        for piece in &pieces {
            for (x, y, p) in piece.image.enumerate_pixels() {
                if p.0[3] == 0 {
                    continue;
                }
                let (gx, gy) = ((piece.bbox.x as u32) + x, (piece.bbox.y as u32) + y);
                let under = *canvas.get_pixel(gx, gy);
                let ta = p.0[3] as f32 / 255.0;
                let blend = |t: u8, u: u8| {
                    (t as f32 * ta + u as f32 * (1.0 - ta)).round() as u8
                };
                canvas.put_pixel(
                    gx,
                    gy,
                    image::Rgba([
                        blend(p.0[0], under.0[0]),
                        blend(p.0[1], under.0[1]),
                        blend(p.0[2], under.0[2]),
                        255,
                    ]),
                );
            }
        }
        for (x, y, p) in source.enumerate_pixels() {
            let q = canvas.get_pixel(x, y);
            for c in 0..3 {
                assert!(
                    (p.0[c] as i32 - q.0[c] as i32).abs() <= 3,
                    "pixel ({x},{y}) channel {c}: {} vs {}",
                    p.0[c],
                    q.0[c]
                );
            }
        }
    }

    #[test]
    fn fully_occluded_mask_yields_no_piece() {
        let (w, h) = (32u32, 32u32);
        let source = RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let masks = vec![
            ("hidden".to_string(), rect_mask(w, h, Rect { x: 4, y: 4, w: 8, h: 8 })),
            ("cover".to_string(), rect_mask(w, h, Rect { x: 0, y: 0, w, h })),
        ];
        let pieces = exclusive_cut(
            &source,
            &masks,
            &CutOptions { seam_fringe: 0, rescue_unclaimed: false },
        )
        .unwrap();
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].id, "cover");
    }

    #[test]
    fn guided_filter_sharpens_soft_boundary_onto_color_edge() {
        // Guide: hard vertical color edge at x=40. Soft mask: a blurry 16px ramp centred
        // on the edge (what a bilinear-upscaled SAM boundary looks like at ~2× scale).
        let (w, h) = (80u32, 40u32);
        let guide = image::RgbImage::from_fn(w, h, |x, _| {
            if x < 40 { image::Rgb([30, 60, 200]) } else { image::Rgb([220, 180, 40]) }
        });
        let soft: Vec<f32> = (0..w * h)
            .map(|i| {
                let x = (i % w) as f32;
                (1.0 - (x - 32.0) / 16.0).clamp(0.0, 1.0) // ramp 32..48, crosses 0.5 at 40
            })
            .collect();
        let q = guided_filter_rgb(&soft, &guide, 8, 1e-4);
        let row = 20 * w as usize;
        let width_of = |m: &[f32]| {
            let a = (0..w as usize).find(|&x| m[row + x] < 0.75).unwrap_or(0);
            let b = (0..w as usize).find(|&x| m[row + x] < 0.25).unwrap_or(w as usize);
            b - a
        };
        // The transition collapses onto the color edge: much narrower than the input
        // ramp, still crossing 0.5 at the edge.
        assert!(
            width_of(&q) <= width_of(&soft) / 2,
            "transition should sharpen: {} vs {}",
            width_of(&q),
            width_of(&soft)
        );
        let cross = (0..w as usize).find(|&x| q[row + x] < 0.5).unwrap_or(w as usize);
        assert!((cross as i32 - 40).abs() <= 2, "0.5 crossing stays on the edge, got {cross}");
        // Far from the boundary the mask stays saturated.
        assert!(q[row + 5] > 0.9);
        assert!(q[row + 70] < 0.1);
    }

    #[test]
    fn hole_cap_keeps_large_openings_fills_specks() {
        let (w, h) = (60u32, 60u32);
        // A ring shape: solid block with a big 20×20 window and a tiny 2×2 speck hole.
        let mut raw = rect_mask(w, h, Rect { x: 5, y: 5, w: 50, h: 50 });
        for y in 20..40 {
            for x in 20..40 {
                raw.put_pixel(x, y, image::Luma([0])); // big window (400 px)
            }
        }
        for y in 10..12 {
            for x in 10..12 {
                raw.put_pixel(x, y, image::Luma([0])); // speck (4 px)
            }
        }
        let refined = refine(&raw, None, &[(6, 6)], Some(50));
        // Speck filled…
        assert_eq!(refined.get_pixel(10, 10).0[0], 255, "speck should be filled");
        // …big window kept open (check its centre, clear of the r=2 close).
        assert_eq!(refined.get_pixel(30, 30).0[0], 0, "window should stay open");

        // Uncapped fills both.
        let filled = refine(&raw, None, &[(6, 6)], None);
        assert_eq!(filled.get_pixel(30, 30).0[0], 255);
    }
}
