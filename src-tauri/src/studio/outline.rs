//! Mask → editable outline: trace a binary mask's boundaries into closed polygon rings
//! (outer boundaries AND holes), then simplify with Douglas–Peucker so the user gets a
//! manageable set of draggable vertices instead of a pixel staircase. The frontend
//! rasterizes the edited rings back into a mask (even-odd fill), so round-tripping
//! through `layers_set_mask` needs no new persistence.

use std::collections::BTreeMap;

use image::GrayImage;

/// Trace all boundary rings of `mask` (pixels > 127) and simplify each with tolerance
/// `tol` (px). Rings are closed implicitly (last point connects to first), in pixel-corner
/// coordinates. Specks and micro-holes below ~16 px² of enclosed area are dropped.
pub fn trace_outlines(mask: &GrayImage, tol: f64) -> Vec<Vec<[f64; 2]>> {
    let (w, h) = (mask.width() as i32, mask.height() as i32);
    let filled = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < w && y < h && mask.get_pixel(x as u32, y as u32).0[0] > 127
    };

    // Directed boundary edges on the pixel-corner grid, walking each filled pixel's
    // exposed sides clockwise (screen coords, y down). Keyed by start corner.
    let vw = (w + 1) as usize;
    let vid = |x: i32, y: i32| -> usize { y as usize * vw + x as usize };
    let mut edges: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let add = |edges: &mut BTreeMap<usize, Vec<usize>>, a: usize, b: usize| {
        edges.entry(a).or_default().push(b);
    };
    for y in 0..h {
        for x in 0..w {
            if !filled(x, y) {
                continue;
            }
            if !filled(x, y - 1) {
                add(&mut edges, vid(x, y), vid(x + 1, y)); // top → right
            }
            if !filled(x + 1, y) {
                add(&mut edges, vid(x + 1, y), vid(x + 1, y + 1)); // right → down
            }
            if !filled(x, y + 1) {
                add(&mut edges, vid(x + 1, y + 1), vid(x, y + 1)); // bottom → left
            }
            if !filled(x - 1, y) {
                add(&mut edges, vid(x, y + 1), vid(x, y)); // left → up
            }
        }
    }

    // Stitch directed edges into closed loops. At degenerate corners (diagonally touching
    // regions) two outgoing edges exist — prefer the sharpest clockwise turn relative to
    // the incoming direction so touching regions stay separate rings.
    let dir_of = |a: usize, b: usize| -> (i32, i32) {
        let (ax, ay) = ((a % vw) as i32, (a / vw) as i32);
        let (bx, by) = ((b % vw) as i32, (b / vw) as i32);
        ((bx - ax).signum(), (by - ay).signum())
    };
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    while let Some((&start, _)) = edges.iter().next() {
        let mut ring_v: Vec<usize> = vec![start];
        let mut cur = start;
        let mut incoming: Option<(i32, i32)> = None;
        loop {
            let Some(outs) = edges.get_mut(&cur) else { break };
            if outs.is_empty() {
                edges.remove(&cur);
                break;
            }
            // Pick the preferred outgoing edge: clockwise turn, then straight, then ccw.
            let pick = if outs.len() == 1 {
                0
            } else if let Some((dx, dy)) = incoming {
                let cw = (-dy, dx);
                let prefs = [cw, (dx, dy), (dy, -dx)];
                let mut best = 0;
                'p: for pref in prefs {
                    for (i, &to) in outs.iter().enumerate() {
                        if dir_of(cur, to) == pref {
                            best = i;
                            break 'p;
                        }
                    }
                }
                best
            } else {
                0
            };
            let next = outs.swap_remove(pick);
            if outs.is_empty() {
                edges.remove(&cur);
            }
            incoming = Some(dir_of(cur, next));
            if next == start {
                break; // closed
            }
            ring_v.push(next);
            cur = next;
        }
        if ring_v.len() < 4 {
            continue;
        }
        // Corner indices → points, collapsing collinear runs (unit steps → long edges).
        let pts: Vec<[f64; 2]> = ring_v
            .iter()
            .map(|&v| [(v % vw) as f64, (v / vw) as f64])
            .collect();
        let mut slim: Vec<[f64; 2]> = Vec::new();
        let n = pts.len();
        for i in 0..n {
            let prev = pts[(i + n - 1) % n];
            let next = pts[(i + 1) % n];
            let p = pts[i];
            let collinear = (p[0] - prev[0]) * (next[1] - p[1]) == (p[1] - prev[1]) * (next[0] - p[0]);
            if !collinear {
                slim.push(p);
            }
        }
        if slim.len() < 3 || ring_area(&slim) < 16.0 {
            continue; // speck / micro-hole
        }
        // Canonical start (smallest y, then x) → deterministic rings and stable
        // simplification across re-traces of the same mask.
        if let Some(min_i) = (0..slim.len()).min_by(|&a, &b| {
            (slim[a][1], slim[a][0]).partial_cmp(&(slim[b][1], slim[b][0])).unwrap()
        }) {
            slim.rotate_left(min_i);
        }
        rings.push(simplify_ring(&slim, tol));
    }
    // Big rings first: outer boundaries before their holes, stable for the UI.
    rings.sort_by(|a, b| ring_area(b).total_cmp(&ring_area(a)));
    rings
}

/// |shoelace| / 2.
fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        s += a[0] * b[1] - b[0] * a[1];
    }
    (s / 2.0).abs()
}

/// Douglas–Peucker on a closed ring: anchor at the two mutually farthest-ish points
/// (index 0 and the point farthest from it), simplify both halves, rejoin.
fn simplify_ring(ring: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    if ring.len() <= 4 || tol <= 0.0 {
        return ring.to_vec();
    }
    let far = ring
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let da = (a[0] - ring[0][0]).powi(2) + (a[1] - ring[0][1]).powi(2);
            let db = (b[0] - ring[0][0]).powi(2) + (b[1] - ring[0][1]).powi(2);
            da.total_cmp(&db)
        })
        .map(|(i, _)| i)
        .unwrap_or(ring.len() / 2)
        .max(1);
    let mut half1 = ring[0..=far].to_vec();
    let mut half2 = ring[far..].to_vec();
    half2.push(ring[0]);
    half1 = douglas_peucker(&half1, tol);
    half2 = douglas_peucker(&half2, tol);
    let mut out = half1;
    out.pop(); // shared vertex at `far`
    out.extend_from_slice(&half2[..half2.len() - 1]); // drop the closing duplicate of p0
    if out.len() < 3 {
        ring.to_vec()
    } else {
        out
    }
}

fn douglas_peucker(pts: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let mut worst = 0.0;
    let mut idx = 0;
    for (i, p) in pts.iter().enumerate().skip(1).take(pts.len() - 2) {
        let d = point_segment_dist(*p, a, b);
        if d > worst {
            worst = d;
            idx = i;
        }
    }
    if worst <= tol {
        return vec![a, b];
    }
    let mut left = douglas_peucker(&pts[0..=idx], tol);
    let right = douglas_peucker(&pts[idx..], tol);
    left.pop();
    left.extend_from_slice(&right);
    left
}

fn point_segment_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = vx * vx + vy * vy;
    if len2 <= 0.0 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a[0] + t * vx, a[1] + t * vy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_mask(w: u32, h: u32, x0: u32, y0: u32, rw: u32, rh: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let inside = x >= x0 && y >= y0 && x < x0 + rw && y < y0 + rh;
            image::Luma([if inside { 255 } else { 0 }])
        })
    }

    #[test]
    fn rectangle_traces_to_four_corners() {
        let mask = rect_mask(60, 40, 10, 8, 30, 20);
        let rings = trace_outlines(&mask, 1.0);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].len(), 4, "rect simplifies to its 4 corners: {:?}", rings[0]);
        assert!((ring_area(&rings[0]) - 600.0).abs() < 1.0);
    }

    #[test]
    fn hole_produces_second_ring_and_specks_drop() {
        let mut mask = rect_mask(60, 60, 5, 5, 50, 50);
        for y in 20..40 {
            for x in 20..40 {
                mask.put_pixel(x, y, image::Luma([0])); // 20×20 hole
            }
        }
        mask.put_pixel(2, 2, image::Luma([255])); // 1px speck
        let rings = trace_outlines(&mask, 1.0);
        assert_eq!(rings.len(), 2, "outer + hole, speck dropped");
        assert!(ring_area(&rings[0]) > ring_area(&rings[1]));
        assert!((ring_area(&rings[1]) - 400.0).abs() < 1.0);
    }

    #[test]
    fn diagonally_touching_regions_stay_separate_rings() {
        let mut mask = GrayImage::new(20, 20);
        for y in 2..8 {
            for x in 2..8 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        for y in 8..14 {
            for x in 8..14 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        let rings = trace_outlines(&mask, 1.0);
        assert_eq!(rings.len(), 2, "diagonal touch must not merge into one ring");
    }

    #[test]
    fn simplify_respects_tolerance() {
        // A rectangle with a 1px nub: tol 0.5 keeps the nub, tol 2 flattens it.
        let mut mask = rect_mask(40, 30, 5, 5, 20, 15);
        mask.put_pixel(25, 10, image::Luma([255]));
        let fine = trace_outlines(&mask, 0.4);
        let coarse = trace_outlines(&mask, 2.0);
        assert!(fine[0].len() > coarse[0].len());
        // The nub may survive as at most one vertex depending on the DP anchor split;
        // the contract is "near-rectangle", not an exact count.
        assert!(coarse[0].len() <= 5, "coarse ring should be near-rectangular: {:?}", coarse[0]);
        // Determinism: re-tracing yields identical rings.
        assert_eq!(trace_outlines(&mask, 2.0)[0], coarse[0]);
    }
}
