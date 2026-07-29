//! Symbol fit pass: scale, centre and re-canvas each keyed cut-out so every symbol in
//! a pay class carries comparable VISUAL WEIGHT — the eye reads ink (covered pixels),
//! not height. A hat and a hand at equal height differ ~3× in ink and read as
//! different pay classes; pure ink normalisation instead makes slim objects comically
//! large. So the scale is a weighted GEOMETRIC blend of a height-based and an
//! ink-based candidate, hard-clamped to the safe box, anchored centroid-biased.
//!
//! Runs after keying, before compression. Never re-keys. Deterministic and always
//! derived from the canonical keyed input, so re-running can never compound scaling.
//!
//! Possible follow-up (measured, not built): perceived size also tracks VALUE — a
//! bright object reads larger than a dark one at identical ink. A luminance-weighted
//! ink metric (weight = alpha · (0.3 + 0.7·luminance)) would capture this, but it is
//! speculative and needs tuning against real art before it earns a config field.

use image::RgbaImage;

use crate::model::asset_record::MassReport;
use crate::model::game_config::FitClass;

/// Everything the fit pass needs for one symbol (all per-project config + the
/// per-symbol footprint nudge).
#[derive(Debug, Clone, Copy)]
pub struct MassSpec {
    pub class: FitClass,
    pub area_weight: f64,
    pub centroid_bias: f64,
    pub alpha_floor: u8,
    pub safe_w: f64,
    pub safe_h: f64,
    /// Per-symbol footprint vs the class default: scales the class TARGET (ink ∝ nudge²
    /// because ink is an area, height ∝ nudge because it is linear). So 0.6 ships this
    /// symbol at ~60% of its class footprint and 1.0 = the class norm. This is what keeps
    /// relative scale between symbols (a coin small, a dragon large) instead of inflating
    /// everything to one class target. Still capped by the safe box — it can never
    /// overflow the cell.
    pub nudge: f64,
    /// Trust a set sheet's own relative scale: skip the class-target rescale entirely and
    /// keep each cut cell at its native size (only re-center it). Set for `provider="set"`
    /// variations, where the model already sized every symbol against its neighbours on
    /// one sheet.
    pub trust_scale: bool,
    /// Fixed square output canvas (0 = keep the source canvas).
    pub canvas: u32,
}

/// Single-pass alpha metrics: ink, bbox, centroid.
struct Metrics {
    ink: f64,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    cx: f64,
    cy: f64,
}

fn measure(img: &RgbaImage, floor: u8) -> Option<Metrics> {
    let (w, h) = img.dimensions();
    let (mut ink, mut wx, mut wy) = (0f64, 0f64, 0f64);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        let a = p.0[3];
        if a < floor {
            continue;
        }
        let af = a as f64 / 255.0;
        ink += af;
        wx += x as f64 * af;
        wy += y as f64 * af;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if ink <= 0.0 || x1 < x0 {
        return None;
    }
    Some(Metrics { ink, x0, y0, x1, y1, cx: wx / ink, cy: wy / ink })
}

/// Fit one keyed cut-out. Returns `(png, report)`; `(None, None)` when the image has
/// no usable transparency to measure against (opaque source — nothing to fit), and an
/// error when alpha exists but carries no ink (a broken, fully transparent symbol).
pub fn fit_symbol(
    png_bytes: &[u8],
    spec: MassSpec,
) -> Result<(Option<Vec<u8>>, Option<MassReport>), String> {
    let img = image::load_from_memory(png_bytes)
        .map_err(|e| format!("decode for fit failed: {e}"))?
        .to_rgba8();
    let (sw, sh) = img.dimensions();

    // Needs a real background to measure against.
    let transparent = img.pixels().filter(|p| p.0[3] < 10).count();
    if (transparent as f64) < 0.01 * (sw as f64 * sh as f64) {
        return Ok((None, None));
    }

    let m = measure(&img, spec.alpha_floor.max(1))
        .ok_or_else(|| "no ink above the alpha floor — the symbol is fully transparent".to_string())?;

    // The cell = the output canvas.
    let (ow, oh) = if spec.canvas > 0 { (spec.canvas, spec.canvas) } else { (sw, sh) };
    let cell_area = ow as f64 * oh as f64;
    let bbox_w = (m.x1 - m.x0 + 1) as f64;
    let bbox_h = (m.y1 - m.y0 + 1) as f64;

    // 2. Scale. The safe box is a hard cap on the bbox in every mode.
    let class = spec.class;
    let nudge = spec.nudge.clamp(0.25, 4.0);
    let s_max = ((spec.safe_w.clamp(0.1, 1.0) * ow as f64) / bbox_w)
        .min((spec.safe_h.clamp(0.1, 1.0) * oh as f64) / bbox_h);

    // Class targets scaled by this symbol's authored footprint: ink is an AREA (∝ nudge²),
    // height is LINEAR (∝ nudge). Reported below so the flag reflects the authored intent.
    let eff_ink = (class.ink * nudge * nudge).clamp(0.01, 1.0);
    let eff_height = (class.height * nudge).clamp(0.05, 1.0);
    let target_ink = eff_ink * cell_area;
    let s_height = (eff_height * oh as f64) / bbox_h;
    let s_area = (target_ink / m.ink).sqrt();

    let (s_final, box_clamped) = if spec.trust_scale {
        // Trust the sheet: keep the cut cell's native size (only re-centered below) so the
        // model's own relative scale between symbols survives Process instead of every
        // cell being re-normalised to its class target.
        (1.0, false)
    } else {
        // Author intent: geometric blend of the height candidate and the ink candidate,
        // hard-clamped to the safe box.
        let w = spec.area_weight.clamp(0.0, 1.0);
        let s_raw = s_height.powf(1.0 - w) * s_area.powf(w);
        (s_raw.min(s_max), s_raw > s_max * 1.001)
    };
    let upscaled = s_final > 1.15;

    // 3. Place: uniform Lanczos resample of the WHOLE image (keeps faint wisps),
    // anchored centroid-biased, then clamped into the safe box.
    let rw = ((sw as f64 * s_final).round() as u32).max(1);
    let rh = ((sh as f64 * s_final).round() as u32).max(1);
    let scaled = image::imageops::resize(&img, rw, rh, image::imageops::FilterType::Lanczos3);

    let bias = spec.centroid_bias.clamp(0.0, 1.0);
    let anchor_x = bias * m.cx + (1.0 - bias) * ((m.x0 + m.x1) as f64 / 2.0);
    let anchor_y = bias * m.cy + (1.0 - bias) * ((m.y0 + m.y1) as f64 / 2.0);
    let mut dx = ow as f64 / 2.0 - anchor_x * s_final;
    let mut dy = oh as f64 / 2.0 - anchor_y * s_final;

    // Safe-box placement clamp (the bbox must sit fully inside).
    let (min_x, min_y) = ((1.0 - spec.safe_w) / 2.0 * ow as f64, (1.0 - spec.safe_h) / 2.0 * oh as f64);
    let (max_x, max_y) = (ow as f64 - min_x, oh as f64 - min_y);
    let (bx0, bx1) = (m.x0 as f64 * s_final + dx, (m.x1 + 1) as f64 * s_final + dx);
    let (by0, by1) = (m.y0 as f64 * s_final + dy, (m.y1 + 1) as f64 * s_final + dy);
    let mut placement_clamped = false;
    if bx0 < min_x {
        dx += min_x - bx0;
        placement_clamped = true;
    } else if bx1 > max_x {
        dx -= bx1 - max_x;
        placement_clamped = true;
    }
    if by0 < min_y {
        dy += min_y - by0;
        placement_clamped = true;
    } else if by1 > max_y {
        dy -= by1 - max_y;
        placement_clamped = true;
    }
    let (dx, dy) = (dx.round() as i64, dy.round() as i64);

    let mut out = RgbaImage::new(ow, oh);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let sx = x as i64 - dx;
        let sy = y as i64 - dy;
        if sx >= 0 && sy >= 0 && (sx as u32) < rw && (sy as u32) < rh {
            *px = *scaled.get_pixel(sx as u32, sy as u32);
        }
    }

    // 4. Report. Target reflects the authored footprint (class × nudge²); a trusted set
    //    cell is never flagged — its scale is the sheet's, not a class target to hit.
    let ink_before = m.ink / cell_area;
    let ink_after = m.ink * s_final * s_final / cell_area;
    let flag = if spec.trust_scale {
        "ok"
    } else if ink_after < eff_ink - class.tolerance {
        "underweight"
    } else if ink_after > eff_ink + class.tolerance {
        "overweight"
    } else {
        "ok"
    };
    let report = MassReport {
        achieved: ink_after,
        target: eff_ink,
        clamped: box_clamped || placement_clamped,
        clamped_by: if box_clamped {
            "box".into()
        } else if placement_clamped {
            "placement".into()
        } else {
            "none".into()
        },
        ink_before,
        scale_height: s_height,
        scale_area: s_area,
        scale_final: s_final,
        flag: flag.into(),
        upscaled,
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode fitted failed: {e}"))?;
    Ok((Some(buf.into_inner()), Some(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(ink: f64, height: f64, tol: f64) -> MassSpec {
        MassSpec {
            class: FitClass { ink, height, tolerance: tol },
            area_weight: 0.65,
            centroid_bias: 0.7,
            alpha_floor: 26,
            safe_w: 0.92,
            safe_h: 0.88,
            nudge: 1.0,
            trust_scale: false,
            canvas: 0,
        }
    }

    fn png(img: &RgbaImage) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn solid_rect(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, image::Rgba([180, 60, 60, 255]));
            }
        }
        img
    }

    fn ink_pct(bytes: &[u8], floor: u8) -> f64 {
        let img = image::load_from_memory(bytes).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        img.pixels()
            .filter(|p| p.0[3] >= floor)
            .map(|p| p.0[3] as f64 / 255.0)
            .sum::<f64>()
            / (w as f64 * h as f64)
    }

    /// The original bug: a solid "hat" and a slimmer "hand" at identical height must
    /// land at comparable ink after the fit. Limits baked into the spec's own numbers:
    /// the blend leaves a residual of r^-0.7 (r = s_area/s_height), so ±0.03 tolerance
    /// holds only for objects ≥ ~74% relative coverage; the safe box caps recovery at
    /// ~78%. Slimmer than that = clamped and FLAGGED (see the sliver test) — the
    /// spec's "surface it, don't fudge it" contract, an art problem by definition.
    #[test]
    fn hat_and_hand_reach_comparable_ink() {
        // "Hat": solid 267×500 (aspect chosen so height- and ink-fit agree, the
        // typical half-bbox-coverage symbol). "Hand": four bars, same 500 height,
        // ~78% of the hat's ink — the slim end of the recoverable window.
        let hat = solid_rect(1000, 1000, 366, 250, 633, 750);
        let mut hand = RgbaImage::new(1000, 1000);
        for i in 0..4u32 {
            let x = 372 + i * 68;
            for y in 250..750 {
                for xx in x..x + 52 {
                    hand.put_pixel(xx, y, image::Rgba([200, 170, 140, 255]));
                }
            }
        }
        let s = spec(0.30, 0.75, 0.03);
        let (hat_out, hat_rep) = fit_symbol(&png(&hat), s).unwrap();
        let (hand_out, hand_rep) = fit_symbol(&png(&hand), s).unwrap();
        let hat_ink = ink_pct(&hat_out.unwrap(), 26);
        let hand_ink = ink_pct(&hand_out.unwrap(), 26);
        assert_eq!(hat_rep.unwrap().flag, "ok", "hat within tolerance");
        assert_eq!(hand_rep.unwrap().flag, "ok", "hand within tolerance");
        assert!(
            (hat_ink - hand_ink).abs() < 0.05,
            "comparable weight: hat {hat_ink:.3} vs hand {hand_ink:.3}"
        );
    }

    /// No output pixel above the alpha floor may sit outside the safe box.
    #[test]
    fn nothing_overflows_the_safe_box() {
        for img in [
            solid_rect(1000, 1000, 100, 100, 900, 900), // oversized
            solid_rect(1000, 1000, 480, 100, 520, 900), // sliver
            solid_rect(1000, 1000, 50, 400, 950, 600),  // wide band
        ] {
            let (out, _) = fit_symbol(&png(&img), spec(0.30, 0.75, 0.03)).unwrap();
            let fitted = image::load_from_memory(&out.unwrap()).unwrap().to_rgba8();
            let (w, h) = fitted.dimensions();
            let (mx, my) = ((1.0 - 0.92) / 2.0 * w as f64, (1.0 - 0.88) / 2.0 * h as f64);
            for (x, y, p) in fitted.enumerate_pixels() {
                if p.0[3] >= 26 {
                    assert!(
                        x as f64 >= mx - 1.0
                            && (x as f64) <= w as f64 - mx + 1.0
                            && y as f64 >= my - 1.0
                            && (y as f64) <= h as f64 - my + 1.0,
                        "ink outside the safe box at {x},{y}"
                    );
                }
            }
        }
    }

    /// Deterministic: the same input yields byte-identical output (the pipeline always
    /// re-derives from the canonical keyed artefact, so scaling can never compound).
    #[test]
    fn fit_is_deterministic() {
        let img = solid_rect(1000, 1000, 300, 200, 640, 820);
        let a = fit_symbol(&png(&img), spec(0.30, 0.75, 0.03)).unwrap().0.unwrap();
        let b = fit_symbol(&png(&img), spec(0.30, 0.75, 0.03)).unwrap().0.unwrap();
        assert_eq!(a, b);
    }

    /// A 1:12 sliver clamps on the box, reports it, flags UNDERWEIGHT, and survives.
    #[test]
    fn extreme_aspect_clamps_and_flags() {
        let sliver = solid_rect(1200, 1200, 580, 60, 620, 1140); // 40×1080 ≈ 1:27
        let (out, rep) = fit_symbol(&png(&sliver), spec(0.30, 0.75, 0.03)).unwrap();
        let rep = rep.unwrap();
        assert!(out.is_some());
        assert_eq!(rep.clamped_by, "box");
        assert_eq!(rep.flag, "underweight");
        assert!(rep.achieved < 0.30);
    }

    /// A fully transparent symbol errors clearly instead of dividing by zero.
    #[test]
    fn empty_input_is_a_clear_error() {
        let empty = RgbaImage::new(400, 400);
        // Give it a couple of sub-floor haze pixels so "has transparency" passes.
        let mut hazy = empty.clone();
        hazy.put_pixel(10, 10, image::Rgba([255, 255, 255, 5]));
        let err = fit_symbol(&png(&hazy), spec(0.30, 0.75, 0.03)).unwrap_err();
        assert!(err.contains("alpha floor"), "{err}");
    }

    /// The fixed-canvas option re-canvases to the requested square.
    #[test]
    fn fixed_canvas_recanvases() {
        // Same agreeing-aspect subject as the hat, on a non-square source canvas.
        let img = solid_rect(1000, 800, 366, 150, 633, 650);
        let mut s = spec(0.30, 0.75, 0.03);
        s.canvas = 512;
        let (out, rep) = fit_symbol(&png(&img), s).unwrap();
        let fitted = image::load_from_memory(&out.unwrap()).unwrap().to_rgba8();
        assert_eq!(fitted.dimensions(), (512, 512));
        assert_eq!(rep.unwrap().flag, "ok");
    }

    /// The per-symbol nudge now scales the FOOTPRINT (the target), so the same art
    /// authored at 0.6 ships far smaller than at 1.0 — the relative-scale control (coin
    /// vs dragon), not the old box-capped final multiply. And it reports OK against its
    /// own scaled target, not UNDERWEIGHT vs the class.
    #[test]
    fn nudge_scales_the_authored_footprint() {
        let img = solid_rect(1000, 1000, 366, 250, 633, 750);
        let big = fit_symbol(&png(&img), spec(0.30, 0.75, 0.03)).unwrap();
        let mut small_spec = spec(0.30, 0.75, 0.03);
        small_spec.nudge = 0.6;
        let small = fit_symbol(&png(&img), small_spec).unwrap();
        let big_ink = ink_pct(&big.0.unwrap(), 26);
        let small_ink = ink_pct(&small.0.unwrap(), 26);
        assert!(
            small_ink < big_ink * 0.5,
            "nudge 0.6 ships far less ink: {small_ink:.3} vs {big_ink:.3}"
        );
        assert_eq!(small.1.unwrap().flag, "ok");
    }

    /// Trust mode keeps a set cell at its native size (only re-centers), so the sheet's
    /// relative scale survives: a subject twice the linear size of another stays ~4× the
    /// ink afterwards, instead of both being normalised to one class target.
    #[test]
    fn trust_scale_keeps_native_relative_size() {
        let big = solid_rect(1000, 1000, 300, 300, 700, 700); // 400²
        let small = solid_rect(1000, 1000, 400, 400, 600, 600); // 200²
        let mut s = spec(0.30, 0.75, 0.03);
        s.trust_scale = true;
        let big_out = fit_symbol(&png(&big), s).unwrap();
        let small_out = fit_symbol(&png(&small), s).unwrap();
        let big_ink = ink_pct(&big_out.0.unwrap(), 26);
        let small_ink = ink_pct(&small_out.0.unwrap(), 26);
        assert!(
            big_ink > small_ink * 3.0,
            "native relative scale preserved: big {big_ink:.3} vs small {small_ink:.3}"
        );
        assert_eq!(big_out.1.unwrap().flag, "ok", "trusted cells are never flagged");
    }
}
