//! candle MobileSAM engine. Loads `mobile_sam-tiny-vitt.safetensors` (TinyViT encoder +
//! SAM prompt-encoder/mask-decoder), preferring Metal on macOS with automatic CPU
//! fallback. The expensive image-encoder pass runs once per source image and is cached in
//! memory and on disk (`studio/sam/embedding.st`); each click prompt only runs the decoder.

use std::path::{Path, PathBuf};

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::segment_anything::sam::{self, Sam};
use image::{GrayImage, RgbImage};

use super::weights::SamModel;

/// A cached image embedding, tied to a source identity.
struct Embed {
    key: String,
    tensor: Tensor,
    /// Dims of the resized image the embedding was computed from (longest side 1024).
    rw: usize,
    rh: usize,
}

pub struct SamEngine {
    sam: Sam,
    device: Device,
    weights: PathBuf,
    model: SamModel,
    /// LRU of one — the studio edits one asset at a time.
    embed: Option<Embed>,
    /// Set after a Metal failure made us rebuild on CPU (don't retry Metal).
    cpu_forced: bool,
}

impl SamEngine {
    pub fn load(weights: &Path, model: SamModel) -> Result<Self, String> {
        let device = preferred_device();
        match Self::load_on(weights, model, device.clone()) {
            Ok(e) => Ok(e),
            Err(err) if !matches!(device, Device::Cpu) => {
                // Metal init can fail on odd setups — fall back silently.
                eprintln!("sam: metal load failed ({err}), falling back to CPU");
                Self::load_on(weights, model, Device::Cpu)
            }
            Err(err) => Err(err),
        }
    }

    pub fn model(&self) -> SamModel {
        self.model
    }

    fn load_on(weights: &Path, model: SamModel, device: Device) -> Result<Self, String> {
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights], DType::F32, &device)
                .map_err(|e| format!("load sam weights failed: {e}"))?
        };
        let sam = match model {
            SamModel::Tiny => Sam::new_tiny(vb),
            // Standard SAM ViT-B configuration.
            SamModel::Base => Sam::new(768, 12, 12, &[2, 5, 8, 11], vb),
        }
        .map_err(|e| format!("build sam model failed: {e}"))?;
        Ok(Self {
            sam,
            cpu_forced: matches!(device, Device::Cpu),
            device,
            weights: weights.to_path_buf(),
            model,
            embed: None,
        })
    }

    /// Segment with normalized point prompts `(x, y, positive)`. Returns a 0/255 mask at
    /// the RESIZED dims (longest side 1024); the caller scales back to source dims.
    pub fn segment(
        &mut self,
        key: &str,
        rgb: &RgbImage,
        prompts: &[(f64, f64, bool)],
        disk_cache: Option<&Path>,
    ) -> Result<GrayImage, String> {
        match self.segment_inner(key, rgb, prompts, disk_cache) {
            Ok(m) => Ok(m),
            Err(err) if !self.cpu_forced => {
                // A Metal op failed mid-forward — rebuild everything on CPU and retry once.
                eprintln!("sam: metal inference failed ({err}), retrying on CPU");
                *self = Self::load_on(&self.weights.clone(), self.model, Device::Cpu)?;
                self.segment_inner(key, rgb, prompts, disk_cache)
            }
            Err(err) => Err(err),
        }
    }

    /// All decoder candidates for a prompt set (SAM proposes 3 granularities), with the
    /// model's self-scored IoU and a logit-derived stability score (area ratio between
    /// tight and loose thresholds — ragged masks score low). Used by the region sweep,
    /// which needs every granularity, not just the best pick.
    pub fn segment_candidates(
        &mut self,
        key: &str,
        rgb: &RgbImage,
        prompts: &[(f64, f64, bool)],
        disk_cache: Option<&Path>,
    ) -> Result<Vec<(GrayImage, f32, f32)>, String> {
        match self.segment_candidates_inner(key, rgb, prompts, disk_cache) {
            Ok(v) => Ok(v),
            Err(err) if !self.cpu_forced => {
                eprintln!("sam: metal inference failed ({err}), retrying on CPU");
                *self = Self::load_on(&self.weights.clone(), self.model, Device::Cpu)?;
                self.segment_candidates_inner(key, rgb, prompts, disk_cache)
            }
            Err(err) => Err(err),
        }
    }

    fn segment_candidates_inner(
        &mut self,
        key: &str,
        rgb: &RgbImage,
        prompts: &[(f64, f64, bool)],
        disk_cache: Option<&Path>,
    ) -> Result<Vec<(GrayImage, f32, f32)>, String> {
        self.ensure_embedding(key, rgb, disk_cache)?;
        let embed = self.embed.as_ref().unwrap();
        let (rw, rh) = (embed.rw, embed.rh);

        let (low_res_mask, iou) = self
            .sam
            .forward_for_embeddings(&embed.tensor, rh, rw, prompts, true)
            .map_err(|e| format!("sam decode failed: {e}"))?;
        let scores = iou
            .flatten_all()
            .and_then(|t| t.to_dtype(DType::F32))
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|e| format!("sam iou read failed: {e}"))?;
        let n_masks = low_res_mask.dims().get(1).copied().unwrap_or(1);

        let mut out = Vec::with_capacity(n_masks);
        for mi in 0..n_masks {
            let logits = low_res_mask
                .narrow(1, mi, 1)
                .and_then(|m| m.upsample_nearest2d(sam::IMAGE_SIZE, sam::IMAGE_SIZE))
                .and_then(|m| m.get(0))
                .and_then(|m| m.i((.., ..rh, ..rw)))
                .and_then(|m| m.flatten_all())
                .and_then(|m| m.to_dtype(DType::F32))
                .and_then(|m| m.to_vec1::<f32>())
                .map_err(|e| format!("sam mask read failed: {e}"))?;
            if logits.len() != rw * rh {
                return Err(format!("sam mask size {} != {}", logits.len(), rw * rh));
            }
            let (mut tight, mut loose) = (0u64, 0u64);
            let mut mask = GrayImage::new(rw as u32, rh as u32);
            for (i, px) in mask.pixels_mut().enumerate() {
                let l = logits[i];
                if l >= 1.0 {
                    tight += 1;
                }
                if l >= -1.0 {
                    loose += 1;
                }
                px.0[0] = if l >= 0.0 { 255 } else { 0 };
            }
            let stability = if loose > 0 { tight as f32 / loose as f32 } else { 0.0 };
            out.push((mask, scores.get(mi).copied().unwrap_or(0.0), stability));
        }
        Ok(out)
    }

    fn segment_inner(
        &mut self,
        key: &str,
        rgb: &RgbImage,
        prompts: &[(f64, f64, bool)],
        disk_cache: Option<&Path>,
    ) -> Result<GrayImage, String> {
        self.ensure_embedding(key, rgb, disk_cache)?;
        let embed = self.embed.as_ref().unwrap();
        let (rw, rh) = (embed.rw, embed.rh);

        // Multi-mask: SAM proposes 3 candidate masks with self-scored IoU — take the best.
        // (Single-mask mode often returns a mediocre compromise for ambiguous clicks.)
        let (low_res_mask, iou) = self
            .sam
            .forward_for_embeddings(&embed.tensor, rh, rw, prompts, true)
            .map_err(|e| format!("sam decode failed: {e}"))?;
        let scores = iou
            .flatten_all()
            .and_then(|t| t.to_dtype(DType::F32))
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|e| format!("sam iou read failed: {e}"))?;
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        let n_masks = low_res_mask
            .dims()
            .get(1)
            .copied()
            .unwrap_or(1);
        let mask = low_res_mask
            .narrow(1, best.min(n_masks.saturating_sub(1)), 1)
            .and_then(|m| m.upsample_nearest2d(sam::IMAGE_SIZE, sam::IMAGE_SIZE))
            .and_then(|m| m.get(0))
            .and_then(|m| m.i((.., ..rh, ..rw)))
            .map_err(|e| format!("sam mask reshape failed: {e}"))?;
        let mask = mask
            .flatten_all()
            .and_then(|m| m.to_dtype(DType::F32))
            .and_then(|m| m.to_vec1::<f32>())
            .map_err(|e| format!("sam mask read failed: {e}"))?;
        if mask.len() != rw * rh {
            return Err(format!("sam mask size {} != {}", mask.len(), rw * rh));
        }
        let mut out = GrayImage::new(rw as u32, rh as u32);
        for (i, px) in out.pixels_mut().enumerate() {
            px.0[0] = if mask[i] >= 0.0 { 255 } else { 0 };
        }
        Ok(out)
    }

    /// Compute (or fetch) the encoder embedding for this source. Order: memory → disk → run.
    fn ensure_embedding(
        &mut self,
        key: &str,
        rgb: &RgbImage,
        disk_cache: Option<&Path>,
    ) -> Result<(), String> {
        // Embeddings are ENCODER-SPECIFIC: tag the cache key with the model.
        let key = format!("{}:{}", self.model.tag(), key);
        let key = key.as_str();
        if self.embed.as_ref().is_some_and(|e| e.key == key) {
            return Ok(());
        }
        let (rw, rh) = resized_dims(rgb.width(), rgb.height());

        // Disk hit?
        if let Some(dir) = disk_cache {
            if let Some(t) = load_disk_embedding(dir, key, &self.device) {
                self.embed = Some(Embed { key: key.into(), tensor: t, rw, rh });
                return Ok(());
            }
        }

        let img = image_tensor(rgb, rw, rh, &self.device)?;
        let tensor = self
            .sam
            .embeddings(&img)
            .map_err(|e| format!("sam encode failed: {e}"))?;
        if let Some(dir) = disk_cache {
            save_disk_embedding(dir, key, &tensor);
        }
        self.embed = Some(Embed { key: key.into(), tensor, rw, rh });
        Ok(())
    }
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

/// Longest side scaled to SAM's 1024 input.
fn resized_dims(w: u32, h: u32) -> (usize, usize) {
    let scale = sam::IMAGE_SIZE as f64 / w.max(h).max(1) as f64;
    (
        ((w as f64 * scale).round() as usize).clamp(1, sam::IMAGE_SIZE),
        ((h as f64 * scale).round() as usize).clamp(1, sam::IMAGE_SIZE),
    )
}

/// Channel-first f32 tensor (values 0–255; `Sam::preprocess` normalizes internally).
fn image_tensor(rgb: &RgbImage, rw: usize, rh: usize, device: &Device) -> Result<Tensor, String> {
    let resized = image::imageops::resize(
        rgb,
        rw as u32,
        rh as u32,
        image::imageops::FilterType::CatmullRom,
    );
    let mut data = vec![0f32; 3 * rw * rh];
    for (x, y, p) in resized.enumerate_pixels() {
        let i = y as usize * rw + x as usize;
        data[i] = p.0[0] as f32;
        data[rw * rh + i] = p.0[1] as f32;
        data[2 * rw * rh + i] = p.0[2] as f32;
    }
    Tensor::from_vec(data, (3, rh, rw), device).map_err(|e| format!("image tensor failed: {e}"))
}

fn embedding_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("embedding.st"), dir.join("embedding.json"))
}

fn load_disk_embedding(dir: &Path, key: &str, device: &Device) -> Option<Tensor> {
    let (st, meta) = embedding_paths(dir);
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
    if meta.get("key")?.as_str()? != key {
        return None;
    }
    let tensors = candle_core::safetensors::load(&st, device).ok()?;
    tensors.get("embedding").cloned()
}

fn save_disk_embedding(dir: &Path, key: &str, tensor: &Tensor) {
    let (st, meta) = embedding_paths(dir);
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Best-effort cache — failures just mean a re-encode next session.
    let _ = tensor.save_safetensors("embedding", &st);
    let _ = std::fs::write(meta, serde_json::json!({ "key": key }).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resized_dims_scales_longest_to_1024() {
        assert_eq!(resized_dims(2048, 1024), (1024, 512));
        assert_eq!(resized_dims(500, 1000), (512, 1024));
        assert_eq!(resized_dims(1024, 1024), (1024, 1024));
    }

    /// The SPIKE: real MobileSAM inference on a synthetic image. Gated on the weights path
    /// so CI without the 40 MB download skips it.
    /// Run: `WF_SAM_WEIGHTS=/path/to/mobile_sam-tiny-vitt.safetensors cargo test sam_spike -- --nocapture`
    #[test]
    fn sam_spike_segments_a_disc() {
        let Some(weights) = std::env::var_os("WF_SAM_WEIGHTS") else {
            return;
        };
        let weights = PathBuf::from(weights);

        // A dark disc on a light background, 800×600.
        let (w, h) = (800u32, 600u32);
        let (cx, cy, r) = (400f32, 300f32, 150f32);
        let img = RgbImage::from_fn(w, h, |x, y| {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < r {
                image::Rgb([40, 40, 60])
            } else {
                image::Rgb([220, 220, 215])
            }
        });

        let model = match std::env::var("WF_SAM_MODEL").as_deref() {
            Ok("base") => SamModel::Base,
            _ => SamModel::Tiny,
        };
        let t0 = std::time::Instant::now();
        let mut engine = SamEngine::load(&weights, model).expect("load engine");
        eprintln!("sam load: {:?} on {:?}", t0.elapsed(), engine.device);

        let t1 = std::time::Instant::now();
        let mask = engine
            .segment("spike", &img, &[(0.5, 0.5, true)], None)
            .expect("segment");
        eprintln!("sam encode+decode: {:?}", t1.elapsed());

        // Decoder-only second call (embedding cached).
        let t2 = std::time::Instant::now();
        let mask2 = engine
            .segment("spike", &img, &[(0.5, 0.5, true), (0.05, 0.05, false)], None)
            .expect("segment 2");
        eprintln!("sam decode-only: {:?}", t2.elapsed());

        // The mask is at resized dims (1024×768). Check disc coverage: centre in, corner out.
        let (mw, mh) = mask.dimensions();
        assert_eq!((mw, mh), (1024, 768));
        assert_eq!(mask.get_pixel(mw / 2, mh / 2).0[0], 255, "disc centre inside mask");
        assert_eq!(mask.get_pixel(10, 10).0[0], 0, "corner outside mask");
        // IoU-ish sanity: mask area within 2x of the disc area (scaled).
        let scale = 1024.0 / 800.0;
        let disc_area = std::f32::consts::PI * (r * scale).powi(2);
        let mask_area = mask.pixels().filter(|p| p.0[0] > 0).count() as f32;
        assert!(
            mask_area > disc_area * 0.5 && mask_area < disc_area * 2.0,
            "mask area {mask_area} vs disc {disc_area}"
        );
        let _ = mask2;
    }
}
