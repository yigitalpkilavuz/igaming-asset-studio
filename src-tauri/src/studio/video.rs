//! Video → transparent FX spritesheet. A short clip (imported, or from a bring-your-own local
//! endpoint — never a hosted gambling-restricted API) is decomposed to frames with ffmpeg, each
//! frame matted to alpha by our OWN Rust cutters (magenta chroma-key or glow-on-black luminance —
//! no third-party matting model / license), a seamless loop is selected, and the frames are
//! composed into the SAME horizontal filmstrip the SpriteCook lane writes (`ai_sheet/sheet.png` +
//! `sheet.json`). The video is scaffolding — only the baked sprite ships.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{imageops, RgbaImage};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::processing::chromakey;
use crate::studio::fx;

/// How the incoming video carries its subject over the background — picks the matte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum VideoBg {
    /// Solid magenta background → chroma-key. Cleanest; steer the generator to a flat magenta.
    Magenta,
    /// Light / glow on pure black → luminance-to-alpha (flames, sparks, auras, coin shine).
    Glow,
}

/// How to make the clip loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum VideoLoop {
    /// Play forward then back (0..N-1..1) — ALWAYS seamless; best for ambient wobble/shimmer.
    PingPong,
    /// Take the subrange between the most-similar frame pair — best for genuinely cyclic motion.
    Seam,
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("ffmpeg failed to launch: {e} — is ffmpeg installed?"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(4).collect::<Vec<_>>().join(" ").chars().take(400).collect();
        return Err(format!("ffmpeg error: {tail}"));
    }
    Ok(())
}

/// Extract frames from `video` into `dir` at `fps`, returning sorted PNG paths.
fn extract_frames(video: &Path, dir: &Path, fps: f64) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create frames dir: {e}"))?;
    run(Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(video)
        .args(["-vf", &format!("fps={fps}"), "-vsync", "0"])
        .arg(dir.join("f_%04d.png")))?;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read frames: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err("no frames extracted — is the file a video?".into());
    }
    Ok(paths)
}

/// Matte one frame's PNG bytes → straight-alpha RGBA (reuses the existing Rust cutters).
fn matte(bytes: &[u8], bg: VideoBg) -> Result<RgbaImage, String> {
    match bg {
        VideoBg::Magenta => {
            let png = chromakey::chroma_key(bytes, (255, 0, 255), 0.28)?;
            Ok(image::load_from_memory(&png).map_err(|e| format!("decode matte: {e}"))?.to_rgba8())
        }
        VideoBg::Glow => {
            let img = image::load_from_memory(bytes).map_err(|e| format!("decode frame: {e}"))?.to_rgba8();
            Ok(fx::luminance_to_alpha(&img))
        }
    }
}

/// Perceptual distance between two frames (mean-squared over a 24×24 RGBA thumbnail). 0 = identical.
fn frame_distance(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let ta = imageops::resize(a, 24, 24, imageops::FilterType::Triangle);
    let tb = imageops::resize(b, 24, 24, imageops::FilterType::Triangle);
    let mut sum = 0.0;
    for (pa, pb) in ta.pixels().zip(tb.pixels()) {
        for c in 0..4 {
            let d = pa.0[c] as f64 - pb.0[c] as f64;
            sum += d * d;
        }
    }
    sum / (24.0 * 24.0 * 4.0)
}

/// Order the matted frames into a seamless loop.
fn make_loop(frames: Vec<RgbaImage>, mode: VideoLoop) -> Vec<RgbaImage> {
    let n = frames.len();
    if n < 3 {
        return frames;
    }
    match mode {
        // 0,1,…,N-1,N-2,…,1 — endpoints not repeated, so the wrap is seamless by construction.
        VideoLoop::PingPong => {
            let mut out = frames.clone();
            for i in (1..n - 1).rev() {
                out.push(frames[i].clone());
            }
            out
        }
        // Take [i, j) where frames[i]≈frames[j] and the span is at least a third of the clip.
        VideoLoop::Seam => {
            let min_len = (n / 3).max(2);
            let (mut bi, mut bj, mut best) = (0usize, n, f64::MAX);
            for i in 0..n.saturating_sub(min_len) {
                for j in (i + min_len)..n {
                    let d = frame_distance(&frames[i], &frames[j]);
                    if d < best {
                        best = d;
                        bi = i;
                        bj = j;
                    }
                }
            }
            frames[bi..bj].to_vec()
        }
    }
}

/// Resample a frame list to exactly `want` frames (even nearest-index sampling).
fn resample(frames: &[RgbaImage], want: usize) -> Vec<RgbaImage> {
    if frames.is_empty() || want == 0 {
        return Vec::new();
    }
    (0..want)
        .map(|i| frames[(i * frames.len() / want).min(frames.len() - 1)].clone())
        .collect()
}

/// Compose same-size frames into one horizontal filmstrip (the fx-sheet layout the export slices).
fn compose_strip(frames: &[RgbaImage]) -> Result<RgbaImage, String> {
    let (fw, fh) = frames.first().ok_or("no frames to compose")?.dimensions();
    let mut strip = RgbaImage::new(fw * frames.len() as u32, fh);
    for (i, f) in frames.iter().enumerate() {
        imageops::overlay(&mut strip, f, (i as u32 * fw) as i64, 0);
    }
    Ok(strip)
}

/// Full bake: `video` → `(strip_png_bytes, frame_count)`. `want` is clamped to 2–24 (even), the
/// same range the SpriteCook sheet uses. `work` is a scratch dir (caller cleans it).
pub fn bake(
    video: &Path,
    bg: VideoBg,
    want: u32,
    loop_mode: VideoLoop,
    work: &Path,
) -> Result<(Vec<u8>, u32), String> {
    let want = ((want.clamp(2, 24) / 2) * 2).max(2) as usize;

    let fdir = work.join("frames");
    let paths = extract_frames(video, &fdir, 12.0)?;
    // Bound the matte cost: keep ~3× the target for loop material, capped.
    let material = (want * 3).clamp(want, 72);
    let picks: Vec<&PathBuf> = if paths.len() > material {
        (0..material).map(|i| &paths[i * paths.len() / material]).collect()
    } else {
        paths.iter().collect()
    };

    let mut mats: Vec<RgbaImage> = Vec::with_capacity(picks.len());
    for p in picks {
        let bytes = std::fs::read(p).map_err(|e| format!("read frame: {e}"))?;
        mats.push(matte(&bytes, bg)?);
    }

    let looped = make_loop(mats, loop_mode);
    let sel = resample(&looped, want);
    let strip = compose_strip(&sel)?;

    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(strip)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode strip: {e}"))?;
    let _ = std::fs::remove_dir_all(&fdir);
    Ok((png.into_inner(), want as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn frame(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    #[test]
    fn ping_pong_is_seamless_and_endpoints_unrepeated() {
        let fs: Vec<RgbaImage> = (0..4).map(|i| frame(4, 4, [i as u8 * 10, 0, 0, 255])).collect();
        let looped = make_loop(fs.clone(), VideoLoop::PingPong);
        // 0,1,2,3,2,1 — length 2N-2, first != last frame's neighbour repeat, wraps cleanly.
        assert_eq!(looped.len(), 6);
        assert_eq!(looped[0], fs[0]);
        assert_eq!(looped[3], fs[3]);
        assert_eq!(looped[5], fs[1]); // …then back to 1, so looping to 0 is one clean step
    }

    #[test]
    fn seam_finds_the_cycle_and_trims_the_repeat() {
        // A there-and-back where frame 6 repeats frame 0 (red) — the natural wrap point — followed
        // by an outro. Seam should lock the [0,6) cycle: since frame[0]≈frame[6], playing 0..5 then
        // looping to 0 continues the motion smoothly (0 is what frame 5 flowed toward). It should
        // drop the redundant repeat frame and the outro.
        let cols = [
            [200, 0, 0, 255],   // 0 red      ← wrap target
            [200, 120, 0, 255], // 1 orange
            [0, 200, 0, 255],   // 2 green
            [0, 0, 200, 255],   // 3 blue
            [0, 200, 0, 255],   // 4 green
            [200, 120, 0, 255], // 5 orange
            [200, 0, 0, 255],   // 6 red ≈ 0  ← redundant repeat (the seam)
            [120, 0, 200, 255], // 7 purple    (outro)
        ];
        let fs: Vec<RgbaImage> = cols.iter().map(|c| frame(8, 8, *c)).collect();
        let looped = make_loop(fs, VideoLoop::Seam);
        assert_eq!(looped.len(), 6, "locks the [0,6) cycle, dropping the repeat + outro");
    }

    #[test]
    fn resample_hits_exact_count_and_composes_a_strip() {
        let fs: Vec<RgbaImage> = (0..5).map(|i| frame(6, 10, [i as u8 * 40, 0, 0, 255])).collect();
        let sel = resample(&fs, 8);
        assert_eq!(sel.len(), 8);
        let strip = compose_strip(&sel).unwrap();
        assert_eq!(strip.dimensions(), (6 * 8, 10)); // one row, width divisible by frame count
    }

    #[test]
    #[ignore = "shells ffmpeg end-to-end; run with `cargo test -- --ignored`"]
    fn bake_end_to_end_from_a_synthesized_clip() {
        // Synthesize a 1s magenta clip with a white box sweeping across (subject motion on a keyable
        // background), then bake it through the real extract→matte→loop→compose pipeline.
        let tmp = std::env::temp_dir().join("asset_pipeline_video_bake_e2e");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let clip = tmp.join("clip.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-y", "-loglevel", "error",
                "-f", "lavfi", "-i", "color=c=magenta:s=96x96:d=1:r=24",
                "-f", "lavfi", "-i", "color=c=white:s=24x24:d=1:r=24",
                "-filter_complex", "[0][1]overlay=x='(W-w)*t':y=36",
                "-pix_fmt", "yuv420p",
            ])
            .arg(&clip)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            eprintln!("ffmpeg unavailable — skipping e2e bake");
            return;
        }

        let (png, n) = bake(&clip, VideoBg::Magenta, 8, VideoLoop::PingPong, &tmp.join("work"))
            .expect("bake should succeed");
        assert_eq!(n, 8, "resampled to the requested even frame count");
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.width() % 8, 0, "strip width divisible by the frame count");
        assert!(img.pixels().any(|p| p.0[3] == 0), "magenta bg keyed to transparent");
        assert!(img.pixels().any(|p| p.0[3] > 200), "the white subject survives the matte");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn magenta_matte_keys_out_the_background() {
        // A magenta field with an opaque green square in the middle → the square survives, bg clears.
        let mut img = RgbaImage::from_pixel(16, 16, Rgba([255, 0, 255, 255]));
        for y in 6..10 {
            for x in 6..10 {
                img.put_pixel(x, y, Rgba([0, 200, 0, 255]));
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let out = matte(&png.into_inner(), VideoBg::Magenta).unwrap();
        assert_eq!(out.get_pixel(0, 0).0[3], 0, "magenta bg is keyed transparent");
        assert!(out.get_pixel(7, 7).0[3] > 200, "the subject stays opaque");
    }
}
