//! Pluggable image-generation providers. Adding a provider = implement `ImageProvider`
//! and register it in `build_provider`.

pub mod drawthings;
pub mod gamelab;
pub mod gemini_image;
pub mod openai_image;
pub mod spritecook;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::secrets;
use crate::settings::AppSettings;

/// A request to generate one image.
#[derive(Debug, Clone)]
pub struct GenRequest {
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    pub seed: Option<u64>,
    /// Request a transparent background where the provider supports it.
    pub transparent: bool,
    /// Optional style-reference images (PNG bytes). When present and the provider supports it
    /// (OpenAI gpt-image via `/images/edits`), the model matches their art style/palette while
    /// drawing the prompt's subject. Ignored by providers that can't take image inputs.
    pub references: Vec<Vec<u8>>,
}

/// How the background of a generated image was produced — drives post-processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "camelCase")]
pub enum BgMode {
    /// Opaque scene (backgrounds/splash) — nothing to remove.
    #[default]
    Opaque,
    /// Native transparency (alpha already present) — nothing to remove.
    Native,
    /// Solid chroma-key colour to be removed by the `chromakey` stage.
    Chroma,
}

/// The chroma-key colour requested for opaque providers (rare in painterly subjects).
pub const CHROMA_RGB: (u8, u8, u8) = (255, 0, 255);

/// Prompt suffix instructing an opaque model to paint a clean, keyable background.
pub fn chroma_instruction() -> &'static str {
    " The entire background must be one solid, perfectly flat, uniform magenta colour \
(hex #ff00ff), with no gradient, no texture, no shadow and no vignette — a single chroma-key \
colour that appears nowhere on the subject itself."
}

/// The result of a generation: PNG bytes + the seed used + how the background was made.
pub struct GenOutput {
    pub png: Vec<u8>,
    pub seed: Option<u64>,
    pub background: BgMode,
}

/// What a provider supports, surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
    pub supports_seed: bool,
    pub native_alpha: bool,
    /// Whether style-reference images can be attached to generations.
    pub supports_refs: bool,
    /// Whether the provider is ready to use (key present / endpoint configured).
    pub configured: bool,
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn supports_seed(&self) -> bool;
    fn native_alpha(&self) -> bool;
    /// Whether `GenRequest.references` are honoured (default: yes for cloud providers).
    fn supports_references(&self) -> bool {
        true
    }
    fn configured(&self) -> bool;
    async fn generate(&self, req: &GenRequest) -> Result<GenOutput, String>;
}

pub const DRAW_THINGS: &str = "drawthings";
pub const OPENAI_IMAGE: &str = "openai_image";
pub const GEMINI_IMAGE: &str = "gemini_image";
pub const SPRITECOOK: &str = "spritecook";
pub const GAMELAB: &str = "gamelab";

/// All provider ids, in display order.
pub const PROVIDER_IDS: &[&str] = &[DRAW_THINGS, OPENAI_IMAGE, GEMINI_IMAGE, SPRITECOOK, GAMELAB];

/// Construct a provider by id from current settings + secrets.
pub fn build_provider(id: &str, settings: &AppSettings) -> Result<Box<dyn ImageProvider>, String> {
    match id {
        DRAW_THINGS => Ok(Box::new(drawthings::DrawThings::new(
            settings.draw_things_url.clone(),
        ))),
        OPENAI_IMAGE => {
            let key = secrets::get(secrets::OPENAI_KEY)?;
            Ok(Box::new(openai_image::OpenAiImage::new(
                key,
                settings.openai_image_model.clone(),
            )))
        }
        GEMINI_IMAGE => {
            let key = secrets::get(secrets::GEMINI_KEY)?;
            Ok(Box::new(gemini_image::GeminiImage::new(
                key,
                settings.gemini_image_model.clone(),
            )))
        }
        SPRITECOOK => {
            let key = secrets::get(secrets::SPRITECOOK_KEY)?;
            Ok(Box::new(spritecook::SpriteCook::new(
                key,
                settings.spritecook_model.clone(),
            )))
        }
        GAMELAB => {
            let key = secrets::get(secrets::GAMELAB_KEY)?;
            Ok(Box::new(gamelab::Gamelab::new(key)))
        }
        other => Err(format!("unknown provider \"{other}\"")),
    }
}

/// Provider info for every registered provider.
pub fn list_providers(settings: &AppSettings) -> Vec<ProviderInfo> {
    PROVIDER_IDS
        .iter()
        .filter_map(|id| build_provider(id, settings).ok())
        .map(|p| ProviderInfo {
            id: p.id().to_string(),
            display_name: p.display_name().to_string(),
            supports_seed: p.supports_seed(),
            native_alpha: p.native_alpha(),
            supports_refs: p.supports_references(),
            configured: p.configured(),
        })
        .collect()
}

/// Model-native generation size for an asset's aspect ratio. SDXL/Flux degrade badly
/// below ~1024, so we generate with the **longest side at `target_long`** (scaling small
/// author sizes UP), rounded to a multiple of 64, min 512. Post-processing then
/// downscales the result to the true author px.
pub fn generation_size(author_w: u32, author_h: u32, target_long: u32) -> (u32, u32) {
    let longest = author_w.max(author_h).max(1) as f32;
    let scale = target_long as f32 / longest;
    // round to nearest 64, floor at 512 (8×64) so models stay in their trained range.
    let round64 = |v: f32| ((v / 64.0).round().max(8.0) as u32) * 64;
    (
        round64(author_w as f32 * scale),
        round64(author_h as f32 * scale),
    )
}

#[cfg(test)]
mod tests {
    use super::generation_size;

    #[test]
    fn targets_model_native_long_side() {
        // 3200×1800 background → longest side ~1024, multiples of 64.
        let (w, h) = generation_size(3200, 1800, 1024);
        assert_eq!(w, 1024);
        assert_eq!(h % 64, 0);
        assert!(h < 1024);
        // A tiny 220×220 symbol is generated LARGE (model-native), not tiny.
        let (sw, sh) = generation_size(220, 220, 1024);
        assert_eq!((sw, sh), (1024, 1024));
    }
}
