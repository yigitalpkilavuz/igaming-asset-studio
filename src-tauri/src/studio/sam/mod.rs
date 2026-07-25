//! Local Segment-Anything (MobileSAM) for interactive part cuts. The image encoder runs
//! once per source (cached in memory and on disk); the mask decoder answers each click
//! prompt in tens of milliseconds, so correction feels live.

pub mod engine;
pub mod weights;

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use specta::Type;

use engine::SamEngine;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum SamStatus {
    /// Weights not downloaded yet; segmentation falls back to manual masks.
    Missing,
    /// `model` is "tiny" (MobileSAM) or "vit_b" (full SAM — better masks).
    Ready { path: String, model: String },
}

/// The process-wide engine. Candle is synchronous — callers go through
/// `spawn_blocking` and this mutex serializes GPU/CPU work.
static ENGINE: OnceLock<Mutex<Option<SamEngine>>> = OnceLock::new();

pub fn engine() -> &'static Mutex<Option<SamEngine>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// Get or lazily load the engine for `model` (reloads if a better model was installed).
pub fn with_engine<T>(
    weights: &std::path::Path,
    model: weights::SamModel,
    f: impl FnOnce(&mut SamEngine) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = engine().lock().map_err(|_| "sam engine poisoned".to_string())?;
    if guard.as_ref().is_none_or(|e| e.model() != model) {
        *guard = Some(SamEngine::load(weights, model)?);
    }
    f(guard.as_mut().unwrap())
}
