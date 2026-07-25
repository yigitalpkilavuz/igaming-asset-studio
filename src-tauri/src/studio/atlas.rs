//! Rust-native texture atlas packer. Shelf-packs part images into ≤2048² pages and writes
//! the legacy libgdx/Spine text `.atlas` format, which the spine-ts 4.2 `TextureAtlas`
//! parser accepts. Pages are named `<base>.webp`, `<base>_2.webp`, … and encoded as both
//! WebP (shipped, referenced by the atlas text) and PNG (debug/tooling copy).

use image::{DynamicImage, ImageFormat, RgbaImage};

const MAX: u32 = 2048;
const PAD: u32 = 2;

pub struct PackInput {
    /// Atlas region name (the part id).
    pub name: String,
    pub image: RgbaImage,
}

pub struct Page {
    /// File name referenced by the atlas text, e.g. `symbol_h1.webp`.
    pub name: String,
    pub webp: Vec<u8>,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct PackResult {
    pub atlas_text: String,
    pub pages: Vec<Page>,
}

struct Placed {
    input_idx: usize,
    page: usize,
    x: u32,
    y: u32,
}

/// Pack the inputs into pages. A region larger than a page is an error.
pub fn pack(inputs: &[PackInput], base_name: &str) -> Result<PackResult, String> {
    if inputs.is_empty() {
        return Err("no textures to pack".into());
    }
    for i in inputs {
        if i.image.width() + 2 * PAD > MAX || i.image.height() + 2 * PAD > MAX {
            return Err(format!(
                "part \"{}\" is {}×{} — larger than a {MAX}² atlas page",
                i.name,
                i.image.width(),
                i.image.height()
            ));
        }
    }

    // Shelf pack, tallest-first, overflowing onto new pages.
    let mut order: Vec<usize> = (0..inputs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(inputs[i].image.height()));

    let mut placed: Vec<Placed> = Vec::new();
    let mut page_sizes: Vec<(u32, u32)> = Vec::new(); // (used_w, used_h) per page
    let (mut page, mut x, mut y, mut row_h) = (0usize, PAD, PAD, 0u32);
    let mut used_w = 0u32;

    let flush_page = |sizes: &mut Vec<(u32, u32)>, used_w: u32, y: u32, row_h: u32| {
        sizes.push((used_w.max(1), (y + row_h + PAD).max(1)));
    };

    for &i in &order {
        let (w, h) = (inputs[i].image.width(), inputs[i].image.height());
        if x + w + PAD > MAX && x > PAD {
            // next shelf
            y += row_h + PAD;
            x = PAD;
            row_h = 0;
        }
        if y + h + PAD > MAX {
            // next page
            flush_page(&mut page_sizes, used_w, y - row_h - PAD, row_h);
            page += 1;
            x = PAD;
            y = PAD;
            row_h = 0;
            used_w = 0;
        }
        placed.push(Placed { input_idx: i, page, x, y });
        x += w + PAD;
        used_w = used_w.max(x);
        row_h = row_h.max(h);
    }
    flush_page(&mut page_sizes, used_w, y, row_h);

    // Compose + encode pages.
    let mut pages: Vec<Page> = Vec::new();
    for (pi, &(pw, ph)) in page_sizes.iter().enumerate() {
        let mut canvas = RgbaImage::new(pw.min(MAX), ph.min(MAX));
        for pl in placed.iter().filter(|p| p.page == pi) {
            image::imageops::overlay(
                &mut canvas,
                &inputs[pl.input_idx].image,
                pl.x as i64,
                pl.y as i64,
            );
        }
        let name = if pi == 0 {
            format!("{base_name}.webp")
        } else {
            format!("{base_name}_{}.webp", pi + 1)
        };
        let dynimg = DynamicImage::ImageRgba8(canvas);
        let webp = webp::Encoder::from_image(&dynimg)
            .map_err(|e| format!("webp encode: {e}"))?
            .encode(92.0)
            .to_vec();
        let mut pngbuf = std::io::Cursor::new(Vec::new());
        dynimg
            .write_to(&mut pngbuf, ImageFormat::Png)
            .map_err(|e| format!("png encode: {e}"))?;
        pages.push(Page {
            name,
            webp,
            png: pngbuf.into_inner(),
            width: dynimg.width(),
            height: dynimg.height(),
        });
    }

    // Atlas text: one block per page, regions sorted by name within their page.
    let mut sorted = placed;
    sorted.sort_by(|a, b| {
        (a.page, &inputs[a.input_idx].name).cmp(&(b.page, &inputs[b.input_idx].name))
    });
    let mut s = String::new();
    for (pi, pg) in pages.iter().enumerate() {
        if pi > 0 {
            s.push('\n');
        }
        s.push_str(&format!("{}\n", pg.name));
        s.push_str(&format!("size: {},{}\n", pg.width, pg.height));
        s.push_str("format: RGBA8888\n");
        s.push_str("filter: Linear,Linear\n");
        s.push_str("repeat: none\n");
        for pl in sorted.iter().filter(|p| p.page == pi) {
            let inp = &inputs[pl.input_idx];
            s.push_str(&format!("{}\n", inp.name));
            s.push_str("  rotate: false\n");
            s.push_str(&format!("  xy: {}, {}\n", pl.x, pl.y));
            s.push_str(&format!("  size: {}, {}\n", inp.image.width(), inp.image.height()));
            s.push_str(&format!("  orig: {}, {}\n", inp.image.width(), inp.image.height()));
            s.push_str("  offset: 0, 0\n");
            s.push_str("  index: -1\n");
        }
    }

    Ok(PackResult { atlas_text: s, pages })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]))
    }

    #[test]
    fn packs_single_page_with_regions() {
        let inputs = vec![
            PackInput { name: "head".into(), image: img(100, 200) },
            PackInput { name: "torso".into(), image: img(300, 400) },
        ];
        let r = pack(&inputs, "symbol_h1").unwrap();
        assert_eq!(r.pages.len(), 1);
        assert!(r.atlas_text.starts_with("symbol_h1.webp\n"));
        assert!(r.atlas_text.contains("head\n"));
        assert!(r.atlas_text.contains("torso\n"));
        assert!(r.atlas_text.contains("size: 300, 400"));
        // Page dims cover the packed content.
        assert!(r.pages[0].width >= 300 && r.pages[0].height >= 400);
    }

    #[test]
    fn overflows_to_second_page() {
        // Three 1200-tall images can't share one 2048 page vertically once widths force
        // shelf wraps: use full-width shelves to force pagination.
        let inputs = vec![
            PackInput { name: "a".into(), image: img(2000, 1200) },
            PackInput { name: "b".into(), image: img(2000, 1200) },
            PackInput { name: "c".into(), image: img(2000, 500) },
        ];
        let r = pack(&inputs, "big").unwrap();
        assert!(r.pages.len() >= 2, "expected pagination, got {} page(s)", r.pages.len());
        assert!(r.atlas_text.contains("big.webp"));
        assert!(r.atlas_text.contains("big_2.webp"));
    }

    #[test]
    fn oversized_region_errors() {
        let inputs = vec![PackInput { name: "x".into(), image: img(2100, 100) }];
        assert!(pack(&inputs, "k").is_err());
    }
}
