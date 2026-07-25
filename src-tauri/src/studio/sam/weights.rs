//! MobileSAM weights acquisition: download-on-first-use into the app's machine-scoped
//! data dir (`<app_data>/models/sam/`), streamed with progress events, written atomically
//! (`.part` + rename) and verified against a pinned sha256.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use serde::Serialize;
use tauri::Emitter;

/// The two supported models. `Base` (SAM ViT-B) segments stylised game art noticeably
/// better than the distilled `Tiny`; the decoder is identical, so clicks stay fast — only
/// the once-per-asset (disk-cached) image encode is slower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamModel {
    Tiny,
    Base,
}

impl SamModel {
    pub fn file(self) -> &'static str {
        match self {
            SamModel::Tiny => "mobile_sam-tiny-vitt.safetensors",
            SamModel::Base => "sam_vit_b_01ec64.safetensors",
        }
    }
    pub fn url(self) -> &'static str {
        match self {
            SamModel::Tiny => {
                "https://huggingface.co/lmz/candle-sam/resolve/main/mobile_sam-tiny-vitt.safetensors"
            }
            SamModel::Base => {
                "https://huggingface.co/lmz/candle-sam/resolve/main/sam_vit_b_01ec64.safetensors"
            }
        }
    }
    pub fn sha256(self) -> &'static str {
        match self {
            SamModel::Tiny => "5ad43a841b57cc798e2837e251c15aa9f602868a1780d195459383d117784cf7",
            SamModel::Base => "65d59ca840f1ea3b60d6b1efe298b125948cbd0a4ea7b9f08eba2201f70cd865",
        }
    }
    /// Cache tag: embeddings from different encoders are incompatible.
    pub fn tag(self) -> &'static str {
        match self {
            SamModel::Tiny => "tiny",
            SamModel::Base => "vit_b",
        }
    }
}

/// Event emitted during download: `{ received, total }` in bytes (total 0 if unknown).
pub const PROGRESS_EVENT: &str = "studio://sam-progress";

#[derive(Clone, Serialize)]
struct Progress {
    received: f64,
    total: f64,
}

/// Where a model's weights live (machine-scoped, NOT inside any project).
pub fn weights_path(config_dir: &Path, model: SamModel) -> PathBuf {
    config_dir.join("models").join("sam").join(model.file())
}

/// The best model on disk (`Base` preferred), if any.
pub fn best_available(config_dir: &Path) -> Option<SamModel> {
    if weights_path(config_dir, SamModel::Base).is_file() {
        Some(SamModel::Base)
    } else if weights_path(config_dir, SamModel::Tiny).is_file() {
        Some(SamModel::Tiny)
    } else {
        None
    }
}

/// Download + verify a model's weights. Emits progress events on `app`.
pub async fn download(
    app: &tauri::AppHandle,
    config_dir: &Path,
    model: SamModel,
) -> Result<PathBuf, String> {
    let path = weights_path(config_dir, model);
    if path.is_file() {
        return Ok(path);
    }
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("create models dir failed: {e}"))?;

    let resp = reqwest::get(model.url())
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0) as f64;

    let tmp = path.with_extension("safetensors.part");
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut received: u64 = 0;
    {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&tmp).map_err(|e| format!("create weights file failed: {e}"))?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download stream failed: {e}"))?;
            file.write_all(&chunk).map_err(|e| format!("write weights failed: {e}"))?;
            sha2::Digest::update(&mut hasher, &chunk);
            received += chunk.len() as u64;
            let _ = app.emit(PROGRESS_EVENT, Progress { received: received as f64, total });
        }
    }

    let digest = format!("{:x}", sha2::Digest::finalize(hasher));
    if digest != model.sha256() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "weights checksum mismatch (got {digest}); download corrupted, try again"
        ));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("finalize weights failed: {e}"))?;
    Ok(path)
}
