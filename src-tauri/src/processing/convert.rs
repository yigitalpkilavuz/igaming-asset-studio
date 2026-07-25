//! Rust-native resize + format conversion. No external tools.

use image::{DynamicImage, ImageFormat};

pub struct ConvertOutputs {
    pub webp: Vec<u8>,
    pub secondary: Vec<u8>,
    /// `"png"` for alpha assets, `"jpg"` for opaque ones.
    pub secondary_ext: &'static str,
}

/// Resize an image (any decodable format) to exact author dims and encode a WebP at the
/// given quality (the studio default is 85; scene exports use the project's setting)
/// plus a secondary twin: PNG for alpha assets, JPG for opaque backgrounds.
pub fn resize_convert(
    bytes: &[u8],
    target_w: u32,
    target_h: u32,
    opaque: bool,
    webp_quality: f32,
) -> Result<ConvertOutputs, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode failed: {e}"))?;
    let resized = img.resize_exact(
        target_w.max(1),
        target_h.max(1),
        image::imageops::FilterType::Lanczos3,
    );

    let webp = webp::Encoder::from_image(&resized)
        .map_err(|e| format!("webp encode failed: {e}"))?
        .encode(webp_quality.clamp(10.0, 100.0))
        .to_vec();

    let (secondary, secondary_ext) = if opaque {
        let rgb = DynamicImage::ImageRgb8(resized.to_rgb8());
        let mut buf = std::io::Cursor::new(Vec::new());
        rgb.write_to(&mut buf, ImageFormat::Jpeg)
            .map_err(|e| format!("jpg encode failed: {e}"))?;
        (buf.into_inner(), "jpg")
    } else {
        let mut buf = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut buf, ImageFormat::Png)
            .map_err(|e| format!("png encode failed: {e}"))?;
        (buf.into_inner(), "png")
    };

    Ok(ConvertOutputs {
        webp,
        secondary,
        secondary_ext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a tiny solid RGBA PNG to bytes for use as test input.
    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([180, 40, 60, 255]));
        let dynimg = DynamicImage::ImageRgba8(img);
        let mut buf = std::io::Cursor::new(Vec::new());
        dynimg.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn produces_webp_and_png_for_alpha() {
        let png = sample_png(64, 64);
        let out = resize_convert(&png, 220, 220, false, 85.0).unwrap();
        assert!(!out.webp.is_empty());
        assert_eq!(out.secondary_ext, "png");
        // The PNG twin decodes back to the requested size.
        let back = image::load_from_memory(&out.secondary).unwrap();
        assert_eq!((back.width(), back.height()), (220, 220));
    }

    #[test]
    fn produces_jpg_for_opaque() {
        let png = sample_png(64, 64);
        let out = resize_convert(&png, 320, 180, true, 85.0).unwrap();
        assert_eq!(out.secondary_ext, "jpg");
        assert!(!out.secondary.is_empty());
    }
}
