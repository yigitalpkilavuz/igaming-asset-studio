//! Depth-based layer proposals: Depth Anything V2 (small) estimates a relative depth map
//! of the approved scene, which is sliced into N contiguous depth bands — selections come
//! from actual estimated depth instead of object segmentation (SAM segments *objects*;
//! "depth band" is not a concept it has, which is why point-prompting misfires on scenery).
//!
//! Model recipe mirrors candle's own depth_anything_v2 example: DINOv2-S backbone +
//! DPT depth head, 518×518 input, ImageNet normalization, bilinear upsample back to
//! source dims, min-max scaled. In the output, LARGER values are NEARER.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::depth_anything_v2::{DepthAnythingV2, DepthAnythingV2Config};
use candle_transformers::models::dinov2;
use futures::StreamExt;
use image::{GrayImage, RgbImage};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Emitter;

use crate::studio::matte;

const INPUT_SIZE: usize = 518;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// The two weight files the model needs, downloaded into `<config>/models/depth/`.
const FILES: [(&str, &str); 2] = [
    (
        "dinov2_vits14.safetensors",
        "https://huggingface.co/lmz/candle-dino-v2/resolve/main/dinov2_vits14.safetensors",
    ),
    (
        "depth_anything_v2_vits.safetensors",
        "https://huggingface.co/jeroenvlek/depth-anything-v2-safetensors/resolve/main/depth_anything_v2_vits.safetensors",
    ),
];

pub const PROGRESS_EVENT: &str = "layers://depth-progress";

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum DepthStatus {
    Missing,
    Ready,
}

fn depth_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("models").join("depth")
}

pub fn weights_ready(config_dir: &Path) -> bool {
    FILES.iter().all(|(f, _)| depth_dir(config_dir).join(f).is_file())
}

#[derive(Clone, Serialize)]
struct Progress {
    received: f64,
    total: f64,
}

/// Download both weight files (streamed, atomic rename). No pinned checksums — the model
/// load validates the safetensors; a corrupt file errors there and can be re-downloaded.
pub async fn download(app: &tauri::AppHandle, config_dir: &Path) -> Result<(), String> {
    let dir = depth_dir(config_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create models dir failed: {e}"))?;
    for (file, url) in FILES {
        let path = dir.join(file);
        if path.is_file() {
            continue;
        }
        let resp = reqwest::get(url).await.map_err(|e| format!("download failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("download failed: HTTP {}", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0) as f64;
        let tmp = path.with_extension("part");
        {
            use std::io::Write;
            let mut out = std::fs::File::create(&tmp)
                .map_err(|e| format!("create weights file failed: {e}"))?;
            let mut received: u64 = 0;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("download stream failed: {e}"))?;
                out.write_all(&chunk).map_err(|e| format!("write weights failed: {e}"))?;
                received += chunk.len() as u64;
                let _ =
                    app.emit(PROGRESS_EVENT, Progress { received: received as f64, total });
            }
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("finalize weights failed: {e}"))?;
    }
    Ok(())
}

// ── Engine ─────────────────────────────────────────────────────────────────────

struct DepthEngine {
    model: DepthAnythingV2,
    device: Device,
}

fn preferred_device() -> Device {
    #[cfg(target_os = "macos")]
    {
        if let Ok(d) = Device::new_metal(0) {
            return d;
        }
    }
    Device::Cpu
}

fn load_engine(config_dir: &Path, device: Device) -> Result<DepthEngine, String> {
    let dir = depth_dir(config_dir);
    let dino_vb = unsafe {
        VarBuilder::from_mmaped_safetensors(
            &[dir.join(FILES[0].0)],
            DType::F32,
            &device,
        )
        .map_err(|e| format!("load dinov2 weights failed: {e}"))?
    };
    let dino = dinov2::vit_small(dino_vb).map_err(|e| format!("build dinov2 failed: {e}"))?;
    let da_vb = unsafe {
        VarBuilder::from_mmaped_safetensors(
            &[dir.join(FILES[1].0)],
            DType::F32,
            &device,
        )
        .map_err(|e| format!("load depth weights failed: {e}"))?
    };
    let model = DepthAnythingV2::new(Arc::new(dino), DepthAnythingV2Config::vit_small(), da_vb)
        .map_err(|e| format!("build depth model failed: {e}"))?;
    Ok(DepthEngine { model, device })
}

/// Load-per-call (the model holds non-`Send` modules, so it can't be cached in a global;
/// proposing layers is a once-per-background action, so the ~1 s load is acceptable).
fn with_engine<T>(
    config_dir: &Path,
    f: impl FnOnce(&DepthEngine) -> Result<T, String>,
) -> Result<T, String> {
    let device = preferred_device();
    let engine = match load_engine(config_dir, device.clone()) {
        Ok(e) => e,
        Err(err) if !matches!(device, Device::Cpu) => {
            eprintln!("depth: metal load failed ({err}), falling back to CPU");
            load_engine(config_dir, Device::Cpu)?
        }
        Err(err) => return Err(err),
    };
    f(&engine)
}

/// Estimate relative depth for `rgb`, returned row-major at source dims, min-max scaled
/// to 0..1 where LARGER = NEARER.
pub fn estimate(config_dir: &Path, rgb: &RgbImage) -> Result<Vec<f32>, String> {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    with_engine(config_dir, |engine| {
        // Preprocess: resize 518×518, /255, ImageNet-normalize, NCHW f32.
        let resized = image::imageops::resize(
            rgb,
            INPUT_SIZE as u32,
            INPUT_SIZE as u32,
            image::imageops::FilterType::Triangle,
        );
        let mut data = vec![0f32; 3 * INPUT_SIZE * INPUT_SIZE];
        for (x, y, p) in resized.enumerate_pixels() {
            let i = y as usize * INPUT_SIZE + x as usize;
            for c in 0..3 {
                data[c * INPUT_SIZE * INPUT_SIZE + i] =
                    (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let input = Tensor::from_vec(data, (1, 3, INPUT_SIZE, INPUT_SIZE), &engine.device)
            .map_err(|e| format!("build input tensor: {e}"))?;
        let depth = engine.model.forward(&input).map_err(|e| format!("depth forward: {e}"))?;
        let depth = depth
            .interpolate2d(h, w)
            .map_err(|e| format!("depth upsample: {e}"))?;
        let flat: Vec<f32> = depth
            .flatten_all()
            .and_then(|t| t.to_vec1())
            .map_err(|e| format!("read depth: {e}"))?;

        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for &v in &flat {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let range = (hi - lo).max(1e-6);
        Ok(flat.iter().map(|v| (v - lo) / range).collect())
    })
}

// ── Banding ────────────────────────────────────────────────────────────────────

/// Slice a depth map into `n` contiguous bands via 1-D k-means on the depth values,
/// returning one mask per band ordered FAR → NEAR. Each mask is edge-snapped onto the
/// image with the shared guided filter, then cleaned (open/close + capped hole fill).
pub fn band_masks(
    depth: &[f32],
    rgb: &RgbImage,
    n: usize,
) -> Result<Vec<GrayImage>, String> {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if depth.len() != w * h {
        return Err("depth map dims mismatch".into());
    }
    let n = n.clamp(2, 8);

    // 1-D k-means, centers seeded on evenly spaced quantiles.
    let mut sorted: Vec<f32> = depth.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut centers: Vec<f32> = (0..n)
        .map(|i| sorted[((i as f64 + 0.5) / n as f64 * (sorted.len() - 1) as f64) as usize])
        .collect();
    let mut assign = vec![0u8; depth.len()];
    for _ in 0..25 {
        for (i, &v) in depth.iter().enumerate() {
            let mut best = 0;
            let mut bd = f32::INFINITY;
            for (c, &ctr) in centers.iter().enumerate() {
                let d = (v - ctr).abs();
                if d < bd {
                    bd = d;
                    best = c;
                }
            }
            assign[i] = best as u8;
        }
        let mut sums = vec![0f64; n];
        let mut counts = vec![0u64; n];
        for (i, &a) in assign.iter().enumerate() {
            sums[a as usize] += depth[i] as f64;
            counts[a as usize] += 1;
        }
        for c in 0..n {
            if counts[c] > 0 {
                centers[c] = (sums[c] / counts[c] as f64) as f32;
            }
        }
    }
    // Band order: ascending centers = far → near (larger depth value = nearer).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| centers[a].total_cmp(&centers[b]));
    let rank_of: Vec<usize> = {
        let mut r = vec![0; n];
        for (rank, &c) in order.iter().enumerate() {
            r[c] = rank;
        }
        r
    };

    let hole_cap = ((w * h) / 2000).max(64);
    let mut masks = Vec::with_capacity(n);
    for band in 0..n {
        // Soft membership → guided edge snap (the 518-res depth boundary is blurry at
        // source scale; snap it onto the actual color edges).
        let soft: Vec<f32> = assign
            .iter()
            .map(|&a| if rank_of[a as usize] == band { 1.0 } else { 0.0 })
            .collect();
        let snapped = matte::guided_filter_rgb(&soft, rgb, 10, 1e-4);
        let mut bits: Vec<bool> = snapped.iter().map(|&v| v > 0.5).collect();

        // Clean: open (kill slivers), close (heal cracks), fill small holes.
        matte::erode(&mut bits, w, h, 2);
        matte::dilate(&mut bits, w, h, 2);
        matte::dilate(&mut bits, w, h, 2);
        matte::erode(&mut bits, w, h, 2);
        matte::fill_holes(&mut bits, w, h, Some(hole_cap));

        let mut mask = GrayImage::new(w as u32, h as u32);
        for (px, &b) in mask.pixels_mut().zip(bits.iter()) {
            px.0[0] = if b { 255 } else { 0 };
        }
        masks.push(mask);
    }
    Ok(masks)
}

/// Canonical layer names + parallax speeds for an `n`-band split, far → near.
pub fn band_plan(n: usize) -> Vec<(String, String, f64)> {
    let n = n.clamp(2, 8);
    let names: Vec<(&str, &str)> = match n {
        2 => vec![("sky", "Sky"), ("foreground", "Foreground")],
        3 => vec![("sky", "Sky"), ("mid", "Mid-ground"), ("foreground", "Foreground")],
        4 => vec![
            ("sky", "Sky"),
            ("far", "Far background"),
            ("mid", "Mid-ground"),
            ("foreground", "Foreground"),
        ],
        _ => {
            let mut v = vec![("sky", "Sky"), ("far", "Far background")];
            for _ in 0..n - 4 {
                v.push(("band", "Band"));
            }
            v.push(("mid", "Mid-ground"));
            v.push(("foreground", "Foreground"));
            v
        }
    };
    names
        .into_iter()
        .enumerate()
        .map(|(i, (id, name))| {
            let id = if id == "band" { format!("band_{}", i) } else { id.to_string() };
            let name = if name == "Band" { format!("Depth band {}", i) } else { name.to_string() };
            // Speeds: slow start, full speed up front (t^1.6 spacing reads naturally),
            // offset from a 0.05 floor so every band moves and speeds stay strictly rising.
            let t = i as f64 / (n - 1) as f64;
            let speed = 0.05 + t.powf(1.6) * 0.95;
            (id, name, speed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_partition_far_to_near_and_snap_to_edges() {
        // Synthetic scene: three vertical color strips; depth ramps with x (right = near),
        // but the depth "boundary" is offset a few px from the color edges — the snap
        // must pull band edges onto the color edges.
        let (w, h) = (90u32, 60u32);
        let rgb = RgbImage::from_fn(w, h, |x, _| {
            if x < 30 {
                image::Rgb([40, 60, 160])
            } else if x < 60 {
                image::Rgb([90, 140, 70])
            } else {
                image::Rgb([200, 170, 60])
            }
        });
        let depth: Vec<f32> = (0..w * h)
            .map(|i| {
                let x = i % w;
                if x < 27 {
                    0.1
                } else if x < 63 {
                    0.5
                } else {
                    0.9
                }
            })
            .collect();
        let masks = band_masks(&depth, &rgb, 3).unwrap();
        assert_eq!(masks.len(), 3);
        // Far band contains the left strip, near band the right strip.
        assert_eq!(masks[0].get_pixel(5, 30).0[0], 255);
        assert_eq!(masks[2].get_pixel(85, 30).0[0], 255);
        assert_eq!(masks[0].get_pixel(85, 30).0[0], 0);
        // Snap: the far/mid boundary sits at the color edge (x=30), not the depth
        // boundary (x=27).
        assert_eq!(masks[0].get_pixel(28, 30).0[0], 255, "snapped past depth seam");
        assert_eq!(masks[1].get_pixel(33, 30).0[0], 255);
    }

    #[test]
    fn band_plan_names_and_speeds_rise() {
        for n in 2..=8 {
            let plan = band_plan(n);
            assert_eq!(plan.len(), n);
            assert_eq!(plan[0].0, "sky");
            assert_eq!(plan[n - 1].0, "foreground");
            assert!(plan.windows(2).all(|w| w[0].2 < w[1].2), "speeds rise (n={n})");
            let ids: std::collections::HashSet<&str> =
                plan.iter().map(|p| p.0.as_str()).collect();
            assert_eq!(ids.len(), n, "unique ids (n={n})");
        }
    }
}
