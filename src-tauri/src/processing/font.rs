//! Deterministic bitmap-font baking. Rasterizes a bundled TrueType face into a colored,
//! outlined glyph atlas and emits a BMFont descriptor — exactly the `.xml` + `.webp` page pair
//! the Stake web-sdk loads as `type: 'font'` (see `apps/lines/static/assets/fonts/goldFont/`).
//!
//! Pure-Rust (no external tool): `ab_glyph` outlines glyphs, we composite a vertical-gradient
//! fill over a dilated outline, shelf-pack into one page, and encode WebP via the `webp` crate.
//! No AI, no per-variation processing — fonts are baked straight from a `FontDef` at export.

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};
use image::{DynamicImage, Rgba, RgbaImage};

use crate::model::game_config::FontDef;

/// Glyphs baked into every font: digits, upper/lowercase, punctuation, currency, math — the set
/// a slot needs for win amounts, multipliers, volatility and labels. Missing glyphs in a given
/// face are skipped gracefully (metrics-only), never fatal.
const GLYPH_SET: &str =
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz .,:;!?'\"$€£¥%+-*/×=@#&()[]x";

/// A bundled typeface the producer can pick.
pub struct Typeface {
    pub id: &'static str,
    pub name: &'static str,
}

/// The typefaces bundled with the app (OFL / Apache licensed, redistributable in exports).
pub fn typefaces() -> Vec<Typeface> {
    vec![
        Typeface { id: "luckiest_guy", name: "Luckiest Guy" },
        Typeface { id: "titan_one", name: "Titan One" },
    ]
}

fn typeface_bytes(id: &str) -> Result<&'static [u8], String> {
    Ok(match id {
        "luckiest_guy" => include_bytes!("../../assets/fonts/LuckiestGuy-Regular.ttf"),
        "titan_one" => include_bytes!("../../assets/fonts/TitanOne-Regular.ttf"),
        other => return Err(format!("unknown typeface: {other}")),
    })
}

fn hex(s: &str) -> [u8; 3] {
    let s = s.trim().trim_start_matches('#');
    let p = |a: usize| u8::from_str_radix(s.get(a..a + 2).unwrap_or("ff"), 16).unwrap_or(255);
    if s.len() >= 6 {
        [p(0), p(2), p(4)]
    } else {
        [255, 255, 255]
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

/// Composite straight-alpha `fg` over `bg` (src-over).
fn over(fg: [u8; 4], bg: Rgba<u8>) -> Rgba<u8> {
    let fa = fg[3] as f32 / 255.0;
    let ba = bg[3] as f32 / 255.0;
    let oa = fa + ba * (1.0 - fa);
    if oa <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let c = |i: usize| ((fg[i] as f32 * fa + bg[i] as f32 * ba * (1.0 - fa)) / oa).round() as u8;
    Rgba([c(0), c(1), c(2), (oa * 255.0).round() as u8])
}

struct Cell {
    ch: char,
    w: u32,
    h: u32,
    xoff: i32,
    yoff: i32,
    xadv: i32,
    buf: RgbaImage, // colored glyph (may be 1x1 for whitespace)
}

/// Rasterize a `FontDef` into `(bmfont_xml, page_webp_bytes)`. The XML's `<page file>` references
/// `<key>.webp`, matching the sibling file the export writes.
pub fn rasterize_font(def: &FontDef) -> Result<(String, Vec<u8>), String> {
    let bytes = typeface_bytes(&def.typeface)?;
    let font = FontRef::try_from_slice(bytes).map_err(|e| format!("load typeface: {e}"))?;
    let px = (def.size_px.max(8)) as f32;
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let ascent = sf.ascent();
    let line_height = (sf.ascent() - sf.descent() + sf.line_gap()).ceil().max(1.0) as i32;
    let base = ascent.ceil() as i32;
    let outline = def.outline_px.min(24) as i32;
    let pad = outline;
    let fill_top = hex(&def.fill_top);
    let fill_bot = hex(&def.fill_bottom);
    let oc = hex(&def.outline_color);

    let mut cells: Vec<Cell> = Vec::new();
    for ch in GLYPH_SET.chars() {
        let gid = font.glyph_id(ch);
        let xadv = sf.h_advance(gid).round().max(0.0) as i32;
        let glyph: Glyph = gid.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
        let Some(outlined) = font.outline_glyph(glyph) else {
            // whitespace / face has no such glyph → metrics-only char.
            cells.push(Cell { ch, w: 0, h: 0, xoff: 0, yoff: 0, xadv, buf: RgbaImage::new(1, 1) });
            continue;
        };
        let bb = outlined.px_bounds();
        let gw = (bb.max.x - bb.min.x).ceil().max(1.0) as i32;
        let gh = (bb.max.y - bb.min.y).ceil().max(1.0) as i32;
        let mut cov = vec![0f32; (gw * gh) as usize];
        outlined.draw(|x, y, c| {
            let (x, y) = (x as i32, y as i32);
            if x >= 0 && y >= 0 && x < gw && y < gh {
                cov[(y * gw + x) as usize] = c;
            }
        });

        let cw = (gw + 2 * pad).max(1) as u32;
        let chh = (gh + 2 * pad).max(1) as u32;
        let mut buf = RgbaImage::new(cw, chh);

        // Outline pass: dilate the coverage mask by `outline` px, paint the outline color.
        if outline > 0 {
            for cy in 0..chh as i32 {
                for cx in 0..cw as i32 {
                    let mut m = 0f32;
                    'k: for dy in -outline..=outline {
                        for dx in -outline..=outline {
                            if dx * dx + dy * dy > outline * outline {
                                continue;
                            }
                            let (sx, sy) = (cx - pad + dx, cy - pad + dy);
                            if sx >= 0 && sy >= 0 && sx < gw && sy < gh {
                                m = m.max(cov[(sy * gw + sx) as usize]);
                                if m >= 1.0 {
                                    break 'k;
                                }
                            }
                        }
                    }
                    if m > 0.0 {
                        buf.put_pixel(cx as u32, cy as u32, Rgba([oc[0], oc[1], oc[2], (m * 255.0) as u8]));
                    }
                }
            }
        }

        // Fill pass: vertical gradient by glyph row, composited over the outline.
        for gy in 0..gh {
            let t = if gh > 1 { gy as f32 / (gh - 1) as f32 } else { 0.0 };
            let col = [lerp(fill_top[0], fill_bot[0], t), lerp(fill_top[1], fill_bot[1], t), lerp(fill_top[2], fill_bot[2], t)];
            for gx in 0..gw {
                let a = cov[(gy * gw + gx) as usize];
                if a <= 0.0 {
                    continue;
                }
                let (dx, dy) = ((gx + pad) as u32, (gy + pad) as u32);
                let bg = *buf.get_pixel(dx, dy);
                let out = over([col[0], col[1], col[2], (a * 255.0) as u8], bg);
                buf.put_pixel(dx, dy, out);
            }
        }

        let xoff = bb.min.x.round() as i32 - pad;
        let yoff = (ascent + bb.min.y).round() as i32 - pad;
        cells.push(Cell { ch, w: cw, h: chh, xoff, yoff, xadv, buf });
    }

    // Shelf-pack (tallest first) into one page.
    let margin = 1u32;
    let mut order: Vec<usize> = (0..cells.len()).filter(|&i| cells[i].w > 0).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(cells[i].h));
    let area: i64 = order.iter().map(|&i| (cells[i].w * cells[i].h) as i64).sum();
    let atlas_w = ((area as f64).sqrt() * 1.3).max(64.0) as u32;
    let atlas_w = atlas_w.next_power_of_two().min(4096).max(64);

    let mut pos = vec![(0u32, 0u32); cells.len()];
    let (mut sx, mut sy, mut shelf_h) = (margin, margin, 0u32);
    for &i in &order {
        let (w, h) = (cells[i].w, cells[i].h);
        if sx + w + margin > atlas_w {
            sx = margin;
            sy += shelf_h + margin;
            shelf_h = 0;
        }
        pos[i] = (sx, sy);
        sx += w + margin;
        shelf_h = shelf_h.max(h);
    }
    let atlas_h = (sy + shelf_h + margin).next_power_of_two().min(4096).max(16);

    let mut page = RgbaImage::new(atlas_w, atlas_h);
    for &i in &order {
        let (x, y) = pos[i];
        image::imageops::overlay(&mut page, &cells[i].buf, x as i64, y as i64);
    }

    // BMFont XML (matches the web-sdk's mm_gold.xml shape).
    let page_file = format!("{}.webp", def.key);
    let mut chars = String::new();
    let mut count = 0u32;
    for (i, c) in cells.iter().enumerate() {
        let (x, y) = pos[i];
        chars.push_str(&format!(
            "    <char id=\"{id}\" x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" xoffset=\"{xo}\" yoffset=\"{yo}\" xadvance=\"{xa}\" page=\"0\" chnl=\"15\"/>\n",
            id = c.ch as u32, w = c.w, h = c.h, xo = c.xoff, yo = c.yoff, xa = c.xadv,
        ));
        count += 1;
    }
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<font>\n  <info face=\"{face}\" size=\"{size}\" bold=\"0\" italic=\"0\" charset=\"\" unicode=\"1\" stretchH=\"100\" smooth=\"1\" aa=\"1\" padding=\"0,0,0,0\" spacing=\"1,1\" outline=\"0\"/>\n  <common lineHeight=\"{lh}\" base=\"{base}\" scaleW=\"{sw}\" scaleH=\"{sh}\" pages=\"1\" packed=\"0\"/>\n  <pages>\n    <page id=\"0\" file=\"{page_file}\"/>\n  </pages>\n  <chars count=\"{count}\">\n{chars}  </chars>\n</font>\n",
        face = def.key, size = def.size_px, lh = line_height, base = base, sw = atlas_w, sh = atlas_h,
    );

    let webp = webp::Encoder::from_image(&DynamicImage::ImageRgba8(page))
        .map_err(|e| format!("font webp encode: {e}"))?
        .encode(100.0)
        .to_vec();

    Ok((xml, webp))
}

/// Rasterize a short sample string for the UI preview (a standalone PNG, transparent bg).
pub fn preview_png(def: &FontDef, sample: &str) -> Result<Vec<u8>, String> {
    let bytes = typeface_bytes(&def.typeface)?;
    let font = FontRef::try_from_slice(bytes).map_err(|e| format!("load typeface: {e}"))?;
    let px = (def.size_px.max(8)) as f32;
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let ascent = sf.ascent();
    let outline = def.outline_px.min(24) as i32;
    let pad = outline + 2;
    let fill_top = hex(&def.fill_top);
    let fill_bot = hex(&def.fill_bottom);
    let oc = hex(&def.outline_color);

    // Measure the run.
    let mut pen = 0f32;
    let mut layout: Vec<(ab_glyph::OutlinedGlyph, f32)> = Vec::new();
    for ch in sample.chars() {
        let gid = font.glyph_id(ch);
        let adv = sf.h_advance(gid);
        let glyph: Glyph = gid.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
        if let Some(o) = font.outline_glyph(glyph) {
            layout.push((o, pen));
        }
        pen += adv;
    }
    let w = (pen.ceil() as i32 + 2 * pad).max(1) as u32;
    let h = (sf.ascent() - sf.descent()).ceil() as i32 + 2 * pad;
    let h = h.max(1) as u32;
    let mut img = RgbaImage::new(w, h);

    for (o, penx) in &layout {
        let bb = o.px_bounds();
        let gw = (bb.max.x - bb.min.x).ceil().max(1.0) as i32;
        let gh = (bb.max.y - bb.min.y).ceil().max(1.0) as i32;
        let mut cov = vec![0f32; (gw * gh) as usize];
        o.draw(|x, y, c| {
            let (x, y) = (x as i32, y as i32);
            if x >= 0 && y >= 0 && x < gw && y < gh {
                cov[(y * gw + x) as usize] = c;
            }
        });
        let ox = (penx + bb.min.x).round() as i32 + pad;
        let oy = (ascent + bb.min.y).round() as i32 + pad;
        // outline
        if outline > 0 {
            for cy in -outline..gh + outline {
                for cx in -outline..gw + outline {
                    let mut m = 0f32;
                    'k: for dy in -outline..=outline {
                        for dx in -outline..=outline {
                            if dx * dx + dy * dy > outline * outline {
                                continue;
                            }
                            let (sx, sy) = (cx + dx, cy + dy);
                            if sx >= 0 && sy >= 0 && sx < gw && sy < gh {
                                m = m.max(cov[(sy * gw + sx) as usize]);
                                if m >= 1.0 {
                                    break 'k;
                                }
                            }
                        }
                    }
                    let (px_, py_) = (ox + cx, oy + cy);
                    if m > 0.0 && px_ >= 0 && py_ >= 0 && (px_ as u32) < w && (py_ as u32) < h {
                        let bg = *img.get_pixel(px_ as u32, py_ as u32);
                        let out = over([oc[0], oc[1], oc[2], (m * 255.0) as u8], bg);
                        img.put_pixel(px_ as u32, py_ as u32, out);
                    }
                }
            }
        }
        // fill
        for gy in 0..gh {
            let t = if gh > 1 { gy as f32 / (gh - 1) as f32 } else { 0.0 };
            let col = [lerp(fill_top[0], fill_bot[0], t), lerp(fill_top[1], fill_bot[1], t), lerp(fill_top[2], fill_bot[2], t)];
            for gx in 0..gw {
                let a = cov[(gy * gw + gx) as usize];
                if a <= 0.0 {
                    continue;
                }
                let (px_, py_) = (ox + gx, oy + gy);
                if px_ >= 0 && py_ >= 0 && (px_ as u32) < w && (py_ as u32) < h {
                    let bg = *img.get_pixel(px_ as u32, py_ as u32);
                    let out = over([col[0], col[1], col[2], (a * 255.0) as u8], bg);
                    img.put_pixel(px_ as u32, py_ as u32, out);
                }
            }
        }
    }

    let mut png = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("preview png encode: {e}"))?;
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> FontDef {
        FontDef {
            key: "font_gold".into(),
            name: "Gold".into(),
            typeface: "luckiest_guy".into(),
            size_px: 80,
            fill_top: "#ffe89a".into(),
            fill_bottom: "#e0a13a".into(),
            outline_color: "#4a2d0a".into(),
            outline_px: 6,
        }
    }

    #[test]
    fn rasterizes_a_valid_bmfont() {
        let (xml, webp) = rasterize_font(&def()).expect("rasterize");
        // BMFont shape: info/common/pages/chars, page references <key>.webp.
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<font>"));
        assert!(xml.contains("face=\"font_gold\""));
        assert!(xml.contains("<page id=\"0\" file=\"font_gold.webp\"/>"));
        // digits present (id 48..57).
        assert!(xml.contains("id=\"48\"")); // '0'
        assert!(xml.contains("id=\"36\"")); // '$'
        // scaleW/scaleH announced; a real char has non-zero size.
        assert!(xml.contains("scaleW="));
        assert!(xml.contains("width=\""));
        // The page is a decodable WebP.
        assert!(webp.len() > 100, "webp encoded ({} bytes)", webp.len());
        let decoded = image::load_from_memory(&webp).expect("decode webp");
        assert!(decoded.width() >= 64 && decoded.height() >= 16);
    }

    #[test]
    fn char_count_matches_chars() {
        let (xml, _) = rasterize_font(&def()).unwrap();
        let count_attr: usize = xml
            .split("<chars count=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse().ok())
            .expect("count attr");
        let char_tags = xml.matches("<char ").count();
        assert_eq!(count_attr, char_tags, "declared count == actual <char> tags");
    }

    #[test]
    fn preview_is_a_png() {
        let png = preview_png(&def(), "1,234.56").expect("preview");
        let img = image::load_from_memory(&png).expect("decode png");
        assert!(img.width() > 10 && img.height() > 10);
    }
}
