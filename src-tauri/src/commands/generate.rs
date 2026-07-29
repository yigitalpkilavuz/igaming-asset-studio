//! Image-generation commands: providers, generate/retry, variation history, settings.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;

use super::{config_dir, find_descriptor, now_ms, projects_root};
use crate::model::asset::{AssetDescriptor, AssetKind};
use crate::model::asset_record::{AssetRecord, Variation, VariationStatus};
use crate::providers::{self, GenRequest, ProviderInfo};
use crate::settings::{self, AppSettings};
use crate::storage;

/// Current non-secret app settings.
#[tauri::command]
#[specta::specta]
pub fn get_settings(app: tauri::AppHandle) -> Result<AppSettings, String> {
    Ok(settings::load(&config_dir(&app)?))
}

/// Persist app settings.
#[tauri::command]
#[specta::specta]
pub fn save_settings(app: tauri::AppHandle, settings: AppSettings) -> Result<(), String> {
    crate::settings::save(&config_dir(&app)?, &settings)
}

/// List the available image providers and whether each is configured.
#[tauri::command]
#[specta::specta]
pub fn list_image_providers(app: tauri::AppHandle) -> Result<Vec<ProviderInfo>, String> {
    Ok(providers::list_providers(&settings::load(&config_dir(&app)?)))
}

/// Generate new variation(s) for an asset. `count` (1–4) requests run CONCURRENTLY —
/// cloud providers handle parallel calls fine, and four takes at once is how you pick
/// a best. If the asset already has an active variation, each new one records it as
/// its `parent` (lineage). Returns the updated record; errors only when EVERY request
/// failed.
#[tauri::command]
#[specta::specta]
pub async fn generate_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    provider_id: String,
    reference_keys: Vec<String>,
    guide_key: String,
    count: u32,
) -> Result<AssetRecord, String> {
    let cfg = settings::load(&config_dir(&app)?);
    let base = projects_root(&app)?;
    generate_variation_core(cfg, base, game_id, asset_key, provider_id, reference_keys, guide_key, count)
        .await
}

/// App-free core of `generate_variation` — shared with the headless CLI.
#[allow(clippy::too_many_arguments)]
pub async fn generate_variation_core(
    cfg: crate::settings::AppSettings,
    base: std::path::PathBuf,
    game_id: String,
    asset_key: String,
    provider_id: String,
    reference_keys: Vec<String>,
    guide_key: String,
    count: u32,
) -> Result<AssetRecord, String> {
    let project = storage::read_project(&base, &game_id)?;
    let descriptor = find_descriptor(&project.config, &asset_key)?;

    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(&asset_key));
    let mut assembled =
        crate::prompts::assemble::assemble(&project.config, &descriptor, &record.prompt);
    if assembled.positive.trim().is_empty() {
        return Err("Add a subject or prompt first.".into());
    }

    let provider = providers::build_provider(&provider_id, &cfg)?;
    if !provider.configured() {
        return Err(format!("Provider \"{provider_id}\" is not configured."));
    }

    let (gw, gh) = gen_dims(&descriptor);
    let seed = if provider.supports_seed() {
        Some(random_seed())
    } else {
        None
    };
    // Load any style-reference images (other assets' best available image) to steer the look.
    // The game's style anchor (if one exists) always rides first.
    let mut references = load_reference_images(&base, &game_id, &asset_key, &reference_keys);
    prepend_anchor(&base, &game_id, &mut references);
    // Optional composition steering: render the named guide template's zones as a layout
    // reference so ref-capable providers keep key subject matter clear of blocked areas.
    // (For providers without reference support this is a no-op; guides remain
    // verification overlays in the preview either way.)
    if !guide_key.trim().is_empty() && provider.supports_references() {
        if let Some(t) =
            project.config.scene.guides.iter().find(|g| g.key == guide_key.trim())
        {
            let (w, h) = (descriptor.author_w.unwrap_or(1024), descriptor.author_h.unwrap_or(1024));
            let guide_png = render_guide_layout(&t.zones, w, h)?;
            if references.len() >= MAX_REFERENCES {
                references.truncate(MAX_REFERENCES - 1);
            }
            references.push(guide_png);
            assembled.positive.push_str(
                " A layout-guide image is attached (flat grey rectangles on a light \
field): the darkened rectangles are zones covered by the game's grid and HUD — keep all \
key subject matter and focal detail OUT of those rectangles; only continuing low-detail \
texture may pass beneath them. Do NOT copy the guide's style or draw its rectangles.",
            );
        }
    }
    let references = references;
    let base_req = GenRequest {
        prompt: assembled.positive.clone(),
        negative: assembled.negative.clone(),
        width: gw,
        height: gh,
        seed,
        transparent: needs_alpha(descriptor.kind),
        references,
    };

    // Fire all requests at once (each with its own seed where seeds exist) and keep
    // whatever came back — one bad roll out of four must not sink the other three.
    let count = count.clamp(1, 4) as usize;
    let outs = futures::future::join_all((0..count).map(|_| {
        let mut req = base_req.clone();
        if provider.supports_seed() {
            req.seed = Some(random_seed());
        }
        let provider = &provider;
        async move { (req.seed, provider.generate(&req).await) }
    }))
    .await;

    let mut errors = Vec::new();
    let parent = record.active_variation.clone();
    for (used_seed, out) in outs {
        let out = match out {
            Ok(out) => out,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let var_id = next_variation_id(&record);
        let raw_file = storage::write_variation_file(
            &base, &game_id, &asset_key, &var_id, "raw.png", &out.png,
        )?;
        record.variations.push(Variation {
            id: var_id.clone(),
            parent: parent.clone(),
            created_at: now_ms(),
            prompt_snapshot: assembled.positive.clone(),
            provider: provider_id.clone(),
            model: None,
            seed: out.seed.or(used_seed).map(|s| s as f64),
            raw_file,
            status: VariationStatus::Ready,
            background: out.background,
            stages: Vec::new(),
            mass_report: None,
            tone_report: None,
            audio_report: None,
            locked: false,
        });
        record.active_variation = Some(var_id);
    }
    if record.active_variation == parent && !errors.is_empty() {
        return Err(errors.remove(0));
    }
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Max reference images to attach to one generation (keeps the request light + cheap).
const MAX_REFERENCES: usize = 4;

/// Render a guide template's zones as a flat layout image (light field, dark blocked
/// rectangles) at ≤1024 long side — attached as a composition reference.
fn render_guide_layout(
    zones: &[crate::model::game_config::GuideZone],
    w: u32,
    h: u32,
) -> Result<Vec<u8>, String> {
    let long = w.max(h).max(1) as f64;
    let s = (1024.0 / long).min(1.0);
    let gw = ((w as f64 * s).round() as u32).max(1);
    let gh = ((h as f64 * s).round() as u32).max(1);
    let mut img = image::RgbaImage::from_pixel(gw, gh, image::Rgba([232, 232, 232, 255]));
    for z in zones {
        let x0 = ((z.x / 100.0 * gw as f64).round().max(0.0) as u32).min(gw);
        let y0 = ((z.y / 100.0 * gh as f64).round().max(0.0) as u32).min(gh);
        let x1 = (((z.x + z.w) / 100.0 * gw as f64).round().max(0.0) as u32).min(gw);
        let y1 = (((z.y + z.h) / 100.0 * gh as f64).round().max(0.0) as u32).min(gh);
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, image::Rgba([72, 72, 72, 255]));
            }
        }
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode guide layout: {e}"))?;
    Ok(buf.into_inner())
}

/// Load style-reference images for the given asset keys: each key's active variation, preferring
/// its clean processed final (transparent PNG) over the raw generation. Re-encoded to PNG and
/// downscaled to a modest size so the upload stays small. Unreadable/self references are skipped.
fn load_reference_images(
    base: &std::path::Path,
    game_id: &str,
    self_key: &str,
    keys: &[String],
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for key in keys {
        if key == self_key || out.len() >= MAX_REFERENCES {
            continue;
        }
        let Ok(Some(record)) = storage::read_asset_record(base, game_id, key) else {
            continue;
        };
        let Some(active) = record.active_variation.as_ref() else {
            continue;
        };
        let Some(var) = record.variations.iter().find(|v| &v.id == active) else {
            continue;
        };
        // Prefer the processed transparent final; fall back to the raw generation.
        let bytes = var
            .stages
            .iter()
            .find(|s| s.name == "png")
            .and_then(|s| {
                let rel = format!("variations/{}/{}", var.id, s.file);
                storage::read_asset_file(base, game_id, key, &rel).ok()
            })
            .or_else(|| {
                let path = storage::asset_dir(base, game_id, key).join(&var.raw_file);
                std::fs::read(path).ok()
            });
        let Some(bytes) = bytes else { continue };
        if let Some(png) = reencode_reference(&bytes) {
            out.push(png);
        }
    }
    out
}

/// Decode a reference image and re-encode as PNG, downscaling so the longest side is ≤ 1024
/// (references only need to convey style, not full resolution).
fn reencode_reference(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let long = img.width().max(img.height());
    let img = if long > 1024 {
        img.resize(1024, 1024, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

// ── Style anchor ────────────────────────────────────────────────────────────────
// A per-game style-calibration image (NOT a game asset): generated once from the style
// master, stored at the project level, and automatically attached as the FIRST reference
// of every generation. Manual per-asset references stack on top (total capped).

fn anchor_file(base: &std::path::Path, game_id: &str) -> std::path::PathBuf {
    storage::project_dir(base, game_id).join("anchor").join("anchor.png")
}

/// If the game has a style anchor, insert it as the first reference (capped overall).
pub(crate) fn prepend_anchor(
    base: &std::path::Path,
    game_id: &str,
    references: &mut Vec<Vec<u8>>,
) {
    let Ok(bytes) = std::fs::read(anchor_file(base, game_id)) else {
        return;
    };
    let Some(png) = reencode_reference(&bytes) else {
        return;
    };
    references.insert(0, png);
    references.truncate(MAX_REFERENCES);
}

/// The anchor's assembled prompt: the game's style master + the fixed calibration-plate
/// composition (no subject, no symbol — pure style).
fn anchor_prompt(config: &crate::model::game_config::GameConfig) -> (String, String) {
    let comp = crate::prompts::templates::style_anchor_composition();
    let style = config.style_prompt.trim();
    let positive = if style.is_empty() {
        comp.to_string()
    } else {
        format!("{}. {}", style.trim_end_matches('.'), comp)
    };
    let floor = crate::prompts::templates::style_anchor_negative();
    let extra = config.negative_prompt.trim();
    let negative = if extra.is_empty() {
        floor
    } else {
        format!("{floor}, {extra}")
    };
    (positive, negative)
}

/// The auto-assembled anchor prompt (style master + calibration-plate composition) —
/// shown editable in the UI so the user can customise before generating.
#[tauri::command]
#[specta::specta]
pub fn style_anchor_prompt(app: tauri::AppHandle, game_id: String) -> Result<String, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    Ok(anchor_prompt(&project.config).0)
}

/// Generate (or regenerate) the game's style anchor with the given provider.
/// A non-empty `override_prompt` is used verbatim as the positive (the anchor's
/// anti-slop negative floor still applies). Returns a data URL for immediate preview.
#[tauri::command]
#[specta::specta]
pub async fn generate_style_anchor(
    app: tauri::AppHandle,
    game_id: String,
    provider_id: String,
    override_prompt: String,
) -> Result<String, String> {
    let cfg = settings::load(&config_dir(&app)?);
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;

    let provider = providers::build_provider(&provider_id, &cfg)?;
    if !provider.configured() {
        return Err(format!("Provider \"{provider_id}\" is not configured."));
    }
    let (auto_positive, negative) = anchor_prompt(&project.config);
    let positive = if override_prompt.trim().is_empty() {
        auto_positive
    } else {
        override_prompt.trim().to_string()
    };
    let seed = if provider.supports_seed() { Some(random_seed()) } else { None };
    let req = GenRequest {
        prompt: positive,
        negative,
        width: 1024,
        height: 1024,
        seed,
        transparent: false, // a flat plate — background removal never applies
        references: Vec::new(),
    };
    let out = provider.generate(&req).await?;

    let path = anchor_file(&base, &game_id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create anchor dir: {e}"))?;
    }
    std::fs::write(&path, &out.png).map_err(|e| format!("write anchor: {e}"))?;
    Ok(data_url_png(&out.png))
}

/// The current style anchor as a data URL, or `None` if the game has no anchor.
#[tauri::command]
#[specta::specta]
pub fn get_style_anchor(app: tauri::AppHandle, game_id: String) -> Result<Option<String>, String> {
    let base = projects_root(&app)?;
    match std::fs::read(anchor_file(&base, &game_id)) {
        Ok(bytes) => Ok(Some(data_url_png(&bytes))),
        Err(_) => Ok(None),
    }
}

/// Whether the game has a style anchor (cheap presence check for the bench chip).
#[tauri::command]
#[specta::specta]
pub fn style_anchor_present(app: tauri::AppHandle, game_id: String) -> Result<bool, String> {
    let base = projects_root(&app)?;
    Ok(anchor_file(&base, &game_id).is_file())
}

/// Use an existing image file as the style anchor (normalised to PNG, ≤1024 long side).
#[tauri::command]
#[specta::specta]
pub fn import_style_anchor(
    app: tauri::AppHandle,
    game_id: String,
    path: String,
) -> Result<String, String> {
    let base = projects_root(&app)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
    let png = reencode_reference(&bytes).ok_or("could not decode that image")?;
    let dest = anchor_file(&base, &game_id);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create anchor dir: {e}"))?;
    }
    std::fs::write(&dest, &png).map_err(|e| format!("write anchor: {e}"))?;
    Ok(data_url_png(&png))
}

/// Remove the style anchor — generations stop referencing it.
#[tauri::command]
#[specta::specta]
pub fn remove_style_anchor(app: tauri::AppHandle, game_id: String) -> Result<(), String> {
    let base = projects_root(&app)?;
    let path = anchor_file(&base, &game_id);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| format!("remove anchor: {e}"))?;
    }
    Ok(())
}

fn data_url_png(bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

// ── Symbol sets: plan → sheet (review/reroll) → commit ───────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetPlanCell {
    pub asset_key: String,
    pub label: String,
}

/// The assembled sheet prompt, shown to the user BEFORE generating — editable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetPlan {
    pub positive: String,
    pub negative: String,
    pub cols: u32,
    pub rows: u32,
    pub cells: Vec<SetPlanCell>,
}

/// A generated (but not yet cut) sheet: preview + where the cuts will land.
/// Seams are fractions of the sheet's width/height (internal boundaries only).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetSheet {
    /// History id of this sheet (e.g. "s003") — select/adjust/commit address it.
    pub id: String,
    pub png: String,
    pub cols: u32,
    pub rows: u32,
    pub seams_x: Vec<f64>,
    pub seams_y: Vec<f64>,
}

/// Pending-sheet sidecar so Commit cuts exactly what the preview showed.
#[derive(serde::Serialize, serde::Deserialize)]
struct SetPendingMeta {
    #[serde(default)]
    created_at: f64,
    asset_keys: Vec<String>,
    cols: u32,
    rows: u32,
    background: crate::providers::BgMode,
    seams_x: Vec<u32>,
    seams_y: Vec<u32>,
}

fn set_pending_dir(base: &std::path::Path, game_id: &str) -> std::path::PathBuf {
    storage::project_dir(base, game_id).join("set_pending")
}

/// How many sheets a game keeps in its set history (oldest pruned beyond this).
const SET_HISTORY_CAP: usize = 12;

fn sheet_ids(root: &std::path::Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().join("sheet.png").is_file())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

fn next_sheet_id(root: &std::path::Path) -> String {
    let max = sheet_ids(root)
        .iter()
        .filter_map(|id| id.strip_prefix('s').and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    format!("s{:03}", max + 1)
}

/// The current sheet's directory, migrating the pre-history single-slot layout
/// (sheet.png directly in set_pending/) into "s001" on first touch.
fn current_sheet_dir(
    base: &std::path::Path,
    game_id: &str,
) -> Result<(String, std::path::PathBuf), String> {
    let root = set_pending_dir(base, game_id);
    // Legacy migration: single-slot layout → s001.
    if root.join("sheet.png").is_file() {
        let dir = root.join("s001");
        std::fs::create_dir_all(&dir).map_err(|e| format!("migrate pending: {e}"))?;
        for f in ["sheet.png", "meta.json"] {
            let _ = std::fs::rename(root.join(f), dir.join(f));
        }
        std::fs::write(root.join("current.txt"), "s001")
            .map_err(|e| format!("write current: {e}"))?;
    }
    let id = std::fs::read_to_string(root.join("current.txt"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no pending sheet — generate one first".to_string())?;
    let dir = root.join(&id);
    if !dir.join("sheet.png").is_file() {
        return Err("no pending sheet — generate one first".into());
    }
    Ok((id, dir))
}

fn write_sheet_thumb(dir: &std::path::Path, sheet: &image::RgbaImage) -> Result<(), String> {
    let thumb = image::DynamicImage::ImageRgba8(sheet.clone())
        .resize(256, 256, image::imageops::FilterType::Triangle);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode thumb: {e}"))?;
    std::fs::write(dir.join("thumb.png"), buf.into_inner())
        .map_err(|e| format!("write thumb: {e}"))
}

/// Resolve selected symbol keys → ordered (asset_key, cell description) + grid shape +
/// assembled prompts. Shared by plan/generate so the preview never lies.
fn assemble_set_plan(
    project: &crate::model::project::Project,
    asset_keys: &[String],
) -> Result<(Vec<(String, String)>, u32, u32, String, String), String> {
    let mut cells: Vec<(String, String)> = Vec::new();
    for key in asset_keys {
        let Some(sym_key) = key.strip_prefix("symbol_") else { continue };
        let Some(sym) = project.config.symbols.iter().find(|s| s.key == sym_key) else {
            continue;
        };

        let desc = if sym.description.trim().is_empty() {
            sym.name.clone()
        } else {
            sym.description.clone()
        };
        use crate::model::game_config::SymbolRole;
        let tier = match sym.role {
            SymbolRole::Low => "a simpler low-pay symbol",
            SymbolRole::High => "a premium high-pay symbol",
            SymbolRole::Wild | SymbolRole::ExpandingWild => "the WILD symbol",
            SymbolRole::Scatter => "the SCATTER symbol",
            SymbolRole::Bonus => "the BONUS symbol",
            SymbolRole::Special => "a special symbol",
        };
        cells.push((key.clone(), format!("{tier} — {desc}")));
    }
    if cells.len() < 2 {
        return Err("select at least two symbols for a set".into());
    }
    let n = cells.len();
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = (n as u32).div_ceil(cols);

    let cell_list = cells
        .iter()
        .enumerate()
        .map(|(i, (_, t))| format!("cell {}: {}", i + 1, t))
        .collect::<Vec<_>>()
        .join("; ");
    let comp = crate::prompts::templates::symbol_set_composition(rows, cols, n, &cell_list);
    let style = project.config.style_prompt.trim();
    let positive =
        if style.is_empty() { comp } else { format!("{}. {}", style.trim_end_matches('.'), comp) };
    let floor = crate::prompts::templates::symbol_set_negative();
    let extra = project.config.negative_prompt.trim();
    let negative = if extra.is_empty() { floor } else { format!("{floor}, {extra}") };
    Ok((cells, cols, rows, positive, negative))
}

/// Assemble the sheet prompt for the selected symbols WITHOUT generating — the composer
/// shows it for review; the user may override it wholesale.
#[tauri::command]
#[specta::specta]
pub fn plan_symbol_set(
    app: tauri::AppHandle,
    game_id: String,
    asset_keys: Vec<String>,
) -> Result<SetPlan, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let (cells, cols, rows, positive, negative) = assemble_set_plan(&project, &asset_keys)?;
    Ok(SetPlan {
        positive,
        negative,
        cols,
        rows,
        cells: cells
            .into_iter()
            .map(|(asset_key, label)| SetPlanCell { asset_key, label })
            .collect(),
    })
}

/// Generate ONE sheet of the selected symbols and hold it as PENDING — nothing is cut
/// yet. Returns the preview plus the detected cut seams so the user can judge both the
/// art AND the split before committing. Calling again (reroll) replaces the pending
/// sheet. `prompt_override` non-empty → used verbatim instead of the assembled prompt.
#[tauri::command]
#[specta::specta]
pub async fn generate_symbol_set_sheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_keys: Vec<String>,
    provider_id: String,
    prompt_override: String,
    reference_keys: Vec<String>,
) -> Result<SetSheet, String> {
    let cfg = settings::load(&config_dir(&app)?);
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let (cells, cols, rows, positive, negative) = assemble_set_plan(&project, &asset_keys)?;
    let positive = if prompt_override.trim().is_empty() {
        positive
    } else {
        prompt_override.trim().to_string()
    };

    let provider = providers::build_provider(&provider_id, &cfg)?;
    if !provider.configured() {
        return Err(format!("Provider \"{provider_id}\" is not configured."));
    }

    // Canvas: ~512 px per cell at the grid's aspect, capped at the 2048 ceiling.
    let (gw, gh) = providers::generation_size(cols * 512, rows * 512, 2048);
    // Style references: any chosen assets' best images ride along; the game's style
    // anchor (if set) always rides first.
    let mut references = load_reference_images(&base, &game_id, "", &reference_keys);
    prepend_anchor(&base, &game_id, &mut references);
    let req = GenRequest {
        prompt: positive,
        negative,
        width: gw,
        height: gh,
        seed: provider.supports_seed().then(random_seed),
        transparent: true,
        references,
    };
    let out = provider.generate(&req).await?;
    let bg = out.background;
    let keys: Vec<String> = cells.iter().map(|(k, _)| k.clone()).collect();

    tauri::async_runtime::spawn_blocking(move || {
        let sheet = image::load_from_memory(&out.png)
            .map_err(|e| format!("decode sheet failed: {e}"))?
            .to_rgba8();
        hold_pending_sheet(&base, &game_id, &out.png, &sheet, keys, cols, rows, bg)
    })
    .await
    .map_err(|e| format!("sheet task failed: {e}"))?
}

/// Use the user's OWN image as the pending sheet — same review/seams/commit flow as a
/// generated one, no provider involved. Background handling is detected from the
/// image: real alpha → native, magenta border → chroma, anything else → opaque
/// (rembg keys each cut cell later, exactly like a single-symbol import).
#[tauri::command]
#[specta::specta]
pub async fn import_symbol_set_sheet(
    app: tauri::AppHandle,
    game_id: String,
    asset_keys: Vec<String>,
    path: String,
) -> Result<SetSheet, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let (cells, cols, rows, _, _) = assemble_set_plan(&project, &asset_keys)?;
    let keys: Vec<String> = cells.iter().map(|(k, _)| k.clone()).collect();
    let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;

    tauri::async_runtime::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes)
            .map_err(|e| format!("could not decode that image: {e}"))?;
        let sheet = img.to_rgba8();
        let bg = detect_sheet_background(&sheet);
        // Normalise to PNG so pending + commit always speak one format.
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(sheet.clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode png: {e}"))?;
        hold_pending_sheet(&base, &game_id, &buf.into_inner(), &sheet, keys, cols, rows, bg)
    })
    .await
    .map_err(|e| format!("sheet task failed: {e}"))?
}

/// Guess how an imported sheet's background should be removed downstream.
fn detect_sheet_background(img: &image::RgbaImage) -> crate::providers::BgMode {
    use crate::providers::{BgMode, CHROMA_RGB};
    if img.pixels().any(|p| p.0[3] < 250) {
        return BgMode::Native;
    }
    let (w, h) = img.dimensions();
    let corners = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)];
    let near_chroma = corners
        .iter()
        .filter(|&&(x, y)| {
            let p = img.get_pixel(x, y);
            let dr = (p.0[0] as f32 - CHROMA_RGB.0 as f32) / 255.0;
            let dg = (p.0[1] as f32 - CHROMA_RGB.1 as f32) / 255.0;
            let db = (p.0[2] as f32 - CHROMA_RGB.2 as f32) / 255.0;
            (dr * dr + dg * dg + db * db).sqrt() / (3.0f32).sqrt() < 0.22
        })
        .count();
    if near_chroma >= 3 {
        BgMode::Chroma
    } else {
        BgMode::Opaque
    }
}

/// Detect seams, persist the sheet + meta as THE pending set, and describe it for the
/// composer preview. Shared by generate and import so commit always cuts exactly what
/// the preview showed.
#[allow(clippy::too_many_arguments)]
fn hold_pending_sheet(
    base: &std::path::Path,
    game_id: &str,
    png: &[u8],
    sheet: &image::RgbaImage,
    asset_keys: Vec<String>,
    cols: u32,
    rows: u32,
    bg: crate::providers::BgMode,
) -> Result<SetSheet, String> {
    let (w, h) = sheet.dimensions();
    let mask = content_mask(sheet, bg);
    // The model sometimes redraws the layout its own way (ask for 4×3, get 5×2) —
    // trust the IMAGE, not the request.
    let (cols, rows) = detect_grid(&mask, w, h, asset_keys.len(), cols, rows);
    let seams_x = best_seams(&col_profile(&mask, w, h), w, cols);
    let seams_y = best_seams(&row_profile(&mask, w, h), h, rows);

    // Every sheet lands in the game's HISTORY (own slot + thumb) and becomes current —
    // a reroll never destroys the previous roll; the composer can go back.
    let root = set_pending_dir(base, game_id);
    std::fs::create_dir_all(&root).map_err(|e| format!("create pending dir: {e}"))?;
    let id = next_sheet_id(&root);
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create sheet dir: {e}"))?;
    std::fs::write(dir.join("sheet.png"), png).map_err(|e| format!("write pending sheet: {e}"))?;
    write_sheet_thumb(&dir, sheet)?;
    let meta = SetPendingMeta {
        created_at: crate::commands::now_ms(),
        asset_keys,
        cols,
        rows,
        background: bg,
        seams_x: seams_x.clone(),
        seams_y: seams_y.clone(),
    };
    std::fs::write(
        dir.join("meta.json"),
        serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write pending meta: {e}"))?;
    std::fs::write(root.join("current.txt"), &id).map_err(|e| format!("write current: {e}"))?;
    // Prune beyond the cap, oldest first (never the current one).
    let ids = sheet_ids(&root);
    if ids.len() > SET_HISTORY_CAP {
        for old in &ids[..ids.len() - SET_HISTORY_CAP] {
            if old != &id {
                let _ = std::fs::remove_dir_all(root.join(old));
            }
        }
    }

    Ok(SetSheet {
        id,
        png: data_url_png(png),
        cols,
        rows,
        seams_x: seams_x.iter().map(|&x| x as f64 / w as f64).collect(),
        seams_y: seams_y.iter().map(|&y| y as f64 / h as f64).collect(),
    })
}

/// One entry of a game's sheet history (newest last), thumb-sized for the filmstrip.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetSheetInfo {
    pub id: String,
    pub thumb: String,
    pub cols: u32,
    pub rows: u32,
    pub created_at: f64,
    pub symbol_count: u32,
    pub current: bool,
}

/// The game's symbol-set sheet history: every generated/imported sheet kept (capped),
/// so a reroll is never destructive.
#[tauri::command]
#[specta::specta]
pub fn list_symbol_set_sheets(
    app: tauri::AppHandle,
    game_id: String,
) -> Result<Vec<SetSheetInfo>, String> {
    let base = projects_root(&app)?;
    let root = set_pending_dir(&base, &game_id);
    let current = std::fs::read_to_string(root.join("current.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in sheet_ids(&root) {
        let dir = root.join(&id);
        let Ok(meta) = std::fs::read(dir.join("meta.json"))
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<SetPendingMeta>(&b).map_err(|e| e.to_string()))
        else {
            continue;
        };
        // Thumb may be missing on migrated legacy slots — derive it lazily.
        if !dir.join("thumb.png").is_file() {
            if let Ok(img) = image::open(dir.join("sheet.png")) {
                let _ = write_sheet_thumb(&dir, &img.to_rgba8());
            }
        }
        let thumb = std::fs::read(dir.join("thumb.png")).map(|b| data_url_png(&b)).unwrap_or_default();
        out.push(SetSheetInfo {
            current: id == current,
            id,
            thumb,
            cols: meta.cols,
            rows: meta.rows,
            created_at: meta.created_at,
            symbol_count: meta.asset_keys.len() as u32,
        });
    }
    Ok(out)
}

/// Make a history sheet the CURRENT cut candidate again and return its full preview
/// (its own grid + seams, exactly as it was).
#[tauri::command]
#[specta::specta]
pub fn select_symbol_set_sheet(
    app: tauri::AppHandle,
    game_id: String,
    sheet_id: String,
) -> Result<SetSheet, String> {
    let base = projects_root(&app)?;
    let root = set_pending_dir(&base, &game_id);
    let dir = root.join(&sheet_id);
    let png = std::fs::read(dir.join("sheet.png"))
        .map_err(|_| format!("sheet \"{sheet_id}\" not found in history"))?;
    let meta: SetPendingMeta = serde_json::from_slice(
        &std::fs::read(dir.join("meta.json")).map_err(|e| format!("read sheet meta: {e}"))?,
    )
    .map_err(|e| format!("bad sheet meta: {e}"))?;
    let (w, h) = image::image_dimensions(dir.join("sheet.png"))
        .map_err(|e| format!("read sheet dims: {e}"))?;
    std::fs::write(root.join("current.txt"), &sheet_id)
        .map_err(|e| format!("write current: {e}"))?;
    Ok(SetSheet {
        id: sheet_id,
        png: data_url_png(&png),
        cols: meta.cols,
        rows: meta.rows,
        seams_x: meta.seams_x.iter().map(|&x| x as f64 / w as f64).collect(),
        seams_y: meta.seams_y.iter().map(|&y| y as f64 / h as f64).collect(),
    })
}

/// Manual override for the pending sheet's cut: change the grid (seams re-detect for
/// the new shape) or pin exact seam positions (fractions 0–1, from dragging the lines
/// in the preview). Persisted into the pending meta, so Commit cuts exactly this.
/// Pass empty seam vecs to re-detect; pass full vecs (cols−1 / rows−1 entries) to pin.
#[tauri::command]
#[specta::specta]
pub async fn adjust_symbol_set(
    app: tauri::AppHandle,
    game_id: String,
    cols: u32,
    rows: u32,
    seams_x: Vec<f64>,
    seams_y: Vec<f64>,
) -> Result<SetSheet, String> {
    let base = projects_root(&app)?;
    let (id, dir) = current_sheet_dir(&base, &game_id)?;
    let png = std::fs::read(dir.join("sheet.png"))
        .map_err(|_| "no pending sheet — generate one first".to_string())?;
    let mut meta: SetPendingMeta = serde_json::from_slice(
        &std::fs::read(dir.join("meta.json")).map_err(|e| format!("read pending meta: {e}"))?,
    )
    .map_err(|e| format!("bad pending meta: {e}"))?;
    let cols = cols.max(1);
    let rows = rows.max(1);
    if ((cols * rows) as usize) < meta.asset_keys.len() {
        return Err(format!(
            "a {rows}×{cols} grid holds {} cells but the set has {} symbols",
            cols * rows,
            meta.asset_keys.len()
        ));
    }

    tauri::async_runtime::spawn_blocking(move || {
        let sheet = image::load_from_memory(&png)
            .map_err(|e| format!("decode sheet failed: {e}"))?
            .to_rgba8();
        let (w, h) = sheet.dimensions();

        // Pinned fractions → px (sorted + clamped); anything else → re-detect.
        let to_px = |fracs: &[f64], extent: u32, div: u32| -> Option<Vec<u32>> {
            (fracs.len() == (div - 1) as usize).then(|| {
                let mut px: Vec<u32> = fracs
                    .iter()
                    .map(|f| ((f.clamp(0.01, 0.99)) * extent as f64).round() as u32)
                    .collect();
                px.sort_unstable();
                px
            })
        };
        let mask = content_mask(&sheet, meta.background);
        let seams_x = to_px(&seams_x, w, cols)
            .unwrap_or_else(|| best_seams(&col_profile(&mask, w, h), w, cols));
        let seams_y = to_px(&seams_y, h, rows)
            .unwrap_or_else(|| best_seams(&row_profile(&mask, w, h), h, rows));

        meta.cols = cols;
        meta.rows = rows;
        meta.seams_x = seams_x.clone();
        meta.seams_y = seams_y.clone();
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("write pending meta: {e}"))?;

        Ok(SetSheet {
            id,
            png: data_url_png(&png),
            cols,
            rows,
            seams_x: seams_x.iter().map(|&x| x as f64 / w as f64).collect(),
            seams_y: seams_y.iter().map(|&y| y as f64 / h as f64).collect(),
        })
    })
    .await
    .map_err(|e| format!("adjust task failed: {e}"))?
}

/// Cut the pending sheet along the detected seams, upscale each cell to working
/// resolution, and land every cell as its symbol's new ACTIVE variation. The normal
/// Process pass handles keying + the fit pass per symbol afterwards.
#[tauri::command]
#[specta::specta]
pub async fn commit_symbol_set(
    app: tauri::AppHandle,
    game_id: String,
) -> Result<Vec<AssetRecord>, String> {
    let base = projects_root(&app)?;
    let (_id, dir) = current_sheet_dir(&base, &game_id)?;
    let png = std::fs::read(dir.join("sheet.png"))
        .map_err(|_| "no pending sheet — generate one first".to_string())?;
    let meta: SetPendingMeta = serde_json::from_slice(
        &std::fs::read(dir.join("meta.json")).map_err(|e| format!("read pending meta: {e}"))?,
    )
    .map_err(|e| format!("bad pending meta: {e}"))?;
    let pre = crate::processing::detect();

    tauri::async_runtime::spawn_blocking(move || {
        let sheet = image::load_from_memory(&png)
            .map_err(|e| format!("decode sheet failed: {e}"))?
            .to_rgba8();
        let cut = split_at_seams(&sheet, &meta.seams_x, &meta.seams_y, meta.asset_keys.len());
        let mut records = Vec::new();
        for (i, (asset_key, cell)) in meta.asset_keys.iter().zip(cut).enumerate() {
            let up = upscale_cell(cell, 1024, i, &pre);
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(up)
                .write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| format!("encode cell failed: {e}"))?;

            let mut record = storage::read_asset_record(&base, &game_id, asset_key)?
                .unwrap_or_else(|| AssetRecord::new(asset_key));
            let var_id = next_variation_id(&record);
            let raw_file = storage::write_variation_file(
                &base,
                &game_id,
                asset_key,
                &var_id,
                "raw.png",
                &buf.into_inner(),
            )?;
            record.variations.push(Variation {
                id: var_id.clone(),
                parent: record.active_variation.clone(),
                created_at: now_ms(),
                prompt_snapshot: format!(
                    "symbol set ({}×{}, cell {})",
                    meta.rows,
                    meta.cols,
                    i + 1
                ),
                provider: "set".into(),
                model: None,
                seed: None,
                raw_file,
                status: VariationStatus::Ready,
                background: meta.background,
                stages: Vec::new(),
                mass_report: None,
            tone_report: None,
            audio_report: None,
                locked: false,
            });
            record.active_variation = Some(var_id);
            storage::write_asset_record(&base, &game_id, &record)?;
            records.push(record);
        }
        // History survives the commit — an old sheet can be re-selected and re-cut.
        Ok(records)
    })
    .await
    .map_err(|e| format!("set task failed: {e}"))?
}

/// Per-pixel "is subject" mask — how we see the sheet the way the cut does.
/// Chroma sheets: anything not close to the key colour; native alpha: anything opaque;
/// opaque sheets: anything far from the border's background colour.
fn content_mask(img: &image::RgbaImage, bg: crate::providers::BgMode) -> Vec<bool> {
    use crate::providers::{BgMode, CHROMA_RGB};
    let (w, h) = img.dimensions();
    let colour_dist = |px: &image::Rgba<u8>, key: (u8, u8, u8)| {
        let dr = (px.0[0] as f32 - key.0 as f32) / 255.0;
        let dg = (px.0[1] as f32 - key.1 as f32) / 255.0;
        let db = (px.0[2] as f32 - key.2 as f32) / 255.0;
        (dr * dr + dg * dg + db * db).sqrt() / (3.0f32).sqrt()
    };
    match bg {
        BgMode::Native => img.pixels().map(|p| p.0[3] > 26).collect(),
        BgMode::Chroma => img
            .pixels()
            .map(|p| p.0[3] > 26 && colour_dist(p, CHROMA_RGB) > 0.22)
            .collect(),
        BgMode::Opaque => {
            // Estimate the background as the average border colour.
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for x in 0..w {
                for y in [0, h - 1] {
                    let p = img.get_pixel(x, y);
                    r += p.0[0] as u64;
                    g += p.0[1] as u64;
                    b += p.0[2] as u64;
                    n += 1;
                }
            }
            for y in 0..h {
                for x in [0, w - 1] {
                    let p = img.get_pixel(x, y);
                    r += p.0[0] as u64;
                    g += p.0[1] as u64;
                    b += p.0[2] as u64;
                    n += 1;
                }
            }
            let key = ((r / n) as u8, (g / n) as u8, (b / n) as u8);
            img.pixels().map(|p| colour_dist(p, key) > 0.12).collect()
        }
    }
}

fn col_profile(mask: &[bool], w: u32, h: u32) -> Vec<u32> {
    let mut prof = vec![0u32; w as usize];
    for y in 0..h {
        for x in 0..w {
            if mask[(y * w + x) as usize] {
                prof[x as usize] += 1;
            }
        }
    }
    prof
}

fn row_profile(mask: &[bool], w: u32, h: u32) -> Vec<u32> {
    let mut prof = vec![0u32; h as usize];
    for y in 0..h {
        for x in 0..w {
            if mask[(y * w + x) as usize] {
                prof[y as usize] += 1;
            }
        }
    }
    prof
}

/// Which grid did the model ACTUALLY draw? For every plausible shape holding `n`
/// cells (cols 1..=n, rows = ceil(n/cols), reading order preserved), lay its seams
/// with the same sliding search the cut uses and score how much ink they cross,
/// normalised per seam. The drawn grid's gutters are empty → near-zero score; a wrong
/// shape slices through symbols → high score. Ties (e.g. an empty sheet) go to the
/// requested shape.
fn detect_grid(
    mask: &[bool],
    w: u32,
    h: u32,
    n: usize,
    req_cols: u32,
    req_rows: u32,
) -> (u32, u32) {
    let colp = col_profile(mask, w, h);
    let rowp = row_profile(mask, w, h);
    let peak_c = *colp.iter().max().unwrap_or(&1) as f64;
    let peak_r = *rowp.iter().max().unwrap_or(&1) as f64;
    // Score at the EXACT nominal boundaries (±2 px): a grid the model actually drew
    // has its gutters AT its own boundaries. Do NOT reuse the wide sliding window
    // here — with ±¼-cell reach even a wrong grid finds some gutter to hide in.
    let score = |prof: &[u32], peak: f64, extent: u32, div: u32| -> f64 {
        if div <= 1 || peak <= 0.0 {
            return 0.0;
        }
        (1..div)
            .map(|d| {
                let nominal = (d * extent / div) as usize;
                let lo = nominal.saturating_sub(2);
                let hi = (nominal + 2).min(prof.len() - 1);
                *prof[lo..=hi].iter().min().unwrap_or(&0) as f64 / peak
            })
            .sum::<f64>()
            / (div - 1) as f64
    };

    let mut best = (req_cols, req_rows);
    let mut best_score = f64::INFINITY;
    for cols in 1..=(n as u32) {
        let rows = (n as u32).div_ceil(cols);
        let mut s = score(&colp, peak_c, w, cols) + score(&rowp, peak_r, h, rows);
        if (cols, rows) == (req_cols, req_rows) {
            s -= 1e-9; // tie-break toward the shape we asked for
        }
        if s < best_score {
            best_score = s;
            best = (cols, rows);
        }
    }
    best
}

/// The reason the cut survives "any condition": instead of trusting the model to
/// respect exact cell boundaries, each internal boundary slides within ±⅙ cell of its
/// nominal position to the spot with the LEAST subject coverage — the gutter the model
/// actually drew. Ties go to the nominal line. Windows never overlap, so seams stay
/// ordered.
fn best_seams(profile: &[u32], extent: u32, divisions: u32) -> Vec<u32> {
    (1..divisions)
        .map(|d| {
            let nominal = d * extent / divisions;
            // ±¼ cell: wide enough for real drift (single-row sheets drift the most),
            // and adjacent windows still can't overlap (½ cell stays between them).
            let win = (extent / divisions / 4).max(2);
            let lo = nominal.saturating_sub(win).max(1);
            let hi = (nominal + win).min(extent - 1);
            (lo..=hi)
                .min_by_key(|&x| {
                    let d = (x as i64 - nominal as i64).unsigned_abs();
                    (profile[x as usize], d)
                })
                .unwrap_or(nominal)
        })
        .collect()
}

/// Slice the first `n` cells out of the sheet along the chosen seam lines.
fn split_at_seams(
    img: &image::RgbaImage,
    seams_x: &[u32],
    seams_y: &[u32],
    n: usize,
) -> Vec<image::RgbaImage> {
    let (w, h) = img.dimensions();
    let xs: Vec<u32> = std::iter::once(0).chain(seams_x.iter().copied()).chain([w]).collect();
    let ys: Vec<u32> = std::iter::once(0).chain(seams_y.iter().copied()).chain([h]).collect();
    let cols = xs.len() - 1;
    (0..n)
        .map(|i| {
            let (r, c) = (i / cols, i % cols);
            image::imageops::crop_imm(img, xs[c], ys[r], xs[c + 1] - xs[c], ys[r + 1] - ys[r])
                .to_image()
        })
        .collect()
}

/// Upscale a sheet cell to `target_long` (Real-ESRGAN when installed, Lanczos
/// otherwise). A cell is only ~1/cols of the canvas, so this restores the working
/// resolution the rest of the pipeline expects.
fn upscale_cell(
    cell: image::RgbaImage,
    target_long: u32,
    idx: usize,
    pre: &crate::processing::Preflight,
) -> image::RgbaImage {
    if cell.width().max(cell.height()) >= target_long {
        return cell;
    }
    if let Some(bin) = &pre.upscale_bin {
        let tmp = std::env::temp_dir().join(format!("wf_setcell_{}_{idx}.png", std::process::id()));
        let write_ok = image::DynamicImage::ImageRgba8(cell.clone())
            .save_with_format(&tmp, image::ImageFormat::Png)
            .is_ok();
        if write_ok {
            let up = run_realesrgan(bin, &tmp)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok());
            let _ = std::fs::remove_file(&tmp);
            if let Some(up) = up {
                return fit_long(up, target_long).to_rgba8();
            }
        }
    }
    fit_long(image::DynamicImage::ImageRgba8(cell), target_long).to_rgba8()
}

/// Import an existing image file as a new (active) variation — the "bring your own source"
/// entry into the pipeline. Normalises to PNG and detects transparency. No prompt/generation.
#[tauri::command]
#[specta::specta]
pub fn import_source_image(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    path: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read image: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("not a readable image: {e}"))?;

    let rgba = img.to_rgba8();
    let has_alpha = img.color().has_alpha() && rgba.pixels().any(|p| p.0[3] < 250);
    // A magenta (#FF00FF) chroma field around the subject → let post-processing key it out.
    let chroma_bg = !has_alpha && is_chroma_background(&rgba);

    // Normalise to PNG so the raw file is always raw.png.
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    let png = png.into_inner();

    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(&asset_key));
    let var_id = next_variation_id(&record);
    let raw_file =
        storage::write_variation_file(&base, &game_id, &asset_key, &var_id, "raw.png", &png)?;

    let variation = Variation {
        id: var_id.clone(),
        parent: record.active_variation.clone(),
        created_at: now_ms(),
        prompt_snapshot: String::new(),
        provider: "import".into(),
        model: None,
        seed: None,
        raw_file,
        status: VariationStatus::Ready,
        background: if has_alpha {
            providers::BgMode::Native
        } else if chroma_bg {
            providers::BgMode::Chroma
        } else {
            providers::BgMode::Opaque
        },
        stages: Vec::new(),
        mass_report: None,
            tone_report: None,
            audio_report: None,
        locked: false,
    };
    record.variations.push(variation);
    record.active_variation = Some(var_id);
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Manually erase areas of a take to transparency: the frontend paints an erase mask
/// (white = remove) over the take's best image; the result lands as a NEW active
/// variation with native alpha. The manual fallback for cut-outs and stray backdrop
/// remnants the automatic keying missed.
#[tauri::command]
#[specta::specta]
pub async fn alpha_erase(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    mask_b64: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let src = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;

    // Operate on what the user SAW: the processed transparent final when present,
    // else the raw generation.
    let bytes = src
        .stages
        .iter()
        .find(|s| s.name == "png")
        .and_then(|s| {
            let rel = format!("variations/{}/{}", src.id, s.file);
            storage::read_asset_file(&base, &game_id, &asset_key, &rel).ok()
        })
        .or_else(|| {
            std::fs::read(storage::asset_dir(&base, &game_id, &asset_key).join(&src.raw_file)).ok()
        })
        .ok_or_else(|| "could not read the take's image".to_string())?;

    let mask_bytes = base64::engine::general_purpose::STANDARD
        .decode(mask_b64.trim())
        .map_err(|e| format!("bad mask data: {e}"))?;

    let png = tauri::async_runtime::spawn_blocking(move || {
        let mut img = image::load_from_memory(&bytes)
            .map_err(|e| format!("decode image: {e}"))?
            .to_rgba8();
        let mask = image::load_from_memory(&mask_bytes)
            .map_err(|e| format!("decode mask: {e}"))?
            .to_rgba8();
        // The mask canvas is painted at display resolution — rescale to the image.
        let mask = if mask.dimensions() != img.dimensions() {
            image::imageops::resize(
                &mask,
                img.width(),
                img.height(),
                image::imageops::FilterType::Triangle,
            )
        } else {
            mask
        };
        for (p, m) in img.pixels_mut().zip(mask.pixels()) {
            // Painted strength = the mask's own alpha (white brush on transparent canvas).
            let erase = m.0[3] as u32;
            p.0[3] = (p.0[3] as u32 * (255 - erase) / 255) as u8;
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode png: {e}"))?;
        Ok::<_, String>(buf.into_inner())
    })
    .await
    .map_err(|e| format!("erase task failed: {e}"))??;

    let new_id = next_variation_id(&record);
    let raw_file =
        storage::write_variation_file(&base, &game_id, &asset_key, &new_id, "raw.png", &png)?;
    record.variations.push(Variation {
        id: new_id.clone(),
        parent: Some(variation_id),
        created_at: now_ms(),
        prompt_snapshot: "alpha erase (manual)".into(),
        provider: "alpha_edit".into(),
        model: None,
        seed: None,
        raw_file,
        status: VariationStatus::Ready,
        background: providers::BgMode::Native,
        stages: Vec::new(),
        mass_report: None,
            tone_report: None,
            audio_report: None,
        locked: false,
    });
    record.active_variation = Some(new_id);
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Resize an image preserving aspect so its longest side becomes `target`.
fn fit_long(img: image::DynamicImage, target: u32) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let long = w.max(h).max(1);
    if long == target {
        return img;
    }
    let s = target as f64 / long as f64;
    img.resize_exact(
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
        image::imageops::FilterType::Lanczos3,
    )
}

/// Run Real-ESRGAN (`realesrgan-ncnn-vulkan -i in -o out`) and return the upscaled PNG bytes.
/// Passes `-m <models>` resolved next to the (real) binary so it works from any working dir.
fn run_realesrgan(bin: &str, input: &std::path::Path) -> Result<Vec<u8>, String> {
    let out = std::env::temp_dir().join(format!("wf_upscale_{}.png", now_ms() as u64));
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["-i", &input.to_string_lossy(), "-o", &out.to_string_lossy()]);
    if let Ok(real) = std::fs::canonicalize(bin) {
        if let Some(models) = real.parent().map(|d| d.join("models")) {
            if models.is_dir() {
                cmd.args(["-m", &models.to_string_lossy(), "-n", "realesrgan-x4plus"]);
            }
        }
    }
    let res = cmd.output().map_err(|e| format!("realesrgan launch: {e}"))?;
    if !res.status.success() {
        return Err(format!("realesrgan: {}", String::from_utf8_lossy(&res.stderr)));
    }
    let bytes = std::fs::read(&out).map_err(|e| format!("read upscaled: {e}"))?;
    let _ = std::fs::remove_file(&out);
    Ok(bytes)
}

/// True when the image's border ring is mostly magenta (#FF00FF-ish: high R & B, low G) —
/// i.e. the subject sits on a chroma-key field the post-processor can remove.
fn is_chroma_background(rgba: &image::RgbaImage) -> bool {
    let (w, h) = rgba.dimensions();
    if w < 8 || h < 8 {
        return false;
    }
    let step = (w.max(h) / 200).max(1);
    let (mut magenta, mut total) = (0u32, 0u32);
    let mut check = |x: u32, y: u32| {
        let p = rgba.get_pixel(x, y).0;
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        total += 1;
        if r > 150 && b > 150 && g < 90 && (r - g) > 90 && (b - g) > 90 {
            magenta += 1;
        }
    };
    let mut x = 0;
    while x < w {
        check(x, 0);
        check(x, h - 1);
        x += step;
    }
    let mut y = 0;
    while y < h {
        check(0, y);
        check(w - 1, y);
        y += step;
    }
    total > 0 && magenta as f32 / total as f32 > 0.6
}

/// Faithful high-res recreation prompt for the gpt-image-2 upscale path.
const UPSCALE_PROMPT: &str = "Recreate this exact image at high resolution. Keep the SAME subject \
with identical shapes, proportions, colours, textures, linework, style and composition — only \
sharper, cleaner and more finely detailed. Do NOT redraw, restyle, recolour, move, add or remove \
anything. Keep the exact same flat background colour edge to edge (if it is magenta #FF00FF, keep \
it that magenta). Output only the upscaled image, same framing.";

/// Targeted revision of a variation via gpt-image-2 edits: apply ONE described change,
/// keep everything else. Saved as a NEW active variation with lineage (reroll stays
/// available — this is sculpting, not gambling). Transparent/native raws are flattened
/// onto magenta first so the result always rides the chroma post-processing path.
#[tauri::command]
#[specta::specta]
pub async fn refine_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    instruction: String,
) -> Result<AssetRecord, String> {
    let instruction = instruction.trim().to_string();
    if instruction.is_empty() {
        return Err("describe the change first".into());
    }
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;

    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let src = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;
    let src_bg = src.background;
    let raw_path = storage::asset_dir(&base, &game_id, &asset_key).join(&src.raw_file);
    let bytes = std::fs::read(&raw_path).map_err(|e| format!("read image: {e}"))?;

    // Normalize the input background: alpha-bearing raws get flattened onto magenta so
    // the edited result is always cleanly keyable afterwards.
    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode image: {e}"))?.to_rgba8();
    let has_alpha = img.pixels().any(|p| p.0[3] < 250);
    let (send_bytes, out_bg, bg_clause) = if src_bg == crate::providers::BgMode::Opaque {
        (bytes, crate::providers::BgMode::Opaque, "Keep the background exactly as it is.")
    } else {
        let flat = if has_alpha {
            let (w, h) = img.dimensions();
            let mut out = image::RgbaImage::new(w, h);
            for (x, y, p) in img.enumerate_pixels() {
                let a = p.0[3] as u32;
                let blend = |c: u8, k: u8| ((c as u32 * a + k as u32 * (255 - a)) / 255) as u8;
                out.put_pixel(x, y, image::Rgba([blend(p.0[0], 255), blend(p.0[1], 0), blend(p.0[2], 255), 255]));
            }
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(out)
                .write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| format!("flatten failed: {e}"))?;
            buf.into_inner()
        } else {
            bytes
        };
        (
            flat,
            crate::providers::BgMode::Chroma,
            "Keep the background a solid, perfectly flat, uniform magenta (#ff00ff) exactly as it is — no gradient, no texture.",
        )
    };

    let prompt = format!(
        "Edit the attached image. Apply ONLY this change: {instruction}. Keep the subject's \
identity, composition, pose, art style, palette, lighting and every detail NOT mentioned \
above EXACTLY as in the original — this is a small targeted revision, not a redraw. {bg_clause}"
    );

    // Target the ASSET's authored aspect, not the source take's: the size parameter
    // always beats prompt text, so "make it 16:9" typed into the instruction can never
    // work — instead a refine of a wrong-aspect take silently corrects the canvas.
    let dims = image::load_from_memory(&send_bytes).map_err(|e| format!("decode: {e}"))?;
    let project = storage::read_project(&base, &game_id)?;
    let descriptor = find_descriptor(&project.config, &asset_key)?;
    let (tw, th) = match (descriptor.author_w, descriptor.author_h) {
        (Some(w), Some(h)) => (w, h),
        _ => (dims.width(), dims.height()),
    };
    let mut prompt = prompt;
    let src_aspect = dims.width() as f64 / dims.height().max(1) as f64;
    let dst_aspect = tw as f64 / th.max(1) as f64;
    if (src_aspect / dst_aspect - 1.0).abs() > 0.02 {
        prompt.push_str(
            " The output canvas has a DIFFERENT aspect ratio than the input: naturally \
extend the scene to fill the new canvas edge to edge — continue the existing artwork \
outward, no borders, no letterboxing, no empty bands.",
        );
    }
    let size = crate::providers::openai_image::edit_size(tw, th);
    let out = crate::providers::openai_image::edit_image(&key, &send_bytes, &prompt, &size, None)
        .await?;

    let new_id = next_variation_id(&record);
    let raw_file =
        storage::write_variation_file(&base, &game_id, &asset_key, &new_id, "raw.png", &out)?;
    record.variations.push(Variation {
        id: new_id.clone(),
        parent: Some(variation_id),
        created_at: now_ms(),
        prompt_snapshot: format!("refine: {instruction}"),
        provider: "refine".into(),
        model: None,
        seed: None,
        raw_file,
        status: VariationStatus::Ready,
        background: out_bg,
        stages: Vec::new(),
        mass_report: None,
            tone_report: None,
            audio_report: None,
        locked: false,
    });
    record.active_variation = Some(new_id);
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Upscale a variation to a NEW active variation (original kept). `method`:
/// - `"api"` → gpt-image-2 faithful high-res recreation (uses the OpenAI key).
/// - anything else → local free: Real-ESRGAN if installed, otherwise Lanczos, to `target_long`.
/// Preserves the source's background mode so a later Process still keys/handles it correctly.
#[tauri::command]
#[specta::specta]
pub async fn upscale_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    target_long: u32,
    method: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let src = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;
    let bg = src.background;
    let raw_path = storage::asset_dir(&base, &game_id, &asset_key).join(&src.raw_file);
    let bytes = std::fs::read(&raw_path).map_err(|e| format!("read image: {e}"))?;

    let (png_bytes, nw, nh, label) = if method == "api" || method == "openai" {
        // gpt-image-2 edit: recreate faithfully at a larger native size.
        let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?
            .filter(|k| !k.is_empty())
            .ok_or_else(|| "OpenAI API key is not set (add it in Settings).".to_string())?;
        let dims = image::load_from_memory(&bytes).map_err(|e| format!("decode image: {e}"))?;
        let size = crate::providers::openai_image::edit_size(dims.width(), dims.height());
        let out =
            crate::providers::openai_image::edit_image(&key, &bytes, UPSCALE_PROMPT, &size, None)
                .await?;
        let up = image::load_from_memory(&out).map_err(|e| format!("decode upscaled: {e}"))?;
        (out, up.width(), up.height(), "gpt-image-2".to_string())
    } else {
        let img = image::load_from_memory(&bytes).map_err(|e| format!("decode image: {e}"))?;
        let long = img.width().max(img.height()).max(1);
        let target = target_long.clamp(long, 4096); // never downscale
        let (upimg, m) = match crate::processing::detect().upscale_bin {
            Some(bin) => match run_realesrgan(&bin, &raw_path) {
                Ok(ai_bytes) => match image::load_from_memory(&ai_bytes) {
                    Ok(ai) => (fit_long(ai, target), "Real-ESRGAN"),
                    Err(_) => (fit_long(img, target), "Lanczos"),
                },
                Err(_) => (fit_long(img, target), "Lanczos"),
            },
            None => (fit_long(img, target), "Lanczos"),
        };
        let (w, h) = (upimg.width(), upimg.height());
        let mut png = std::io::Cursor::new(Vec::new());
        upimg
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| format!("encode png: {e}"))?;
        (png.into_inner(), w, h, m.to_string())
    };

    let new_id = next_variation_id(&record);
    let raw_file =
        storage::write_variation_file(&base, &game_id, &asset_key, &new_id, "raw.png", &png_bytes)?;
    record.variations.push(Variation {
        id: new_id.clone(),
        parent: Some(variation_id),
        created_at: now_ms(),
        prompt_snapshot: format!("upscaled to {nw}×{nh} ({label})"),
        provider: "upscale".into(),
        model: None,
        seed: None,
        raw_file,
        status: VariationStatus::Ready,
        background: bg,
        stages: Vec::new(),
        mass_report: None,
            tone_report: None,
            audio_report: None,
        locked: false,
    });
    record.active_variation = Some(new_id);
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Mark a variation as the active one for an asset.
#[tauri::command]
#[specta::specta]
pub fn set_active_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    if !record.variations.iter().any(|v| v.id == variation_id) {
        return Err(format!("variation \"{variation_id}\" not found"));
    }
    record.active_variation = Some(variation_id);
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Delete one variation from an asset's history: removes it from the record and deletes its
/// on-disk files. If it was the active one, the most recent remaining variation becomes active
/// (or none, if the history is now empty).
#[tauri::command]
#[specta::specta]
pub fn delete_variation(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let idx = record
        .variations
        .iter()
        .position(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;

    if record.variations[idx].locked {
        return Err("This variation is locked. Unlock it before deleting.".into());
    }

    record.variations.remove(idx);

    // Reassign active if we just removed it — prefer the last (most recent) remaining.
    if record.active_variation.as_deref() == Some(variation_id.as_str()) {
        record.active_variation = record.variations.last().map(|v| v.id.clone());
    }

    // Best-effort delete of the variation's files; a failure here shouldn't block the record
    // update (the folder is orphaned at worst).
    let dir = storage::variation_dir(&base, &game_id, &asset_key, &variation_id);
    let _ = std::fs::remove_dir_all(&dir);

    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Mark an asset as template-only (e.g. an `l_base` empty plate other symbols compose
/// onto). Template assets are excluded from Export and Publish — they never ship.
#[tauri::command]
#[specta::specta]
pub fn set_template_only(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    template_only: bool,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .unwrap_or_else(|| AssetRecord::new(&asset_key));
    record.template_only = template_only;
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Lock or unlock a variation. Locked variations are protected from `delete_variation`.
#[tauri::command]
#[specta::specta]
pub fn set_variation_locked(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
    locked: bool,
) -> Result<AssetRecord, String> {
    let base = projects_root(&app)?;
    let mut record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let var = record
        .variations
        .iter_mut()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?;
    var.locked = locked;
    storage::write_asset_record(&base, &game_id, &record)?;
    Ok(record)
}

/// Read a variation's raw image as a `data:image/png;base64,…` URL for display.
#[tauri::command]
#[specta::specta]
pub fn get_variation_image(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<String, String> {
    let base = projects_root(&app)?;
    let record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record".to_string())?;
    let variation = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| "variation not found".to_string())?;
    let bytes = storage::read_asset_file(&base, &game_id, &asset_key, &variation.raw_file)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:image/png;base64,{b64}"))
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Backgrounds and splash art are opaque; everything else wants a transparent bg.
fn needs_alpha(kind: AssetKind) -> bool {
    !matches!(kind, AssetKind::Background | AssetKind::Splash)
}

/// Generation size for an asset (capped, SD-friendly). Defaults to a square if the
/// descriptor has no dims.
fn gen_dims(descriptor: &AssetDescriptor) -> (u32, u32) {
    let w = descriptor.author_w.unwrap_or(1024);
    let h = descriptor.author_h.unwrap_or(1024);
    providers::generation_size(w, h, 1024)
}

pub(crate) fn next_variation_id(record: &AssetRecord) -> String {
    let max = record
        .variations
        .iter()
        .filter_map(|v| v.id.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    format!("v{:03}", max + 1)
}

fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chroma sheet whose symbols have DRIFTED right of their nominal cells — the
    /// exact-grid cut would slice through a symbol; the seam search must find the
    /// gutters the model actually drew.
    #[test]
    fn seams_follow_the_drawn_gutters_not_the_nominal_grid() {
        let (w, h) = (600u32, 200u32);
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        // Three symbols on one row, each shifted +40px right of its cell centre — the
        // symbol in cell 1 now straddles the nominal x=200 boundary.
        for (i, cx) in [140u32, 340, 540].iter().enumerate() {
            for y in 40..160 {
                for x in cx.saturating_sub(70)..(cx + 70).min(w) {
                    img.put_pixel(x, y, image::Rgba([200, 40 * i as u8, 60, 255]));
                }
            }
        }
        let mask = content_mask(&img, crate::providers::BgMode::Chroma);
        let seams = best_seams(&col_profile(&mask, w, h), w, 3);
        // Gutters are 210..270 and 410..470; nominal 200 sits INSIDE symbol 1.
        assert!(
            (210..=270).contains(&seams[0]),
            "seam 0 must land in the drawn gutter, got {}",
            seams[0]
        );
        assert!((410..=470).contains(&seams[1]), "seam 1 in gutter, got {}", seams[1]);

        // The cut cells each contain exactly their own symbol, whole.
        let cells = split_at_seams(&img, &seams, &[], 3);
        assert_eq!(cells.len(), 3);
        // Symbol 2 (cx 540, half-width 70) is clipped by the sheet's right edge → 130 wide.
        for (i, (cell, expect_w)) in cells.iter().zip([140usize, 140, 130]).enumerate() {
            let cmask = content_mask(cell, crate::providers::BgMode::Chroma);
            let ink = cmask.iter().filter(|&&b| b).count();
            assert_eq!(ink, expect_w * 120, "cell {i} must hold its full symbol, got {ink} px");
        }
    }

    /// A perfectly-centred sheet: seams stay at the nominal grid (ties resolve toward
    /// nominal), and the cells tile the sheet exactly even at odd sizes.
    #[test]
    fn seams_stay_nominal_on_a_clean_sheet_and_cells_tile_exactly() {
        let (w, h) = (1023u32, 767u32);
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        let mask = content_mask(&img, crate::providers::BgMode::Chroma);
        let sx = best_seams(&col_profile(&mask, w, h), w, 4);
        let sy = best_seams(&row_profile(&mask, w, h), h, 3);
        assert_eq!(sx, vec![1 * w / 4, 2 * w / 4, 3 * w / 4]);
        assert_eq!(sy, vec![1 * h / 3, 2 * h / 3]);
        let cells = split_at_seams(&img, &sx, &sy, 12);
        let area: u32 = cells.iter().map(|c| c.width() * c.height()).sum();
        assert_eq!(area, w * h);
    }

    /// The user's real case: 10 symbols REQUESTED as 4×3, but the model drew 5×2.
    /// detect_grid must read the drawn layout off the image, and every cell must come
    /// out holding exactly one whole symbol.
    #[test]
    fn detect_grid_reads_the_drawn_layout_not_the_requested_one() {
        let (w, h) = (1000u32, 400u32);
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        // 5 columns × 2 rows of 120×120 symbols centred in 200×200 cells.
        for r in 0..2u32 {
            for c in 0..5u32 {
                for y in (r * 200 + 40)..(r * 200 + 160) {
                    for x in (c * 200 + 40)..(c * 200 + 160) {
                        img.put_pixel(x, y, image::Rgba([200, 30, 60, 255]));
                    }
                }
            }
        }
        let mask = content_mask(&img, crate::providers::BgMode::Chroma);
        let (cols, rows) = detect_grid(&mask, w, h, 10, 4, 3); // requested 4×3
        assert_eq!((cols, rows), (5, 2), "must trust the image over the request");

        let sx = best_seams(&col_profile(&mask, w, h), w, cols);
        let sy = best_seams(&row_profile(&mask, w, h), h, rows);
        let cells = split_at_seams(&img, &sx, &sy, 10);
        assert_eq!(cells.len(), 10);
        for (i, cell) in cells.iter().enumerate() {
            let cmask = content_mask(cell, crate::providers::BgMode::Chroma);
            let ink = cmask.iter().filter(|&&b| b).count();
            assert_eq!(ink, 120 * 120, "cell {i} must hold one whole symbol, got {ink} px");
        }
    }

    /// The single-row complaint: 4 symbols requested as 2×2 but drawn in ONE row,
    /// with drift on top. Grid detection must pick 4×1 and the (now ±¼-cell) seam
    /// windows must reach the drifted gutters.
    #[test]
    fn single_row_sheets_detect_and_cut_whole_symbols() {
        let (w, h) = (1200u32, 300u32);
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        // 4 symbols in one row, 160 wide, each drifted +50px right of its cell centre.
        for (i, cx) in [200u32, 500, 800, 1100].iter().enumerate() {
            for y in 60..240 {
                for x in cx.saturating_sub(80)..(cx + 80).min(w) {
                    img.put_pixel(x, y, image::Rgba([40 * i as u8 + 30, 90, 180, 255]));
                }
            }
        }
        let mask = content_mask(&img, crate::providers::BgMode::Chroma);
        let (cols, rows) = detect_grid(&mask, w, h, 4, 2, 2); // requested 2×2
        assert_eq!((cols, rows), (4, 1), "one drawn row must beat the requested 2×2");

        let sx = best_seams(&col_profile(&mask, w, h), w, cols);
        let cells = split_at_seams(&img, &sx, &[], 4);
        for (i, (cell, expect_w)) in cells.iter().zip([160usize, 160, 160, 160]).enumerate() {
            let cmask = content_mask(cell, crate::providers::BgMode::Chroma);
            let ink = cmask.iter().filter(|&&b| b).count();
            assert_eq!(ink, expect_w * 180, "cell {i} must hold one whole symbol, got {ink} px");
        }
    }

    /// A sheet that FOLLOWS the requested 4×3 grid (with 2 empty cells) must keep it —
    /// grid detection only overrides when the image clearly disagrees.
    #[test]
    fn detect_grid_keeps_the_requested_shape_when_the_image_matches() {
        let (w, h) = (800u32, 600u32);
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        // 10 symbols in reading order on a 4×3 grid of 200×200 cells; cells 11,12 empty.
        for i in 0..10u32 {
            let (r, c) = (i / 4, i % 4);
            for y in (r * 200 + 40)..(r * 200 + 160) {
                for x in (c * 200 + 40)..(c * 200 + 160) {
                    img.put_pixel(x, y, image::Rgba([60, 90, 200, 255]));
                }
            }
        }
        let mask = content_mask(&img, crate::providers::BgMode::Chroma);
        assert_eq!(detect_grid(&mask, w, h, 10, 4, 3), (4, 3));
    }

    /// Opaque sheets (no chroma, no alpha): the border-estimated background still
    /// separates subject from field, so seams work for every provider mode.
    #[test]
    fn content_mask_handles_opaque_sheets() {
        let mut img = image::RgbaImage::from_pixel(100, 50, image::Rgba([235, 230, 220, 255]));
        for y in 10..40 {
            for x in 30..70 {
                img.put_pixel(x, y, image::Rgba([40, 60, 120, 255]));
            }
        }
        let mask = content_mask(&img, crate::providers::BgMode::Opaque);
        assert!(mask[(25 * 100 + 50) as usize], "subject pixel detected");
        assert!(!mask[(5 * 100 + 5) as usize], "background pixel ignored");
    }

    #[test]
    fn guide_layout_renders_zones_dark_on_light() {
        let zones = vec![crate::model::game_config::GuideZone {
            label: "hud bottom".into(),
            x: 0.0,
            y: 80.0,
            w: 100.0,
            h: 20.0,
        }];
        let png = render_guide_layout(&zones, 2048, 1152).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        // Long side scaled to ≤1024, aspect preserved.
        assert_eq!(img.width(), 1024);
        assert_eq!(img.height(), 576);
        // Inside the bottom band: dark. Middle of the canvas: light.
        assert_eq!(img.get_pixel(512, 550).0[0], 72);
        assert_eq!(img.get_pixel(512, 300).0[0], 232);
    }

    #[test]
    fn anchor_rides_first_and_total_stays_capped() {
        let base = std::env::temp_dir().join(format!("wf_anchor_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let gid = "g";

        // No anchor → references untouched.
        let mut refs: Vec<Vec<u8>> = vec![vec![1], vec![2]];
        prepend_anchor(&base, gid, &mut refs);
        assert_eq!(refs.len(), 2);

        // Write a real (tiny) PNG anchor.
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let path = anchor_file(&base, gid);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png.into_inner()).unwrap();

        // Anchor goes first; four manual refs get trimmed to keep the total at the cap.
        let mut refs: Vec<Vec<u8>> = vec![vec![1], vec![2], vec![3], vec![4]];
        prepend_anchor(&base, gid, &mut refs);
        assert_eq!(refs.len(), MAX_REFERENCES);
        assert!(refs[0].starts_with(&[0x89, b'P', b'N', b'G']), "anchor PNG first");
        assert_eq!(refs[1], vec![1]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
