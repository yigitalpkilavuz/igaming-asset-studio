//! Placement + emboss compositing. The glyph is rasterized onto a base-sized canvas
//! per its transform, then lit as a raised mark: the bevel facing the house light
//! (lower-left) gets a highlight, the far side a soft shadow, and the fill keeps the
//! base's own pixels (slightly toned) so the mark reads as the same material.

use image::{GrayImage, Rgba, RgbaImage};

use super::doc::{EmbossParams, Transform};

/// Rasterize `shape` onto a `bw`×`bh` canvas per the transform (bilinear, rotation
/// about the glyph centre, clockwise degrees).
pub fn place(shape: &GrayImage, bw: u32, bh: u32, t: &Transform) -> GrayImage {
    let (sw, sh) = shape.dimensions();
    let long = sw.max(sh).max(1) as f32;
    let scale = (t.scale.max(0.01) as f32 * bw.min(bh) as f32) / long;
    let (cx, cy) = (t.x as f32 * bw as f32, t.y as f32 * bh as f32);
    let rad = (t.rotation as f32).to_radians();
    let (cos, sin) = (rad.cos(), rad.sin());

    let mut out = GrayImage::new(bw, bh);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - cx;
        let dy = y as f32 + 0.5 - cy;
        // Inverse rotate (clockwise screen rotation → counter-clockwise inverse).
        let rx = dx * cos + dy * sin;
        let ry = -dx * sin + dy * cos;
        let sx = rx / scale + sw as f32 / 2.0 - 0.5;
        let sy = ry / scale + sh as f32 / 2.0 - 0.5;
        p.0[0] = bilinear(shape, sx, sy);
    }
    out
}

fn bilinear(img: &GrayImage, x: f32, y: f32) -> u8 {
    let (w, h) = img.dimensions();
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return 0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (fx, fy) = (x - x0, y - y0);
    let sample = |ix: i64, iy: i64| -> f32 {
        if ix < 0 || iy < 0 || ix >= w as i64 || iy >= h as i64 {
            0.0
        } else {
            img.get_pixel(ix as u32, iy as u32).0[0] as f32
        }
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let v = sample(x0, y0) * (1.0 - fx) * (1.0 - fy)
        + sample(x0 + 1, y0) * fx * (1.0 - fy)
        + sample(x0, y0 + 1) * (1.0 - fx) * fy
        + sample(x0 + 1, y0 + 1) * fx * fy;
    v.round().clamp(0.0, 255.0) as u8
}

/// Compose a placed glyph onto the base. Returns the composed image and the glyph's
/// own alpha mask (clipped to the base's opaque area) on the same canvas — the second
/// file the game engine uses for win-glow effects.
pub fn compose(base: &RgbaImage, placed: &GrayImage, p: &EmbossParams) -> (RgbaImage, GrayImage) {
    let (w, h) = base.dimensions();
    assert_eq!(placed.dimensions(), (w, h), "glyph must be placed on a base-sized canvas");
    let n = (w * h) as usize;

    // Glyph coverage clipped to the plate (a mark never overhangs the base silhouette).
    let mut a = vec![0f32; n];
    for (i, (g, b)) in placed.pixels().zip(base.pixels()).enumerate() {
        a[i] = (g.0[0] as f32 / 255.0) * (b.0[3] as f32 / 255.0);
    }

    // Height field: the coverage blurred to bevel width. Its gradient gives the bevel
    // direction; light from the lower-left means rim = gx − gy (see the direction test).
    let depth = p.depth.clamp(1.0, 64.0) as f32;
    let mut height = a.clone();
    let r = (depth * 0.5).round().max(1.0) as usize;
    box_blur(&mut height, w as usize, h as usize, r);
    box_blur(&mut height, w as usize, h as usize, r);

    let mut out = RgbaImage::new(w, h);
    let mut mask = GrayImage::new(w, h);
    let gain = depth * 0.7071;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let bp = base.get_pixel(x, y).0;
            mask.put_pixel(x, y, image::Luma([(a[i] * 255.0).round() as u8]));
            if bp[3] == 0 {
                out.put_pixel(x, y, Rgba(bp));
                continue;
            }
            let gx = (at(&height, w, h, x as i64 + 1, y as i64) - at(&height, w, h, x as i64 - 1, y as i64)) * 0.5;
            let gy = (at(&height, w, h, x as i64, y as i64 + 1) - at(&height, w, h, x as i64, y as i64 - 1)) * 0.5;
            let rim = (gx - gy) * gain;
            let hi = rim.clamp(0.0, 1.0) * p.highlight.clamp(0.0, 2.0) as f32 * 0.6;
            let sh = (-rim).clamp(0.0, 1.0) * p.shadow.clamp(0.0, 2.0) as f32 * 0.5;
            let tone = 1.0 + (p.fill_tone.clamp(0.25, 1.75) as f32 - 1.0) * a[i];
            let mut px = [0u8; 4];
            for c in 0..3 {
                let mut v = bp[c] as f32 * tone;
                v += hi * (255.0 - v); // push toward white on the lit bevel
                v *= 1.0 - sh; // pull toward black on the far bevel
                px[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            px[3] = bp[3];
            out.put_pixel(x, y, Rgba(px));
        }
    }
    (out, mask)
}

fn at(buf: &[f32], w: u32, h: u32, x: i64, y: i64) -> f32 {
    let x = x.clamp(0, w as i64 - 1) as usize;
    let y = y.clamp(0, h as i64 - 1) as usize;
    buf[y * w as usize + x]
}

/// One separable box-blur pass of radius `r` (running-sum, O(N)).
fn box_blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; buf.len()];
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        let row = &buf[y * w..(y + 1) * w];
        let mut sum: f32 = (0..=r.min(w - 1)).map(|x| row[x]).sum::<f32>() + r as f32 * row[0];
        for x in 0..w {
            tmp[y * w + x] = sum * norm;
            let add = row[(x + r + 1).min(w - 1)];
            let sub = row[x.saturating_sub(r)];
            sum += add - sub;
        }
    }
    for x in 0..w {
        let col = |y: usize| tmp[y * w + x];
        let mut sum: f32 = (0..=r.min(h - 1)).map(col).sum::<f32>() + r as f32 * col(0);
        for y in 0..h {
            buf[y * w + x] = sum * norm;
            let add = col((y + r + 1).min(h - 1));
            let sub = col(y.saturating_sub(r));
            sum += add - sub;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_shape(size: u32) -> GrayImage {
        GrayImage::from_pixel(size, size, image::Luma([255]))
    }

    #[test]
    fn place_centres_and_scales() {
        let placed = place(&square_shape(10), 200, 100, &Transform::default());
        // scale 0.42 of min-dim 100 → 42px square centred at (100, 50).
        assert_eq!(placed.get_pixel(100, 50).0[0], 255, "solid at the centre");
        assert_eq!(placed.get_pixel(100, 50 + 25).0[0], 0, "empty outside the square");
        assert_eq!(placed.get_pixel(100 + 18, 50).0[0], 255, "inside the half-width");
        assert_eq!(placed.get_pixel(10, 10).0[0], 0, "far corner empty");
    }

    #[test]
    fn place_respects_offset() {
        let t = Transform { x: 0.25, y: 0.5, scale: 0.2, rotation: 0.0 };
        let placed = place(&square_shape(10), 200, 100, &t);
        assert_eq!(placed.get_pixel(50, 50).0[0], 255);
        assert_eq!(placed.get_pixel(150, 50).0[0], 0);
    }

    #[test]
    fn emboss_lights_the_lower_left_rim() {
        // Mid-gray opaque base, centred disc glyph.
        let base = RgbaImage::from_pixel(128, 128, Rgba([128, 128, 128, 255]));
        let mut placed = GrayImage::new(128, 128);
        for (x, y, p) in placed.enumerate_pixels_mut() {
            let d = ((x as f32 - 64.0).powi(2) + (y as f32 - 64.0).powi(2)).sqrt();
            p.0[0] = if d < 30.0 { 255 } else { 0 };
        }
        let (out, mask) = compose(&base, &placed, &EmbossParams::default());

        // Sample just at the rim, along the light axis: lower-left of the disc edge
        // must be brighter than the base; upper-right darker.
        let ll = out.get_pixel(64 - 21, 64 + 21).0; // on the bevel toward the light
        let ur = out.get_pixel(64 + 21, 64 - 21).0;
        assert!(ll[0] > 132, "lower-left rim highlighted, got {}", ll[0]);
        assert!(ur[0] < 124, "upper-right rim shadowed, got {}", ur[0]);

        // Mask is the glyph alone, on the same canvas.
        assert_eq!(mask.dimensions(), (128, 128));
        assert_eq!(mask.get_pixel(64, 64).0[0], 255);
        assert_eq!(mask.get_pixel(4, 4).0[0], 0);
    }

    #[test]
    fn emboss_leaves_transparent_base_pixels_alone() {
        // Base with a transparent right half; glyph covering everything.
        let mut base = RgbaImage::from_pixel(64, 64, Rgba([100, 100, 100, 255]));
        for y in 0..64 {
            for x in 32..64 {
                base.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
        let placed = GrayImage::from_pixel(64, 64, image::Luma([255]));
        let (out, mask) = compose(&base, &placed, &EmbossParams::default());
        assert_eq!(out.get_pixel(48, 32).0[3], 0, "transparent stays transparent");
        assert_eq!(mask.get_pixel(48, 32).0[0], 0, "mask clipped to the plate");
        assert!(mask.get_pixel(16, 32).0[0] > 200, "mask present on the plate");
    }
}
