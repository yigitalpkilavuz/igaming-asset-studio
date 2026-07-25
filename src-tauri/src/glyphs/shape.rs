//! Turn an AI-generated glyph picture into a clean single-colour shape: white fill,
//! soft anti-aliased alpha, speckles dropped, cropped to the mark with a small margin.
//! Works from either native transparency or a flat-background render (the generation
//! prompt asks for solid black on white, but polarity is detected, not assumed).

use image::{Rgba, RgbaImage};

/// Minimum kept-component area relative to the largest one (drops speckles/noise while
/// keeping genuinely multi-stroke glyphs like a three-loop coil).
const COMPONENT_KEEP_RATIO: usize = 50;

pub fn extract_shape(img: &RgbaImage) -> Result<RgbaImage, String> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return Err("glyph image is too small".into());
    }
    let n = (w * h) as usize;

    // Soft coverage field in 0..1: native alpha when present, otherwise a smooth
    // threshold around Otsu's split of the luminance histogram.
    let has_alpha = img.pixels().filter(|p| p.0[3] < 250).count() > n / 100;
    let mut a = vec![0f32; n];
    if has_alpha {
        for (i, p) in img.pixels().enumerate() {
            a[i] = p.0[3] as f32 / 255.0;
        }
    } else {
        let gray: Vec<u8> = img
            .pixels()
            .map(|p| ((p.0[0] as u32 * 299 + p.0[1] as u32 * 587 + p.0[2] as u32 * 114) / 1000) as u8)
            .collect();
        let t = otsu_midpoint(&gray);
        // The shape is the minority side of the split (icon on a plain field, either polarity).
        let dark = gray.iter().filter(|&&g| (g as f32) < t).count();
        let dark_is_shape = dark * 2 < n;
        const SOFT: f32 = 24.0; // half-width of the anti-alias ramp, in gray levels
        for (i, &g) in gray.iter().enumerate() {
            let d = if dark_is_shape { t - g as f32 } else { g as f32 - t };
            a[i] = (d / SOFT + 0.5).clamp(0.0, 1.0);
        }
    }

    // Drop speckles: label solid components, keep anything within 1/50 of the largest.
    let solid: Vec<bool> = a.iter().map(|&v| v >= 0.5).collect();
    let labels = label_components(&solid, w as usize, h as usize);
    let mut areas = std::collections::HashMap::new();
    for &l in &labels {
        if l > 0 {
            *areas.entry(l).or_insert(0usize) += 1;
        }
    }
    let largest = areas.values().copied().max().unwrap_or(0);
    if largest < 32 {
        return Err("no usable shape found in the glyph image".into());
    }
    if largest > n * 9 / 10 {
        return Err("the glyph fills the whole image — could not separate it from the background".into());
    }
    let min_keep = (largest / COMPONENT_KEEP_RATIO).max(24);
    let keep: std::collections::HashSet<u32> =
        areas.into_iter().filter(|&(_, area)| area >= min_keep).map(|(l, _)| l).collect();

    // Kill alpha outside kept components (grown 2px so the anti-alias fringe survives).
    let mut keep_mask: Vec<bool> = labels.iter().map(|l| keep.contains(l)).collect();
    grow(&mut keep_mask, w as usize, h as usize, 2);
    for (v, k) in a.iter_mut().zip(&keep_mask) {
        if !k {
            *v = 0.0;
        }
    }

    // Crop to the mark with a small margin.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if a[(y * w + x) as usize] >= 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 {
        return Err("no usable shape found in the glyph image".into());
    }
    let margin = ((x1 - x0).max(y1 - y0) / 25).max(3);
    let x0 = x0.saturating_sub(margin);
    let y0 = y0.saturating_sub(margin);
    let x1 = (x1 + margin).min(w - 1);
    let y1 = (y1 + margin).min(h - 1);

    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = RgbaImage::new(cw, ch);
    for y in 0..ch {
        for x in 0..cw {
            let v = a[((y + y0) * w + (x + x0)) as usize];
            out.put_pixel(x, y, Rgba([255, 255, 255, (v * 255.0).round() as u8]));
        }
    }
    Ok(out)
}

/// Otsu's split of an 8-bit histogram, returned as the midpoint between the two class
/// means (the raw Otsu index sits ON the shape's own spike for hard bi-level images,
/// which would leave shape pixels at half coverage).
fn otsu_midpoint(gray: &[u8]) -> f32 {
    let mut hist = [0u64; 256];
    for &g in gray {
        hist[g as usize] += 1;
    }
    let total = gray.len() as u64;
    let sum_all: u64 = hist.iter().enumerate().map(|(v, &c)| v as u64 * c).sum();
    let (mut sum_b, mut w_b) = (0u64, 0u64);
    let (mut best_mid, mut best_var) = (127.5f32, 0f64);
    for t in 0..256 {
        w_b += hist[t];
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as u64 * hist[t];
        let m_b = sum_b as f64 / w_b as f64;
        let m_f = (sum_all - sum_b) as f64 / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f) * (m_b - m_f);
        if var > best_var {
            best_var = var;
            best_mid = ((m_b + m_f) / 2.0) as f32;
        }
    }
    best_mid
}

/// 4-connected component labelling (0 = background).
fn label_components(mask: &[bool], w: usize, h: usize) -> Vec<u32> {
    let mut labels = vec![0u32; mask.len()];
    let mut next = 0u32;
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        next += 1;
        labels[start] = next;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if mask[j] && labels[j] == 0 {
                    labels[j] = next;
                    stack.push(j);
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
    labels
}

/// Grow a boolean mask by `r` pixels (chebyshev), in place.
fn grow(mask: &mut [bool], w: usize, h: usize, r: usize) {
    for _ in 0..r {
        let prev = mask.to_vec();
        for y in 0..h {
            for x in 0..w {
                if prev[y * w + x] {
                    continue;
                }
                let hit = (x > 0 && prev[y * w + x - 1])
                    || (x + 1 < w && prev[y * w + x + 1])
                    || (y > 0 && prev[(y - 1) * w + x])
                    || (y + 1 < h && prev[(y + 1) * w + x]);
                if hit {
                    mask[y * w + x] = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc_image(w: u32, h: u32, fg: [u8; 3], bg: [u8; 3]) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w as f32 / 5.0);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let c = if d < r { fg } else { bg };
            *p = Rgba([c[0], c[1], c[2], 255]);
        }
        img
    }

    #[test]
    fn extracts_dark_on_light_and_inverted() {
        for (fg, bg) in [([10, 10, 10], [245, 245, 245]), ([245, 245, 245], [10, 10, 10])] {
            let shape = extract_shape(&disc_image(100, 100, fg, bg)).unwrap();
            let (w, h) = shape.dimensions();
            // Cropped near the disc (diameter 40) plus margin — far smaller than the input.
            assert!(w < 60 && h < 60, "cropped to the mark, got {w}×{h}");
            let centre = shape.get_pixel(w / 2, h / 2).0;
            assert_eq!(centre[3], 255, "solid inside the mark");
            assert_eq!(&centre[..3], &[255, 255, 255], "white fill");
            assert_eq!(shape.get_pixel(0, 0).0[3], 0, "transparent margin");
        }
    }

    #[test]
    fn drops_speckles_keeps_multi_stroke() {
        let mut img = disc_image(120, 120, [0, 0, 0], [255, 255, 255]);
        // A second real stroke…
        for y in 20..30 {
            for x in 20..60 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        // …and a 2×2 speck of noise far away.
        for y in 110..112 {
            for x in 110..112 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let shape = extract_shape(&img).unwrap();
        let (w, h) = shape.dimensions();
        // The bbox spans disc + stroke but the speck was dropped, so it must not
        // reach the far corner region.
        assert!(w < 90 && h < 90, "speck excluded from crop, got {w}×{h}");
        assert!(shape.pixels().filter(|p| p.0[3] > 200).count() > 1500, "both strokes kept");
    }
}
