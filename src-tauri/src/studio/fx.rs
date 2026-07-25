//! AI-generated FX layers (glows, flares, auras). The trick: for ADDITIVE blending,
//! black is invisible — so we ask gpt-image-2 to paint ONLY the light effect on pure
//! black, in place over the symbol (the same in-place registration trick inpainting
//! uses). Luminance then becomes alpha (with an un-premultiply so `rgb × a` still equals
//! the generated light exactly), and the result is an ordinary part on an additive slot.

use image::RgbaImage;

use super::doc::Rect;

/// Prompt for the in-place FX edit.
pub fn fx_prompt(brief: &str) -> String {
    format!(
        "Return an image the SAME size and framing as the reference. Render ONLY this light \
effect, positioned where it belongs relative to the character: {brief}.\n\
Rules: the effect is painted as PURE LIGHT on a PURE BLACK (#000000) background — soft \
glows, flares, rays or embers exactly where they should appear. Do NOT draw the character \
itself, its outline, or any silhouette; only the light. Everything that is not glowing is \
pure black. No text, no frame, no watermark.",
    )
}

/// Flatten a transparent source over black (what the model sees as the reference).
pub fn flatten_on_black(source: &RgbaImage) -> RgbaImage {
    let (w, h) = source.dimensions();
    RgbaImage::from_fn(w, h, |x, y| {
        let p = source.get_pixel(x, y);
        let a = p.0[3] as u32;
        let ch = |c: u8| ((c as u32 * a) / 255) as u8;
        image::Rgba([ch(p.0[0]), ch(p.0[1]), ch(p.0[2]), 255])
    })
}

/// Black-background light → straight-alpha RGBA: `a = max(r,g,b)`, RGB un-premultiplied
/// (`rgb' = rgb/a`) so that compositing `rgb' × a` reproduces the generated light exactly
/// under additive blending. Near-black noise (a ≤ 6) is clamped to fully transparent.
pub fn luminance_to_alpha(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    RgbaImage::from_fn(w, h, |x, y| {
        let p = img.get_pixel(x, y);
        let a = p.0[0].max(p.0[1]).max(p.0[2]);
        if a <= 6 {
            return image::Rgba([0, 0, 0, 0]);
        }
        let un = |c: u8| ((c as u32 * 255) / a as u32).min(255) as u8;
        image::Rgba([un(p.0[0]), un(p.0[1]), un(p.0[2]), a])
    })
}

/// Tight bbox of non-transparent pixels; `None` if the image is fully dark.
pub fn alpha_bbox(img: &RgbaImage) -> Option<Rect> {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    let mut any = false;
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[3] > 0 {
            any = true;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    any.then(|| Rect { x: x0 as i32, y: y0 as i32, w: x1 - x0 + 1, h: y1 - y0 + 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_transform_preserves_additive_light_exactly() {
        // A dim orange glow pixel, a bright white core, and black background.
        let mut img = RgbaImage::from_pixel(4, 1, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([120, 60, 20, 255]));
        img.put_pixel(2, 0, image::Rgba([255, 255, 255, 255]));

        let out = luminance_to_alpha(&img);
        // Black → fully transparent (adds nothing either way).
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 0]);
        // rgb' × a reproduces the original light within rounding.
        let p = out.get_pixel(1, 0);
        for (c, orig) in [(0usize, 120u32), (1, 60), (2, 20)] {
            let recon = (p.0[c] as u32 * p.0[3] as u32 + 127) / 255;
            assert!(
                (recon as i32 - orig as i32).abs() <= 2,
                "channel {c}: recon {recon} vs {orig}"
            );
        }
        assert_eq!(p.0[3], 120); // alpha = max channel
        assert_eq!(out.get_pixel(2, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn flatten_and_bbox() {
        let mut src = RgbaImage::new(6, 4);
        src.put_pixel(2, 1, image::Rgba([200, 100, 50, 128])); // semi-transparent
        let flat = flatten_on_black(&src);
        assert_eq!(flat.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(flat.get_pixel(2, 1).0[0], 100); // premultiplied over black

        let lit = luminance_to_alpha(&flat);
        let bb = alpha_bbox(&lit).unwrap();
        assert_eq!((bb.x, bb.y, bb.w, bb.h), (2, 1, 1, 1));
        assert!(alpha_bbox(&luminance_to_alpha(&RgbaImage::from_pixel(
            3,
            3,
            image::Rgba([0, 0, 0, 255])
        )))
        .is_none());
    }
}
