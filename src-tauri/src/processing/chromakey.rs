//! Rust-native chroma-key background removal. No external tools — removes a solid key
//! colour (magenta by default) with a soft tolerance band and light edge despill.

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

/// Remove `key` colour from an image → transparent PNG bytes.
/// `tol` is the inner (fully-keyed) colour distance in 0..1; a feather band extends it.
pub fn chroma_key(bytes: &[u8], key: (u8, u8, u8), tol: f32) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode failed: {e}"))?;
    let src = img.to_rgba8();
    let (w, h) = src.dimensions();
    let mut out = RgbaImage::new(w, h);

    let (kr, kg, kb) = (key.0 as f32, key.1 as f32, key.2 as f32);
    let t0 = tol; // fully keyed within this distance
    let t1 = tol + 0.18; // feather out to here
    let norm = (3.0f32).sqrt();

    // Pass 1: alpha from colour distance to the key.
    let mut alphas = vec![0u8; (w * h) as usize];
    for (x, y, px) in src.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        let dr = (r as f32 - kr) / 255.0;
        let dg = (g as f32 - kg) / 255.0;
        let db = (b as f32 - kb) / 255.0;
        let dist = (dr * dr + dg * dg + db * db).sqrt() / norm; // 0..1

        let mut alpha = a as f32 / 255.0;
        if dist <= t0 {
            alpha = 0.0;
        } else if dist < t1 {
            alpha *= (dist - t0) / (t1 - t0);
        }
        alphas[(y * w + x) as usize] = (alpha * 255.0).round() as u8;
    }

    // Pass 2: despill ONLY near keyed edges. Spill is where key light bleeds onto the
    // subject's rim — interior pixels are the artwork's own colour and must never be
    // touched (a global R/B-toward-G pull desaturates every red and purple in the
    // image). A pixel despills only when it sits within DESPILL_R of a (semi)keyed
    // pixel, and only by the magenta EXCESS min(R,B)−G — large for a genuine magenta
    // cast (R and B elevated together), near zero for a pure red.
    const DESPILL_R: i32 = 2;
    for (x, y, px) in src.enumerate_pixels() {
        let [r, g, b, _] = px.0;
        let alpha = alphas[(y * w + x) as usize];
        let (mut rr, mut bb) = (r as f32, b as f32);
        if alpha > 0 {
            let near_edge = (-DESPILL_R..=DESPILL_R).any(|dy| {
                (-DESPILL_R..=DESPILL_R).any(|dx| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && (nx as u32) < w
                        && (ny as u32) < h
                        && alphas[(ny as u32 * w + nx as u32) as usize] < 255
                })
            });
            if near_edge {
                let excess = rr.min(bb) - g as f32;
                if excess > 0.0 {
                    rr -= excess * 0.6;
                    bb -= excess * 0.6;
                }
            }
        }
        out.put_pixel(x, y, Rgba([rr.round() as u8, g, bb.round() as u8, alpha]));
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(out)
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("encode failed: {e}"))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    /// A magenta field with an opaque green square in the middle.
    fn sample() -> Vec<u8> {
        let mut img = RgbaImage::from_pixel(40, 40, Rgba([255, 0, 255, 255]));
        for y in 15..25 {
            for x in 15..25 {
                img.put_pixel(x, y, Rgba([40, 160, 60, 255]));
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn keys_out_magenta_keeps_subject() {
        let out = chroma_key(&sample(), (255, 0, 255), 0.18).unwrap();
        let img = image::load_from_memory(&out).unwrap().to_rgba8();
        // Corner (was magenta) → transparent.
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        // Center (green subject) → opaque.
        assert_eq!(img.get_pixel(20, 20).0[3], 255);
    }

    #[test]
    fn despill_never_touches_interior_reds() {
        // A crimson square (B slightly above G — true of most reds in real art) on
        // magenta. The old global despill desaturated its ENTIRE interior toward grey;
        // only rim pixels adjacent to the keyed background may shift.
        let crimson = Rgba([200, 30, 60, 255]);
        let mut img = RgbaImage::from_pixel(40, 40, Rgba([255, 0, 255, 255]));
        for y in 10..30 {
            for x in 10..30 {
                img.put_pixel(x, y, crimson);
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();

        let out = chroma_key(&buf.into_inner(), (255, 0, 255), 0.18).unwrap();
        let keyed = image::load_from_memory(&out).unwrap().to_rgba8();
        // Interior: byte-identical colour, fully opaque.
        assert_eq!(keyed.get_pixel(20, 20).0, crimson.0, "interior red must not shift");
        assert_eq!(keyed.get_pixel(12, 12).0, crimson.0, "near-rim interior must not shift");
        // Rim pixel may despill, but only by the magenta excess (min(R,B)−G = 30),
        // never the old half-way-to-grey pull (which sent R 200 → 115).
        let rim = keyed.get_pixel(10, 20).0;
        assert!(rim[0] >= 180, "rim red overcorrected: {rim:?}");
    }

    #[test]
    fn despill_still_kills_magenta_fringe_at_edges() {
        // A magenta-cast rim pixel (R and B elevated together) next to the keyed
        // background must lose most of its cast.
        let mut img = RgbaImage::from_pixel(20, 20, Rgba([255, 0, 255, 255]));
        for y in 5..15 {
            for x in 5..15 {
                img.put_pixel(x, y, Rgba([40, 160, 60, 255]));
            }
        }
        img.put_pixel(5, 10, Rgba([230, 90, 220, 255])); // spill-tinted rim pixel
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();

        let out = chroma_key(&buf.into_inner(), (255, 0, 255), 0.18).unwrap();
        let keyed = image::load_from_memory(&out).unwrap().to_rgba8();
        let px = keyed.get_pixel(5, 10).0;
        // excess = min(230,220) − 90 = 130; both channels drop by 0.6·excess = 78.
        assert!(px[0] <= 160 && px[2] <= 150, "fringe kept its magenta cast: {px:?}");
    }
}
