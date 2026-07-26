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

/// Target region count for the partition the labeler reads — far fewer than the old
/// overlapping pile; the labeler then groups these into 3–8 parts.
const MAX_REGIONS: usize = 20;

/// A raw SAM candidate at RESIZED dims, kept with its ranking score for winner-take-all.
struct Candidate {
    /// Set-pixel indices (row-major, resized dims).
    idxs: Vec<u32>,
    area: u64,
    /// Predicted IoU × stability — higher wins overlaps.
    score: f32,
}

/// Sweep a `grid`×`grid` point lattice over the opaque pixels, decode a SAM mask per point
/// (encoder is computed once and cached), then RESOLVE the overlapping candidates into a
/// non-overlapping partition of the silhouette (each opaque pixel in exactly one region).
/// Returns the regions largest-first. Never fails on region count — a source with no
/// confident structure comes back as one whole-silhouette region the user can split.
/// Runs blocking — call inside `spawn_blocking`.
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

    // Content-aware seeds: a light grid for coverage PLUS distance-transform lobe maxima
    // (limbs/props a uniform grid steps over) and per-colour-cluster interior points (a
    // differently-hued gem/eye that geometry alone misses). One SAM decode per seed under a
    // single engine lock (encoder embedding cached). Thresholds are relaxed vs a clean-
    // object sweep so thin/small ornament masks survive — the winner-take-all resolver
    // cleans up the extra noise, and content-seeded regions are protected from the merge.
    let seeds = seed_points(source, grid);
    const MIN_IOU: f32 = 0.72;
    const MIN_STABILITY: f32 = 0.80;
    let raw: Vec<(GrayImage, f32)> = sam::with_engine(weights, model, |engine| {
        let mut out: Vec<(GrayImage, f32)> = Vec::new();
        for s in &seeds {
            let px = ((s.nx * (w - 1) as f64).round() as u32).min(w - 1);
            let py = ((s.ny * (h - 1) as f64).round() as u32).min(h - 1);
            if alpha.get_pixel(px, py).0[0] <= 8 {
                continue; // transparent — nothing to segment here
            }
            let cands =
                engine.segment_candidates(source_sha, &rgb, &[(s.nx, s.ny, true)], Some(sam_dir))?;
            for (mask, iou, stability) in cands {
                if iou < MIN_IOU || stability < MIN_STABILITY {
                    continue;
                }
                out.push((mask, iou * stability));
            }
        }
        Ok(out)
    })?;
    // No confident structure → hand back the whole silhouette as one splittable region
    // instead of failing.
    if raw.is_empty() {
        return Ok(silhouette_region(&alpha));
    }

    // Scale-independent area window against the resized frame. The candidate floor is low
    // (keep small ornament masks); the MERGE floor below is what actually removes slivers,
    // and content-seeded regions are protected from it so genuine small parts survive.
    let (rw, rh) = raw[0].0.dimensions();
    let (rw, rh) = (rw as usize, rh as usize);
    let resized_total = (rw * rh) as u64;
    let min_cand = (resized_total / 2000).max(48) as usize; // keep small candidates
    let merge_floor = (resized_total / 500).max(64) as usize; // unprotected sliver floor
    let max_area = (resized_total * 92 / 100) as usize;

    let cands: Vec<Candidate> = raw
        .iter()
        .filter_map(|(mask, score)| {
            let idxs: Vec<u32> = mask
                .pixels()
                .enumerate()
                .filter(|(_, p)| p.0[0] > 127)
                .map(|(i, _)| i as u32)
                .collect();
            let area = idxs.len();
            (area >= min_cand && area <= max_area)
                .then_some(Candidate { idxs, area: area as u64, score: *score })
        })
        .collect();
    if cands.is_empty() {
        return Ok(silhouette_region(&alpha));
    }

    // Content seeds in resized-dim pixels — the regions they land in are protected from the
    // sliver merge, so a small gem/eye SAM found isn't folded back into its neighbour.
    let content_seeds: Vec<(usize, usize)> = seeds
        .iter()
        .filter(|s| s.content)
        .map(|s| {
            (
                ((s.nx * (rw - 1) as f64).round() as usize).min(rw - 1),
                ((s.ny * (rh - 1) as f64).round() as usize).min(rh - 1),
            )
        })
        .collect();

    // Resized-dims opaque domain for winner-take-all + gap fill.
    let opaque_resized: Vec<bool> = image::imageops::resize(
        &alpha,
        rw as u32,
        rh as u32,
        image::imageops::FilterType::Nearest,
    )
    .pixels()
    .map(|p| p.0[0] > 8)
    .collect();
    let (labels, count) = resolve_partition(
        &cands,
        &opaque_resized,
        rw,
        rh,
        merge_floor,
        MAX_REGIONS,
        &content_seeds,
    );
    if count == 0 {
        return Ok(silhouette_region(&alpha));
    }

    // Nearest-upscale the label map to source dims (never blend ids), then extract one
    // Region per label: clip to source alpha, close speck holes, tight bbox.
    let (wu, hu) = (w as usize, h as usize);
    let mut regions = Vec::new();
    for label in 1..=count {
        let mut mb = vec![false; wu * hu];
        for y in 0..hu {
            let ry = y * rh / hu;
            for x in 0..wu {
                let rx = x * rw / wu;
                if labels[ry * rw + rx] == label {
                    mb[y * wu + x] = true;
                }
            }
        }
        for (m, a) in mb.iter_mut().zip(alpha.pixels()) {
            *m = *m && a.0[0] > 8;
        }
        matte::fill_holes(&mut mb, wu, hu, Some(((w * h) / 2000).max(64) as usize));
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
    if regions.is_empty() {
        return Ok(silhouette_region(&alpha));
    }
    regions.sort_by(|a, b| b.area.cmp(&a.area));
    for (i, r) in regions.iter_mut().enumerate() {
        r.id = (i + 1) as u32;
    }
    Ok(regions)
}

/// The whole opaque silhouette as a single region — the graceful fallback when the sweep
/// finds no confident structure (returned instead of erroring; the user can split it).
fn silhouette_region(alpha: &GrayImage) -> Vec<Region> {
    let (w, h) = alpha.dimensions();
    let mut mask = GrayImage::new(w, h);
    let mut area = 0u32;
    for (px, a) in mask.pixels_mut().zip(alpha.pixels()) {
        if a.0[0] > 8 {
            px.0[0] = 255;
            area += 1;
        }
    }
    match matte::mask_bbox(&mask) {
        Some(bbox) => vec![Region { id: 1, mask, area, bbox }],
        None => Vec::new(),
    }
}

/// Resolve overlapping candidates into ONE label per opaque pixel. The MOST-SPECIFIC
/// (smallest-area) candidate claims each pixel, score breaking ties — this errs toward more
/// parts, which matters because over-segmentation is recoverable (the labeler merges) but
/// under-segmentation is not (a same-colour arm fused to the torso can never be split
/// later). It also subsumes the old IoU dedupe. Uncovered opaque pixels are BFS-filled from
/// the nearest label, so the
/// partition covers the whole silhouette (no silent gaps left for cut-time rescue). Small
/// regions merge into their longest-shared-border neighbour and the count is squeezed to
/// `max_regions` — except regions containing a `content_seed`, which are protected from the
/// sliver floor so genuine small parts survive. Returns (labels 1..=count, 0 = bg; count).
fn resolve_partition(
    cands: &[Candidate],
    opaque: &[bool],
    w: usize,
    h: usize,
    min_area: usize,
    max_regions: usize,
    content_seeds: &[(usize, usize)],
) -> (Vec<u32>, u32) {
    let n = w * h;
    // 1. Claim, most-specific (smallest) candidate first; score breaks ties.
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| {
        cands[a].area.cmp(&cands[b].area).then(
            cands[b]
                .score
                .partial_cmp(&cands[a].score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    let mut owner = vec![u32::MAX; n];
    for &ci in &order {
        for &i in &cands[ci].idxs {
            let i = i as usize;
            if opaque[i] && owner[i] == u32::MAX {
                owner[i] = ci as u32;
            }
        }
    }
    // 2. Fill uncovered opaque pixels from the nearest owned label.
    nearest_label_fill(&mut owner, opaque, w, h);

    // 3. Compact owner (candidate indices) → dense 1-based labels.
    let mut remap = vec![0u32; cands.len() + 1];
    let mut labels = vec![0u32; n];
    let mut count = 0u32;
    for i in 0..n {
        let o = owner[i];
        if o == u32::MAX {
            continue;
        }
        let idx = o as usize;
        if remap[idx] == 0 {
            count += 1;
            remap[idx] = count;
        }
        labels[i] = remap[idx];
    }

    // 4. Protect regions a content seed landed in, then merge slivers + squeeze to budget.
    let mut protected = vec![false; count as usize + 1];
    for &(sx, sy) in content_seeds {
        if sx < w && sy < h {
            let l = labels[sy * w + sx] as usize;
            if l != 0 {
                protected[l] = true;
            }
        }
    }
    merge_small_regions(&mut labels, &mut count, w, h, min_area, max_regions, &protected);
    (labels, count)
}

/// Multi-source BFS: every opaque pixel with no owner adopts the nearest owned label.
/// (Same expansion as matte::exclusive_cut's unclaimed-pixel rescue.)
fn nearest_label_fill(owner: &mut [u32], opaque: &[bool], w: usize, h: usize) {
    let mut queue: std::collections::VecDeque<usize> = owner
        .iter()
        .enumerate()
        .filter(|(_, c)| **c != u32::MAX)
        .map(|(i, _)| i)
        .collect();
    while let Some(i) = queue.pop_front() {
        let label = owner[i];
        let (x, y) = (i % w, i / w);
        let mut visit = |ni: usize| {
            if opaque[ni] && owner[ni] == u32::MAX {
                owner[ni] = label;
                queue.push_back(ni);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
}

/// Fold regions below `min_area` (and, once none remain, the smallest regions until the
/// count fits `max_regions`) into the neighbour they share the longest border with. Stray
/// slivers left by winner-take-all rejoin the part around them, so the labeler sees a
/// small, clean set. `protected` regions (a content seed landed in them) are exempt from
/// the floor — only merged as a last resort when still over budget and nothing else is
/// left. Operates in place on a 1-based label map; `count` is updated.
fn merge_small_regions(
    labels: &mut [u32],
    count: &mut u32,
    w: usize,
    h: usize,
    min_area: usize,
    max_regions: usize,
    protected: &[bool],
) {
    let orig = *count as usize;
    let prot = |l: usize| protected.get(l).copied().unwrap_or(false);
    let mut live = orig;
    loop {
        if live <= 1 {
            break;
        }
        let mut area = vec![0usize; orig + 1];
        for &l in labels.iter() {
            if l != 0 {
                area[l as usize] += 1;
            }
        }
        let over = live > max_regions;
        // Prefer the smallest UNPROTECTED region below the floor; if none and we're over
        // budget, the smallest unprotected of any size, then (last resort) the smallest.
        let victim = (1..=orig)
            .filter(|&l| area[l] > 0 && !prot(l) && area[l] < min_area)
            .min_by_key(|&l| area[l])
            .or_else(|| {
                over.then(|| {
                    (1..=orig)
                        .filter(|&l| area[l] > 0 && !prot(l))
                        .min_by_key(|&l| area[l])
                        .or_else(|| (1..=orig).filter(|&l| area[l] > 0).min_by_key(|&l| area[l]))
                })
                .flatten()
            });
        let Some(victim) = victim else {
            break; // all remaining regions clear the floor (or are protected) and fit budget
        };
        // Longest-shared-border neighbour (4-connectivity); fall back to the largest other
        // region for a detached island so we always make progress.
        let mut border = vec![0usize; orig + 1];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if labels[i] as usize != victim {
                    continue;
                }
                let tally = |ni: usize, border: &mut [usize]| {
                    let nl = labels[ni] as usize;
                    if nl != 0 && nl != victim {
                        border[nl] += 1;
                    }
                };
                if x > 0 {
                    tally(i - 1, &mut border);
                }
                if x + 1 < w {
                    tally(i + 1, &mut border);
                }
                if y > 0 {
                    tally(i - w, &mut border);
                }
                if y + 1 < h {
                    tally(i + w, &mut border);
                }
            }
        }
        let target = (1..=orig)
            .filter(|&l| l != victim && border[l] > 0)
            .max_by_key(|&l| border[l])
            .or_else(|| {
                (1..=orig)
                    .filter(|&l| l != victim && area[l] > 0)
                    .max_by_key(|&l| area[l])
            });
        let Some(target) = target else { break };
        for l in labels.iter_mut() {
            if *l as usize == victim {
                *l = target as u32;
            }
        }
        live -= 1;
    }
    // Compact to 1..=live contiguous.
    let mut remap = vec![0u32; orig + 1];
    let mut next = 0u32;
    for l in labels.iter_mut() {
        if *l == 0 {
            continue;
        }
        let cur = *l as usize;
        if remap[cur] == 0 {
            next += 1;
            remap[cur] = next;
        }
        *l = remap[cur];
    }
    *count = next;
}

/// Two-pass chamfer distance (ortho 2, diag 3) from each set pixel to the nearest unset
/// pixel — shared by deepest-point label placement and the content-seed generator.
fn chamfer_dt(bits: &[bool], w: usize, h: usize) -> Vec<u32> {
    const INF: u32 = 1 << 20;
    let mut d: Vec<u32> = bits.iter().map(|&b| if b { INF } else { 0 }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if d[i] == 0 {
                continue;
            }
            let mut m = d[i];
            if x > 0 {
                m = m.min(d[i - 1] + 2);
            }
            if y > 0 {
                m = m.min(d[i - w] + 2);
            }
            if x > 0 && y > 0 {
                m = m.min(d[i - w - 1] + 3);
            }
            if x + 1 < w && y > 0 {
                m = m.min(d[i - w + 1] + 3);
            }
            d[i] = m;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if d[i] == 0 {
                continue;
            }
            let mut m = d[i];
            if x + 1 < w {
                m = m.min(d[i + 1] + 2);
            }
            if y + 1 < h {
                m = m.min(d[i + w] + 2);
            }
            if x + 1 < w && y + 1 < h {
                m = m.min(d[i + w + 1] + 3);
            }
            if x > 0 && y + 1 < h {
                m = m.min(d[i + w - 1] + 3);
            }
            d[i] = m;
        }
    }
    d
}

/// Point farthest from the region's boundary (chamfer distance-transform argmax). Always
/// lands ON the region — fixes labels drifting off rings / L-shaped legs at the bbox centre.
fn deepest_point(mask: &GrayImage) -> (i32, i32) {
    let (w, h) = mask.dimensions();
    let (w, h) = (w as usize, h as usize);
    let bits: Vec<bool> = mask.pixels().map(|p| p.0[0] > 127).collect();
    let d = chamfer_dt(&bits, w, h);
    let mut best = 0u32;
    let mut at = (w as i32 / 2, h as i32 / 2);
    for y in 0..h {
        for x in 0..w {
            let v = d[y * w + x];
            if v > best {
                best = v;
                at = (x as i32, y as i32);
            }
        }
    }
    at
}

/// A SAM seed point, normalized 0..1. `content` seeds (distance-transform / colour) are the
/// small parts we went looking for; the regions they land in are protected from the merge.
struct Seed {
    nx: f64,
    ny: f64,
    content: bool,
}

/// Content-aware SAM seeds, computed at a small working resolution: a light grid for
/// coverage, distance-transform lobe maxima (limbs/props a uniform grid steps over), and
/// per-colour-cluster interior points (a flush ornament of a distinct hue). Content seeds
/// come first so the cap keeps them; grid points too close to a content seed are dropped.
fn seed_points(source: &RgbaImage, grid: u32) -> Vec<Seed> {
    let (sw, sh) = source.dimensions();
    let scale = (256.0 / sw.max(sh).max(1) as f64).min(1.0);
    let ww = ((sw as f64 * scale).round() as u32).max(1);
    let wh = ((sh as f64 * scale).round() as u32).max(1);
    let small = image::imageops::resize(source, ww, wh, image::imageops::FilterType::Triangle);
    let (w, h) = (ww as usize, wh as usize);
    let n = w * h;
    let mut opaque = vec![false; n];
    let mut rgb = vec![[0u8; 3]; n];
    for (i, p) in small.pixels().enumerate() {
        opaque[i] = p.0[3] > 24;
        rgb[i] = [p.0[0], p.0[1], p.0[2]];
    }
    let denom = |d: usize| d.saturating_sub(1).max(1) as f64;
    let norm = |x: usize, y: usize| (x as f64 / denom(w), y as f64 / denom(h));

    let mut seeds: Vec<Seed> = Vec::new();
    // Content: distance-transform lobe maxima.
    let dt = chamfer_dt(&opaque, w, h);
    for (x, y) in local_maxima(&dt, w, h) {
        let (nx, ny) = norm(x, y);
        seeds.push(Seed { nx, ny, content: true });
    }
    // Content: colour-cluster interior points.
    for (x, y) in kmeans_seed_points(&rgb, &opaque, w, h) {
        let (nx, ny) = norm(x, y);
        seeds.push(Seed { nx, ny, content: true });
    }
    // Grid coverage: opaque points not already near a content seed.
    let min_sep = (w.max(h) as f64 * 0.02).max(2.0);
    for gy in 0..grid {
        for gx in 0..grid {
            let nx = (gx as f64 + 0.5) / grid as f64;
            let ny = (gy as f64 + 0.5) / grid as f64;
            let px = ((nx * denom(w)).round() as usize).min(w - 1);
            let py = ((ny * denom(h)).round() as usize).min(h - 1);
            if !opaque[py * w + px] {
                continue;
            }
            let close = seeds.iter().any(|s| {
                let dx = (s.nx - nx) * w as f64;
                let dy = (s.ny - ny) * h as f64;
                (dx * dx + dy * dy).sqrt() < min_sep
            });
            if !close {
                seeds.push(Seed { nx, ny, content: false });
            }
        }
    }
    const MAX_SEEDS: usize = 240;
    seeds.truncate(MAX_SEEDS);
    seeds
}

/// Peaks of a distance field, thinned by non-max suppression so each protrusion yields
/// about one seed (suppression radius scales with local thickness).
fn local_maxima(dt: &[u32], w: usize, h: usize) -> Vec<(usize, usize)> {
    let maxd = *dt.iter().max().unwrap_or(&0);
    if maxd == 0 {
        return Vec::new();
    }
    let min_depth = (maxd / 6).max(4); // ignore shallow ripples near the boundary
    let mut peaks: Vec<(usize, usize, u32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = dt[y * w + x];
            if v < min_depth {
                continue;
            }
            let mut is_peak = true;
            'nb: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if dt[ny as usize * w + nx as usize] > v {
                        is_peak = false;
                        break 'nb;
                    }
                }
            }
            if is_peak {
                peaks.push((x, y, v));
            }
        }
    }
    peaks.sort_by(|a, b| b.2.cmp(&a.2));
    let mut kept: Vec<(usize, usize, u32)> = Vec::new();
    for p in peaks {
        let r = (p.2 as f64 / 3.0).max(3.0);
        if kept.iter().all(|k| {
            let dx = k.0 as f64 - p.0 as f64;
            let dy = k.1 as f64 - p.1 as f64;
            (dx * dx + dy * dy).sqrt() > r
        }) {
            kept.push(p);
        }
    }
    kept.into_iter().map(|(x, y, _)| (x, y)).collect()
}

/// k-means the opaque colours, then seed the deepest interior point of each sizeable
/// connected patch of every cluster — catches a differently-hued gem/eye/ornament flush
/// with a larger part (invisible to geometry). Deterministic (no RNG).
fn kmeans_seed_points(rgb: &[[u8; 3]], opaque: &[bool], w: usize, h: usize) -> Vec<(usize, usize)> {
    const K: usize = 8;
    const ITERS: usize = 8;
    let idxs: Vec<usize> = (0..w * h).filter(|&i| opaque[i]).collect();
    if idxs.len() < K * 4 {
        return Vec::new();
    }
    // Deterministic init: evenly spaced samples of opaque pixels sorted by packed RGB.
    let mut sorted = idxs.clone();
    sorted.sort_by_key(|&i| {
        let c = rgb[i];
        ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32
    });
    let mut cent: Vec<[f32; 3]> = (0..K)
        .map(|k| {
            let i = sorted[((k * sorted.len()) / K).min(sorted.len() - 1)];
            [rgb[i][0] as f32, rgb[i][1] as f32, rgb[i][2] as f32]
        })
        .collect();
    let mut assign = vec![0u8; w * h];
    for _ in 0..ITERS {
        for &i in &idxs {
            let c = rgb[i];
            let mut bk = 0usize;
            let mut bd = f32::MAX;
            for (k, ct) in cent.iter().enumerate() {
                let dr = c[0] as f32 - ct[0];
                let dg = c[1] as f32 - ct[1];
                let db = c[2] as f32 - ct[2];
                let d = dr * dr + dg * dg + db * db;
                if d < bd {
                    bd = d;
                    bk = k;
                }
            }
            assign[i] = bk as u8;
        }
        let mut sum = vec![[0f64; 3]; K];
        let mut cnt = vec![0u64; K];
        for &i in &idxs {
            let k = assign[i] as usize;
            let c = rgb[i];
            sum[k][0] += c[0] as f64;
            sum[k][1] += c[1] as f64;
            sum[k][2] += c[2] as f64;
            cnt[k] += 1;
        }
        for k in 0..K {
            if cnt[k] > 0 {
                cent[k] = [
                    (sum[k][0] / cnt[k] as f64) as f32,
                    (sum[k][1] / cnt[k] as f64) as f32,
                    (sum[k][2] / cnt[k] as f64) as f32,
                ];
            }
        }
    }
    let min_cc = ((w * h) / 400).max(12); // ~0.25% of the working frame
    let mut seeds = Vec::new();
    for k in 0..K {
        let bits: Vec<bool> = (0..w * h).map(|i| opaque[i] && assign[i] as usize == k).collect();
        let mut comps: Vec<Vec<usize>> = connected_components(&bits, w, h)
            .into_iter()
            .filter(|c| c.len() >= min_cc)
            .collect();
        comps.sort_by_key(|c| std::cmp::Reverse(c.len()));
        for comp in comps.into_iter().take(3) {
            let mut cb = vec![false; w * h];
            for &i in &comp {
                cb[i] = true;
            }
            let d = chamfer_dt(&cb, w, h);
            let mut best = 0u32;
            let mut at = comp[0];
            for &i in &comp {
                if d[i] > best {
                    best = d[i];
                    at = i;
                }
            }
            seeds.push((at % w, at / w));
        }
    }
    seeds
}

/// 4-connected components of a boolean mask, as lists of pixel indices.
fn connected_components(bits: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; bits.len()];
    let mut comps = Vec::new();
    for start in 0..bits.len() {
        if !bits[start] || seen[start] {
            continue;
        }
        let mut comp = Vec::new();
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % w, i / w);
            let visit = |ni: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                if bits[ni] && !seen[ni] {
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
        comps.push(comp);
    }
    comps
}

/// Greedy graph-colour the regions so no two edge-adjacent regions share a palette index —
/// with a partition, the flat overlay then reads as clean paint-by-numbers. Returns the
/// palette index per region id (indexable by `region.id`).
fn color_regions(labels: &[u32], w: usize, h: usize) -> Vec<usize> {
    let max_id = labels.iter().copied().max().unwrap_or(0) as usize;
    let mut adj: Vec<std::collections::BTreeSet<u32>> = vec![Default::default(); max_id + 1];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let l = labels[i];
            if l == 0 {
                continue;
            }
            if x + 1 < w {
                let r = labels[i + 1];
                if r != 0 && r != l {
                    adj[l as usize].insert(r);
                    adj[r as usize].insert(l);
                }
            }
            if y + 1 < h {
                let dn = labels[i + w];
                if dn != 0 && dn != l {
                    adj[l as usize].insert(dn);
                    adj[dn as usize].insert(l);
                }
            }
        }
    }
    let pal = PALETTE.len();
    let mut color = vec![usize::MAX; max_id + 1];
    let mut ids: Vec<usize> = (1..=max_id).collect();
    ids.sort_by_key(|&l| std::cmp::Reverse(adj[l].len())); // most-constrained first
    for l in ids {
        let mut used = vec![false; pal];
        for &nb in &adj[l] {
            let c = color[nb as usize];
            if c != usize::MAX {
                used[c] = true;
            }
        }
        color[l] = (0..pal).find(|&c| !used[c]).unwrap_or(l % pal);
    }
    for c in color.iter_mut() {
        if *c == usize::MAX {
            *c = 0;
        }
    }
    color
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

/// Render the labeling overlay: dimmed source under a FLAT paint-by-numbers fill — one
/// palette colour per region, adjacent regions guaranteed distinct, each number drawn at
/// the region's deepest interior point. Regions partition the silhouette (no overlap), so
/// the result is unambiguous for the labeler to read. Returned as PNG bytes.
pub fn overlay_png(source: &RgbaImage, regions: &[Region]) -> Result<Vec<u8>, String> {
    let (w, h) = source.dimensions();
    // Label map from the partition, for adjacency-aware colouring.
    let mut labels = vec![0u32; (w * h) as usize];
    for r in regions {
        for (x, y, p) in r.mask.enumerate_pixels() {
            if p.0[0] > 127 {
                labels[(y * w + x) as usize] = r.id;
            }
        }
    }
    let colors = color_regions(&labels, w as usize, h as usize);

    let mut img = RgbaImage::new(w, h);
    // Dimmed source over mid gray so both dark and light art stays readable.
    for (x, y, p) in source.enumerate_pixels() {
        let a = p.0[3] as u32;
        let blend = |c: u8| (((c as u32 * a + 120 * (255 - a)) / 255) / 2 + 40) as u8;
        img.put_pixel(x, y, image::Rgba([blend(p.0[0]), blend(p.0[1]), blend(p.0[2]), 255]));
    }
    // Flat, near-opaque fill (no overlap → no stacking to muddy the colours).
    for r in regions {
        let col = PALETTE[colors[r.id as usize] % PALETTE.len()];
        for (x, y, m) in r.mask.enumerate_pixels() {
            if m.0[0] > 127 {
                let p = img.get_pixel_mut(x, y);
                for c in 0..3 {
                    p.0[c] = ((p.0[c] as u32 * 30 + col[c] as u32 * 70) / 100) as u8;
                }
            }
        }
    }
    for r in regions {
        let (cx, cy) = deepest_point(&r.mask);
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
the ORIGINAL artwork (use it to recognize what each area actually IS — a thumb belongs to \
the hand, an engraving belongs to the plate it is carved on), second the SAME artwork \
divided into numbered, colored regions (they over-segment on purpose). Group EVERY numbered \
region into named parts. The right NUMBER of parts depends on the subject — do not force a \
fixed count, and do not merge to the fewest. One test decides whether something is its own \
part: it can MOVE INDEPENDENTLY of its neighbour in believable animation. Rules:\n\
- SPLIT at every joint or hinge that would bend or swing. A posable character splits at its \
joints — head/neck, torso, and each limb broken into upper arm / forearm / hand and thigh / \
shin / foot (NOT one rigid arm or leg), so elbow, knee and wrist can move. An object or \
emblem splits into its main body PLUS each ornament that dangles, sways, glints or swings on \
its own — a plume, chain, charm, gem or hinged lid.\n\
- MERGE what always moves together: fragments of one surface, decorations painted ON a \
piece, patterns, engravings, texture and tiny slivers all belong to the piece they sit on; \
a face's features are not parts. Two regions that never move relative to each other are one \
part.\n\
- So the count FOLLOWS the subject: a stiff icon or plain emblem is ~3-6 parts, a richly \
ornamented emblem ~6-10, a fully posable character ~8-14. Use as many as the subject has \
independently-moving pieces, and no more — neither lump a jointed limb into one piece nor \
shatter a solid shape into fragments.\n\
- If a PLANNED MOTION is given it OVERRIDES these defaults: split exactly what that motion \
moves on its own (only the joints it bends, only the ornaments it stirs) and merge \
everything the motion never separates.\n\
- Assign every region number to exactly ONE part (overlapping regions: give the number to \
the part it mostly belongs to).\n\
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
        // Cap headroom for a fully-jointed character (head + torso + 2 arms×3 + 2 legs×3 =
        // 15); the subject-adaptive prompt decides the actual count below this.
        if out.len() >= 16 {
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

    fn cand(idxs: Vec<u32>, score: f32) -> Candidate {
        Candidate { area: idxs.len() as u64, idxs, score }
    }

    #[test]
    fn resolve_partition_winner_take_all_and_full_coverage() {
        let (w, h) = (10usize, 4usize);
        let opaque = vec![true; w * h];
        let mk = |pred: &dyn Fn(usize) -> bool| -> Vec<u32> {
            (0..w * h).filter(|&i| pred(i % w)).map(|i| i as u32).collect()
        };
        let a = cand(mk(&|x| x < 6), 0.5); // left, weaker
        let b = cand(mk(&|x| x >= 4), 0.9); // right, stronger; overlap columns 4..6
        let (labels, count) = resolve_partition(&[a, b], &opaque, w, h, 1, 20, &[]);
        assert_eq!(count, 2);
        assert!(labels.iter().all(|&l| l != 0), "every opaque pixel is labeled");
        // Overlap column x=5 goes to the stronger right region; column 0 to the left one.
        assert_eq!(labels[5], labels[9], "overlap joins the stronger region");
        assert_ne!(labels[0], labels[9], "left and right stay distinct");
    }

    #[test]
    fn merge_small_regions_absorbs_sliver() {
        let (w, h) = (10usize, 1usize);
        // region 1 = x0..8, region 2 = a 2px sliver at x8..10; floor 3 folds it into 1.
        let mut labels: Vec<u32> = (0..w).map(|x| if x < 8 { 1 } else { 2 }).collect();
        let mut count = 2;
        merge_small_regions(&mut labels, &mut count, w, h, 3, 20, &[false; 3]);
        assert_eq!(count, 1);
        assert!(labels.iter().all(|&l| l == 1), "sliver merged into its neighbour");
    }

    #[test]
    fn protected_small_region_survives_merge() {
        let (w, h) = (10usize, 1usize);
        // Same sliver as above, but region 2 is protected (a content seed landed in it):
        // it must NOT be folded away despite being under the floor.
        let mut labels: Vec<u32> = (0..w).map(|x| if x < 8 { 1 } else { 2 }).collect();
        let mut count = 2;
        let protected = [false, false, true]; // label 2 protected
        merge_small_regions(&mut labels, &mut count, w, h, 3, 20, &protected);
        assert_eq!(count, 2, "protected sliver kept as its own region");
        assert!(labels[9] != labels[0], "the small part stays distinct");
    }

    #[test]
    fn local_maxima_finds_two_lobes() {
        // Two separated 9×9 blobs → the distance transform peaks once per blob.
        let (w, h) = (40usize, 20usize);
        let mut bits = vec![false; w * h];
        for (cx, cy) in [(8usize, 10usize), (30, 10)] {
            for y in cy - 4..=cy + 4 {
                for x in cx - 4..=cx + 4 {
                    bits[y * w + x] = true;
                }
            }
        }
        let dt = chamfer_dt(&bits, w, h);
        let peaks = local_maxima(&dt, w, h);
        assert_eq!(peaks.len(), 2, "one seed per lobe");
        // Each peak sits inside a blob (x near 8 or near 30).
        assert!(peaks.iter().all(|&(x, _)| (4..=12).contains(&x) || (26..=34).contains(&x)));
    }

    #[test]
    fn kmeans_seeds_find_distinct_patch() {
        // Blue field with a small red square — the seed generator must place a seed inside
        // the red patch (a flush, differently-coloured ornament).
        let (w, h) = (40usize, 40usize);
        let opaque = vec![true; w * h];
        let mut rgb = vec![[20u8, 40, 200]; w * h]; // blue everywhere
        for y in 8..18 {
            for x in 8..18 {
                rgb[y * w + x] = [210, 40, 30]; // red 10×10 patch
            }
        }
        let seeds = kmeans_seed_points(&rgb, &opaque, w, h);
        assert!(
            seeds.iter().any(|&(x, y)| (8..18).contains(&x) && (8..18).contains(&y)),
            "a seed landed inside the red patch: {seeds:?}"
        );
    }

    #[test]
    fn deepest_point_lands_on_ring_band() {
        // 20×20 ring: outer 2..18 with a 4px hole 8..12 — the bbox centre falls in the hole.
        let mask = GrayImage::from_fn(20, 20, |x, y| {
            let outer = x >= 2 && x < 18 && y >= 2 && y < 18;
            let inner = x >= 8 && x < 12 && y >= 8 && y < 12;
            image::Luma([if outer && !inner { 255 } else { 0 }])
        });
        let (px, py) = deepest_point(&mask);
        assert_eq!(mask.get_pixel(px as u32, py as u32).0[0], 255, "label sits on the band");
        assert_eq!(mask.get_pixel(10, 10).0[0], 0, "bbox centre is off the region (in the hole)");
    }

    #[test]
    fn color_regions_differ_for_adjacent() {
        // Three vertical stripes 1|2|3: 1–2 and 2–3 adjacent, 1–3 not.
        let (w, h) = (9usize, 2usize);
        let labels: Vec<u32> = (0..w * h).map(|i| (i % w / 3 + 1) as u32).collect();
        let colors = color_regions(&labels, w, h);
        assert_ne!(colors[1], colors[2]);
        assert_ne!(colors[2], colors[3]);
    }

    #[test]
    fn silhouette_region_from_alpha() {
        let alpha = GrayImage::from_fn(10, 10, |x, y| {
            image::Luma([if x >= 2 && x < 7 && y >= 3 && y < 8 { 255 } else { 0 }])
        });
        let regs = silhouette_region(&alpha);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].bbox, Rect { x: 2, y: 3, w: 5, h: 5 });
        assert_eq!(regs[0].area, 25);
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

    /// Headless dump of the real discovery + overlay on an actual source PNG, for visual
    /// inspection. Env-gated so normal `cargo test` skips it (mirrors the SAM spike). Run:
    ///   WF_SAM_WEIGHTS=/…/sam_vit_b_01ec64.safetensors WF_SRC=/…/source.png WF_OUT=/…/out \
    ///     cargo test --lib regions::tests::autocut_headless_dump -- --nocapture
    #[test]
    fn autocut_headless_dump() {
        let (Some(weights), Some(src), Some(out)) = (
            std::env::var_os("WF_SAM_WEIGHTS"),
            std::env::var_os("WF_SRC"),
            std::env::var_os("WF_OUT"),
        ) else {
            return;
        };
        let weights = std::path::PathBuf::from(weights);
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let model = if weights.to_string_lossy().contains("vit_b") {
            SamModel::Base
        } else {
            SamModel::Tiny
        };
        let source = image::open(&src).unwrap().to_rgba8();
        let sam_dir = out.join("sam");
        let sha = format!("headless-{}x{}", source.width(), source.height());

        // Seeds first — draw them on the source so we can see WHERE Stage-2 looks.
        let seeds = seed_points(&source, 16);
        let (nc, ng) = (
            seeds.iter().filter(|s| s.content).count(),
            seeds.iter().filter(|s| !s.content).count(),
        );
        eprintln!("seeds: {} content + {} grid = {}", nc, ng, seeds.len());
        let mut seed_img = source.clone();
        for s in &seeds {
            let cx = (s.nx * (source.width() - 1) as f64).round() as i32;
            let cy = (s.ny * (source.height() - 1) as f64).round() as i32;
            let col = if s.content { [40, 230, 255, 255] } else { [140, 140, 140, 255] };
            let r = if s.content { 6 } else { 2 };
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    let (x, y) = (cx + dx, cy + dy);
                    if x >= 0 && y >= 0 && x < source.width() as i32 && y < source.height() as i32 {
                        seed_img.put_pixel(x as u32, y as u32, image::Rgba(col));
                    }
                }
            }
        }
        image::DynamicImage::ImageRgba8(seed_img)
            .save(out.join("seeds.png"))
            .unwrap();

        let t = std::time::Instant::now();
        let regions = discover(&weights, model, &source, &sha, &sam_dir, 16).unwrap();
        eprintln!("discover: {} regions in {:?} on {:?}", regions.len(), t.elapsed(), model.tag());
        for r in &regions {
            eprintln!("  region {:>2} area={:>7} bbox={:?}", r.id, r.area, r.bbox);
        }
        let overlay = overlay_png(&source, &regions).unwrap();
        std::fs::write(out.join("overlay.png"), &overlay).unwrap();
        eprintln!("wrote {}/overlay.png and seeds.png", out.display());
    }
}
