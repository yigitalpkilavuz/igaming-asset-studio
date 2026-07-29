//! Pluggable AUDIO providers (parallel to the image `ImageProvider` layer). Adding one =
//! implement `AudioProvider` + register it in `build_audio_provider`. Only gambling-safe
//! providers are shipped here — Stake is a real-money platform, so ElevenLabs / MusicGen /
//! Udio are deliberately excluded (their licenses prohibit real-money-gambling use or
//! aren't commercial). Current set: Stability "Stable Audio" and Google "Lyria".

pub mod lyria;
pub mod stable_audio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::game_config::AudioKind;
use crate::settings::AppSettings;

/// A request to generate one audio cue.
#[derive(Debug, Clone)]
pub struct AudioRequest {
    pub prompt: String,
    pub kind: AudioKind,
    /// The cue loops seamlessly (drives a "seamless loop" prompt hint + the loop master later).
    pub looped: bool,
    /// Target duration in seconds (providers clamp to their own limits).
    pub target_secs: f32,
    pub seed: Option<u64>,
}

/// A generation result: encoded bytes + their container format + duration.
pub struct AudioOutput {
    pub bytes: Vec<u8>,
    /// File extension for the raw bytes, e.g. `"wav"` | `"mp3"`.
    pub format: String,
    pub duration_secs: f32,
}

#[async_trait]
pub trait AudioProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Whether this provider can make music beds.
    fn does_music(&self) -> bool {
        true
    }
    /// Whether this provider can make SFX one-shots.
    fn does_sfx(&self) -> bool {
        true
    }
    /// Ready to use (key/token present + any required config set).
    fn configured(&self) -> bool;
    async fn generate(&self, req: &AudioRequest) -> Result<AudioOutput, String>;
}

/// What an audio provider supports, surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioProviderInfo {
    pub id: String,
    pub display_name: String,
    pub does_music: bool,
    pub does_sfx: bool,
    pub configured: bool,
}

pub const STABLE_AUDIO: &str = "stable_audio";
pub const LYRIA: &str = "lyria";

/// All audio provider ids, in display order.
pub const AUDIO_PROVIDER_IDS: &[&str] = &[STABLE_AUDIO, LYRIA];

/// Construct an audio provider by id from current settings + secrets.
pub fn build_audio_provider(
    id: &str,
    settings: &AppSettings,
) -> Result<Box<dyn AudioProvider>, String> {
    match id {
        STABLE_AUDIO => Ok(Box::new(stable_audio::StableAudio::new(
            crate::secrets::get(crate::secrets::STABILITY_KEY)?,
            settings.stable_audio_model.clone(),
        ))),
        LYRIA => Ok(Box::new(lyria::Lyria::new(
            crate::secrets::get(crate::secrets::VERTEX_KEY)?,
            settings.lyria_model.clone(),
            settings.vertex_project.clone(),
            settings.vertex_location.clone(),
        ))),
        other => Err(format!("unknown audio provider \"{other}\"")),
    }
}

/// Provider info for every registered audio provider.
pub fn list_audio_providers(settings: &AppSettings) -> Vec<AudioProviderInfo> {
    AUDIO_PROVIDER_IDS
        .iter()
        .filter_map(|id| build_audio_provider(id, settings).ok())
        .map(|p| AudioProviderInfo {
            id: p.id().to_string(),
            display_name: p.display_name().to_string(),
            does_music: p.does_music(),
            does_sfx: p.does_sfx(),
            configured: p.configured(),
        })
        .collect()
}
