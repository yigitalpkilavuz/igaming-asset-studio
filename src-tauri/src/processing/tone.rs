//! Tone normalisation pass — the value budget's enforcement arm.
//!
//! The art is UNLIT: layers separate by tonal value alone, so every asset must land
//! inside its declared band (median HSV value over opaque pixels). Generated assets
//! land wherever the model puts them; this pass solves a gamma from the target —
//! `gamma = ln(target) / ln(current)` — and applies it per channel. Gamma (never add,
//! never multiply) leaves black at black and white at white and moves only the
//! midtones, so the style's flat tonal steps and hard contour lines survive intact.
//!
//! In-band assets pass through untouched (never nudged to the centre); out-of-band
//! assets are corrected to the NEAREST band edge. Corrections beyond the gamma clamp
//! are applied at the clamp and flagged NEEDS_REGEN — that far off is a generation
//! failure, not a colour-grading job.

use crate::model::asset_record::ToneReport;

/// Everything the pass needs, resolved from the blueprint by the caller.
#[derive(Debug, Clone, Copy)]
pub struct ToneSpec {
    /// Target band (median HSV value, 0–1).
    pub min: f64,
    pub max: f64,
    /// Only pixels with alpha ABOVE this are measured (keying fringe must not drag
    /// the median).
    pub alpha_floor: u8,
    /// No output channel may exceed this — headroom reserved for the runtime flash.
    pub ceiling: f64,
    /// Gamma clamp: corrections outside [lo, hi] are clamped + flagged NEEDS_REGEN.
    pub gamma_lo: f64,
    pub gamma_hi: f64,
}

/// Median perceptual luminance (Rec. 709: 0.2126 R + 0.7152 G + 0.0722 B) over pixels with
/// alpha > floor. None when nothing qualifies. Luminance tracks a symbol's *visual weight* far
/// better than HSV value (max channel), which over-reads saturated colour as "bright" — a
/// saturated gold reads value 1.0 but is a rich mid-tone. Grayscale art is unaffected (luma = value
/// when R=G=B), so the tone bands keep their meaning for neutral art while no longer darkening
/// colour just for being saturated.
pub fn median_value(img: &image::RgbaImage, alpha_floor: u8) -> Option<f64> {
    let mut vals: Vec<u8> = img
        .pixels()
        .filter(|p| p.0[3] > alpha_floor)
        .map(|p| (0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64).round() as u8)
        .collect();
    if vals.is_empty() {
        return None;
    }
    let mid = vals.len() / 2;
    let (_, m, _) = vals.select_nth_unstable(mid);
    Some(*m as f64 / 255.0)
}

/// Run the pass on PNG bytes. Returns `(corrected_png, report)`; `corrected_png` is
/// `None` when the asset is already in band (bit-for-bit untouched) or when the
/// median can't be measured/moved.
pub fn tone_pass(png_bytes: &[u8], spec: ToneSpec) -> Result<(Option<Vec<u8>>, ToneReport), String> {
    let img = image::load_from_memory(png_bytes)
        .map_err(|e| format!("tone decode failed: {e}"))?
        .to_rgba8();

    let mut report = ToneReport {
        median_before: 0.0,
        median_after: 0.0,
        band_min: spec.min,
        band_max: spec.max,
        gamma_required: 1.0,
        gamma_applied: 1.0,
        clamped: false,
        ceiling_hit: false,
        flag: "in_band".into(),
    };

    let Some(median) = median_value(&img, spec.alpha_floor) else {
        report.flag = "no_opaque_pixels".into();
        return Ok((None, report));
    };
    report.median_before = median;
    report.median_after = median;

    // In band → untouched. Correct only to the NEAREST edge, never to the centre.
    if median >= spec.min && median <= spec.max {
        return Ok((None, report));
    }
    let target = median.clamp(spec.min, spec.max);

    // A pure-black or pure-white median can't be moved by a gamma curve at all.
    if median <= 1.0 / 255.0 || median >= 1.0 {
        report.flag = "needs_regen".into();
        return Ok((None, report));
    }

    let gamma_required = target.ln() / median.ln();
    let gamma = gamma_required.clamp(spec.gamma_lo, spec.gamma_hi);
    report.gamma_required = gamma_required;
    report.gamma_applied = gamma;
    report.clamped = (gamma - gamma_required).abs() > 1e-9;
    report.flag = if report.clamped { "needs_regen".into() } else { "corrected".into() };

    // Per-channel LUT: v^gamma, capped at the ceiling. Alpha copied through untouched.
    let cap = (spec.ceiling * 255.0).round().min(255.0) as u8;
    let mut ceiling_hit = false;
    let lut: [u8; 256] = std::array::from_fn(|i| {
        let v = ((i as f64 / 255.0).powf(gamma) * 255.0).round() as u32;
        let v = v.min(255) as u8;
        if v > cap {
            ceiling_hit = true;
            cap
        } else {
            v
        }
    });

    let mut out = img;
    for p in out.pixels_mut() {
        p.0[0] = lut[p.0[0] as usize];
        p.0[1] = lut[p.0[1] as usize];
        p.0[2] = lut[p.0[2] as usize];
    }
    // The LUT is monotone, so the median maps through it exactly.
    report.median_after = median_value(&out, spec.alpha_floor).unwrap_or(report.median_before);
    report.ceiling_hit = ceiling_hit && report.median_after >= spec.ceiling - 0.04;

    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("tone encode failed: {e}"))?;
    Ok((Some(buf.into_inner()), report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: ToneSpec = ToneSpec {
        min: 0.18,
        max: 0.22,
        alpha_floor: 200,
        ceiling: 0.92,
        gamma_lo: 0.55,
        gamma_hi: 1.60,
    };

    fn png_of(img: image::RgbaImage) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// Flat grey square at value v (0–255), with a transparent border that must never
    /// be measured.
    fn flat(v: u8) -> Vec<u8> {
        let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        for y in 8..56 {
            for x in 8..56 {
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        png_of(img)
    }

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).unwrap().to_rgba8()
    }

    // 1. Solves correctly: 0.176 with band 0.18–0.22 lands just inside the low edge.
    #[test]
    fn solves_to_the_nearest_band_edge()  {
        let src = flat(45); // 45/255 ≈ 0.176
        let (out, r) = tone_pass(&src, SPEC).unwrap();
        let m = median_value(&decode(&out.expect("corrected")), 200).unwrap();
        assert!(m >= 0.175 && m <= 0.20, "median {m} must land at the low edge");
        assert_eq!(r.flag, "corrected");
        assert!(!r.clamped);
        assert!((r.median_after - m).abs() < 0.005);
    }

    // 2. Flat steps survive: four flat bands stay four flat bands.
    #[test]
    fn flat_steps_stay_flat_and_distinct() {
        let mut img = image::RgbaImage::new(64, 64);
        for (i, v) in [30u8, 60, 90, 120].iter().enumerate() {
            for y in (i as u32 * 16)..((i as u32 + 1) * 16) {
                for x in 0..64 {
                    img.put_pixel(x, y, image::Rgba([*v, *v, *v, 255]));
                }
            }
        }
        let (out, _) = tone_pass(&png_of(img), SPEC).unwrap();
        let out = decode(&out.expect("corrected"));
        let mut bands = std::collections::BTreeSet::new();
        for y in [8u32, 24, 40, 56] {
            let row: Vec<u8> = (0..64).map(|x| out.get_pixel(x, y).0[0]).collect();
            assert!(row.iter().all(|&v| v == row[0]), "band at y={y} must stay FLAT");
            bands.insert(row[0]);
        }
        assert_eq!(bands.len(), 4, "four distinct bands must remain");
    }

    // 3. Depth spread preserved (a lift must not compress internal contrast).
    #[test]
    fn lift_keeps_or_widens_the_depth_spread() {
        let mut img = image::RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                // 3:1 split keeps the MEDIAN on the dark band (35 → 0.137, below
                // the 0.18 floor); spread stays 20/255.
                let v = if y < 48 { 35u8 } else { 55 };
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let (out, _) = tone_pass(&png_of(img), SPEC).unwrap();
        let out = decode(&out.expect("corrected"));
        let spread = out.get_pixel(0, 60).0[0] as i32 - out.get_pixel(0, 4).0[0] as i32;
        assert!(spread >= 20, "spread must not compress (got {spread}, was 20)");
    }

    // 4. Idempotent: a corrected result is in band, so a second run is a no-op.
    #[test]
    fn second_run_is_untouched() {
        let (out, _) = tone_pass(&flat(45), SPEC).unwrap();
        let corrected = out.expect("corrected");
        let (again, r) = tone_pass(&corrected, SPEC).unwrap();
        assert!(again.is_none(), "second run must not touch the asset");
        assert_eq!(r.flag, "in_band");
    }

    // 5. In-band assets untouched.
    #[test]
    fn in_band_passes_through_untouched() {
        let (out, r) = tone_pass(&flat(51), SPEC).unwrap(); // 51/255 = 0.20, in band
        assert!(out.is_none());
        assert_eq!(r.flag, "in_band");
        assert!((r.median_before - 0.2).abs() < 0.01);
    }

    // 6. Clamp path: needing gamma far outside the clamp → clamped + NEEDS_REGEN.
    #[test]
    fn far_off_assets_clamp_and_flag_needs_regen() {
        let (out, r) = tone_pass(&flat(220), SPEC).unwrap(); // 0.86 vs band 0.18–0.22
        assert_eq!(r.flag, "needs_regen");
        assert!(r.clamped);
        assert!((r.gamma_applied - SPEC.gamma_hi).abs() < 1e-9, "applied at the clamp");
        assert!(r.gamma_required > SPEC.gamma_hi);
        let m = median_value(&decode(&out.expect("still corrected, at the clamp")), 200).unwrap();
        assert!(m < 0.86 && m > 0.22, "moved toward the band but not forced into it: {m}");
    }

    // 7. Alpha bit-for-bit intact.
    #[test]
    fn alpha_is_untouched() {
        let mut img = image::RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([45, 45, 45, ((x * 8 + y) % 256) as u8]);
        }
        let src = png_of(img);
        let before = decode(&src);
        let (out, _) = tone_pass(&src, SPEC).unwrap();
        let after = decode(&out.expect("corrected"));
        for (b, a) in before.pixels().zip(after.pixels()) {
            assert_eq!(b.0[3], a.0[3], "alpha must be bit-for-bit identical");
        }
    }

    // 8. Ceiling respected: no output channel exceeds 0.92.
    #[test]
    fn ceiling_is_never_exceeded() {
        let mut img = image::RgbaImage::from_pixel(32, 32, image::Rgba([45, 45, 45, 255]));
        for x in 0..8 {
            img.put_pixel(x, 0, image::Rgba([250, 250, 250, 255])); // hot pixels
        }
        let spec = ToneSpec { min: 0.60, max: 0.65, gamma_lo: 0.30, ..SPEC };
        let (out, _) = tone_pass(&png_of(img), spec).unwrap();
        let out = decode(&out.expect("corrected"));
        let cap = (0.92_f64 * 255.0).round() as u8;
        for p in out.pixels() {
            assert!(p.0[0] <= cap && p.0[1] <= cap && p.0[2] <= cap, "pixel over ceiling");
        }
    }
}
