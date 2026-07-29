//! Glyph-on-base commands: generate a base template ONCE as its own asset (e.g.
//! `symbol_l_base`, an empty plate), then any symbol can pick that base, describe a
//! mark, and have it AI-generated as a clean shape and embossed on — pixel-identical
//! template across the whole set. Shapes live in a cross-game library.

use base64::Engine;

use super::{config_dir, now_ms, projects_root};
use crate::glyphs::doc::{AssetGlyph, GlyphInfo};
use crate::glyphs::{emboss, shape, store as gstore};
use crate::model::asset_record::{AssetRecord, Variation, VariationStatus};
use crate::providers::{self, GenRequest};
use crate::settings;
use crate::storage;
use crate::studio::segment::slugify;

/// Functional scaffold for generating a glyph as a maskable flat shape. Style-neutral
/// on purpose: the mark's character comes entirely from the user's description.
fn glyph_gen_prompt(desc: &str) -> (String, String) {
    (
        format!(
            "{desc}, drawn as a single flat solid pure-black pictogram silhouette, centred \
on a plain pure-white background. Clean smooth vector-like edges, generous stroke thickness, \
the mark fills most of the frame. No gradients, no shading, no 3D, no outline stroke, no \
texture, no frame, no background elements, no extra text or lettering."
        ),
        "gradient, shading, 3d render, bevel, texture, colour, background scenery, frame, \
border, watermark, signature, photo"
            .to_string(),
    )
}

fn data_url_png(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn encode_png(img: image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).map_err(|e| format!("encode png: {e}"))?;
    Ok(buf.into_inner())
}

// ── Shape library (cross-game) ────────────────────────────────────────────────

/// Every glyph in the cross-game library, newest first.
#[tauri::command]
#[specta::specta]
pub fn glyph_list(app: tauri::AppHandle) -> Result<Vec<GlyphInfo>, String> {
    gstore::list_glyphs(&projects_root(&app)?)
}

/// A glyph's shape as a data URL (white fill, alpha = the mark).
#[tauri::command]
#[specta::specta]
pub fn glyph_image(app: tauri::AppHandle, glyph_id: String) -> Result<String, String> {
    gstore::validate_id(&glyph_id)?;
    let path = gstore::shape_path(&projects_root(&app)?, &glyph_id);
    let bytes = std::fs::read(&path).map_err(|e| format!("read shape: {e}"))?;
    Ok(data_url_png(&bytes))
}

/// Generate a glyph shape from a prompt and save it to the library. When `glyph_id` is
/// non-empty an existing glyph is regenerated in place (same id, new shape + prompt);
/// otherwise a new id is derived from the prompt.
#[tauri::command]
#[specta::specta]
pub async fn glyph_generate(
    app: tauri::AppHandle,
    prompt: String,
    provider_id: String,
    glyph_id: String,
) -> Result<GlyphInfo, String> {
    let desc = prompt.trim().to_string();
    if desc.is_empty() {
        return Err("describe the glyph first".into());
    }
    let cfg = settings::load(&config_dir(&app)?);
    let root = projects_root(&app)?;
    let provider = providers::build_provider(&provider_id, &cfg)?;
    if !provider.configured() {
        return Err(format!("Provider \"{provider_id}\" is not configured."));
    }

    let (positive, negative) = glyph_gen_prompt(&desc);
    let req = GenRequest {
        prompt: positive,
        negative,
        width: 1024,
        height: 1024,
        seed: provider.supports_seed().then(|| now_ms() as u64),
        // A flat black-on-white render converts more reliably than native alpha.
        transparent: false,
        references: Vec::new(),
    };
    let out = provider.generate(&req).await?;

    let img = image::load_from_memory(&out.png)
        .map_err(|e| format!("decode generated glyph: {e}"))?
        .to_rgba8();
    let shape_img = shape::extract_shape(&img)?;
    let png = encode_png(image::DynamicImage::ImageRgba8(shape_img))?;

    let info = if glyph_id.trim().is_empty() {
        GlyphInfo { id: fresh_id(&root, &desc)?, name: desc.clone(), prompt: desc, created_at: now_ms() }
    } else {
        let mut info = gstore::read_glyph(&root, glyph_id.trim())?
            .ok_or_else(|| format!("glyph \"{glyph_id}\" not found"))?;
        info.prompt = desc.clone();
        info.name = desc;
        info
    };
    gstore::write_glyph(&root, &info)?;
    std::fs::write(gstore::shape_path(&root, &info.id), &png)
        .map_err(|e| format!("write shape: {e}"))?;
    Ok(info)
}

/// A library id derived from the description, deduped against existing entries.
fn fresh_id(root: &std::path::Path, desc: &str) -> Result<String, String> {
    let base_id = {
        let s: String = slugify(desc).chars().take(40).collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() { "glyph".to_string() } else { s }
    };
    let mut id = base_id.clone();
    let mut n = 1;
    while gstore::read_glyph(root, &id)?.is_some() {
        n += 1;
        id = format!("{base_id}_{n}");
    }
    Ok(id)
}

/// Import an image file as a glyph shape (secondary path). Any raster format works —
/// the same shape extraction runs on it (native alpha or flat background).
#[tauri::command]
#[specta::specta]
pub fn glyph_import(app: tauri::AppHandle, name: String, path: String) -> Result<GlyphInfo, String> {
    let root = projects_root(&app)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|_| "could not decode that file — use a raster image (PNG/JPG/WebP); export SVGs to PNG first".to_string())?
        .to_rgba8();
    let shape_img = shape::extract_shape(&img)?;
    let png = encode_png(image::DynamicImage::ImageRgba8(shape_img))?;

    let name = if name.trim().is_empty() { "imported glyph".to_string() } else { name.trim().to_string() };
    let info = GlyphInfo {
        id: fresh_id(&root, &name)?,
        name,
        prompt: String::new(),
        created_at: now_ms(),
    };
    gstore::write_glyph(&root, &info)?;
    std::fs::write(gstore::shape_path(&root, &info.id), &png)
        .map_err(|e| format!("write shape: {e}"))?;
    Ok(info)
}

/// Remove a glyph from the library (assets referencing it keep their prompt and can
/// simply regenerate).
#[tauri::command]
#[specta::specta]
pub fn glyph_delete(app: tauri::AppHandle, glyph_id: String) -> Result<(), String> {
    gstore::delete_glyph(&projects_root(&app)?, &glyph_id)
}

// ── Per-asset glyph-on-base ───────────────────────────────────────────────────

/// This asset's saved glyph settings, if it has any.
#[tauri::command]
#[specta::specta]
pub fn glyph_get(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
) -> Result<Option<AssetGlyph>, String> {
    gstore::read_asset_glyph(&projects_root(&app)?, &game_id, &asset_key)
}

/// Persist the asset's glyph settings (base choice, prompt, placement, emboss).
#[tauri::command]
#[specta::specta]
pub fn glyph_save(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    glyph: AssetGlyph,
) -> Result<(), String> {
    gstore::write_asset_glyph(&projects_root(&app)?, &game_id, &asset_key, &glyph)
}

/// Resolve the base template image: the base asset's active variation, preferring its
/// clean processed final over the raw generation.
fn load_base(
    base: &std::path::Path,
    game_id: &str,
    base_key: &str,
) -> Result<image::RgbaImage, String> {
    if base_key.trim().is_empty() {
        return Err("pick a base asset first (e.g. an empty plate generated as its own symbol)".into());
    }
    let record = storage::read_asset_record(base, game_id, base_key)?
        .ok_or_else(|| format!("base \"{base_key}\" has no generations yet"))?;
    let active = record
        .active_variation
        .as_ref()
        .ok_or_else(|| format!("base \"{base_key}\" has no active variation"))?;
    let var = record
        .variations
        .iter()
        .find(|v| &v.id == active)
        .ok_or_else(|| "active variation missing from the record".to_string())?;
    let bytes = var
        .stages
        .iter()
        .find(|s| s.name == "png")
        .and_then(|s| {
            let rel = format!("variations/{}/{}", var.id, s.file);
            storage::read_asset_file(base, game_id, base_key, &rel).ok()
        })
        .or_else(|| std::fs::read(storage::asset_dir(base, game_id, base_key).join(&var.raw_file)).ok())
        .ok_or_else(|| format!("could not read base \"{base_key}\"'s image"))?;
    Ok(image::load_from_memory(&bytes)
        .map_err(|e| format!("decode base image: {e}"))?
        .to_rgba8())
}

/// Compose a glyph onto its base in memory. Returns the composed image + the glyph's
/// own alpha mask on the same canvas.
fn compose_now(
    base: &std::path::Path,
    game_id: &str,
    glyph: &AssetGlyph,
) -> Result<(image::RgbaImage, image::GrayImage), String> {
    let glyph_id = glyph
        .glyph_id
        .as_deref()
        .ok_or_else(|| "generate the glyph first".to_string())?;
    let base_img = load_base(base, game_id, &glyph.base_asset)?;
    let shape_img = image::open(gstore::shape_path(base, glyph_id))
        .map_err(|e| format!("read glyph shape: {e}"))?
        .to_luma_alpha8();
    let alpha = image::GrayImage::from_fn(shape_img.width(), shape_img.height(), |x, y| {
        image::Luma([shape_img.get_pixel(x, y).0[1]])
    });
    let placed = emboss::place(&alpha, base_img.width(), base_img.height(), &glyph.transform);
    Ok(emboss::compose(&base_img, &placed, &glyph.params))
}

/// Both compose outputs, on the same canvas.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ComposedPreview {
    pub image: String,
    pub mask: String,
    pub width: u32,
    pub height: u32,
}

/// Compose in memory (nothing written) — the live preview while placing/tuning.
#[tauri::command]
#[specta::specta]
pub async fn glyph_preview(
    app: tauri::AppHandle,
    game_id: String,
    glyph: AssetGlyph,
) -> Result<ComposedPreview, String> {
    let base = projects_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (out, mask) = compose_now(&base, &game_id, &glyph)?;
        let (w, h) = out.dimensions();
        Ok(ComposedPreview {
            image: data_url_png(&encode_png(image::DynamicImage::ImageRgba8(out))?),
            mask: data_url_png(&encode_png(image::DynamicImage::ImageLuma8(mask))?),
            width: w,
            height: h,
        })
    })
    .await
    .map_err(|e| format!("preview task failed: {e}"))?
}

/// Compose and write the result as this asset's new ACTIVE variation (provider
/// `glyphs`), with `glyph_mask.png` beside the raw on the same canvas — the alpha the
/// game engine uses for win-glow. Also persists the glyph settings.
#[tauri::command]
#[specta::specta]
pub async fn glyph_compose(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    glyph: AssetGlyph,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        gstore::write_asset_glyph(&base, &game_id, &asset_key, &glyph)?;
        let (out, mask) = compose_now(&base, &game_id, &glyph)?;
        let base_had_alpha = out.pixels().any(|p| p.0[3] < 250);
        let png = encode_png(image::DynamicImage::ImageRgba8(out))?;
        let mask_png = encode_png(image::DynamicImage::ImageLuma8(mask))?;

        let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
            .unwrap_or_else(|| AssetRecord::new(&asset_key));
        let var_id = super::generate::next_variation_id(&record);
        let raw_file =
            storage::write_variation_file(&base, &game_id, &asset_key, &var_id, "raw.png", &png)?;
        storage::write_variation_file(
            &base, &game_id, &asset_key, &var_id, "glyph_mask.png", &mask_png,
        )?;
        record.variations.push(Variation {
            id: var_id.clone(),
            parent: record.active_variation.clone(),
            created_at: now_ms(),
            prompt_snapshot: format!("glyph on {}: {}", glyph.base_asset, glyph.prompt),
            provider: "glyphs".into(),
            model: None,
            seed: None,
            raw_file,
            status: VariationStatus::Ready,
            background: if base_had_alpha {
                providers::BgMode::Native
            } else {
                providers::BgMode::Opaque
            },
            stages: Vec::new(),
        mass_report: None,
            tone_report: None,
            audio_report: None,
            locked: false,
        });
        record.active_variation = Some(var_id);
        storage::write_asset_record(&base, &game_id, &record)?;
        Ok(record)
    })
    .await
    .map_err(|e| format!("compose task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_prompt_is_style_neutral_and_maskable() {
        let (pos, neg) = glyph_gen_prompt("a jagged lightning bolt");
        assert!(pos.contains("a jagged lightning bolt"));
        assert!(pos.contains("pure-black") && pos.contains("pure-white"));
        // No aesthetic vocabulary baked in — the description carries all character.
        for banned in ["macabre", "ornate", "premium", "gothic", "vintage"] {
            assert!(!pos.to_lowercase().contains(banned), "style word leaked: {banned}");
        }
        assert!(neg.contains("gradient"));
    }
}
