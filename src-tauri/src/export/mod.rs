//! Stake-format export: assemble processed finals into the ASSETS.md §16 dist tree and
//! emit the §17 `assets.ts` manifest snippet. Also runs the §18 readiness check.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::asset::{AssetDescriptor, Format, Production};
use crate::{storage, taxonomy};

/// Result of an export run, surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    /// Absolute path of the dist folder that was (re)built.
    pub dist_path: String,
    /// Asset keys written to dist.
    pub written: Vec<String>,
    /// Required raster assets that have no processed active variation yet.
    pub missing: Vec<String>,
    /// Procedural / runtime / manual-tool assets handled outside this pipeline.
    pub waived: Vec<String>,
    /// The `assets.ts` manifest fragment (§17).
    pub manifest_snippet: String,
}

/// Preload set (§17) — assets needed before first interaction.
fn is_preload(key: &str) -> bool {
    key.starts_with("bg_base_") || key.starts_with("splash_hero_")
}

/// Build the dist tree for a game and return a report. Rebuilds dist from scratch.
pub fn build_dist(base: &Path, game_id: &str) -> Result<ExportReport, String> {
    let project = storage::read_project(base, game_id)?;
    let assets = taxonomy::derive_assets(&project.config);

    // dist/<game_id>/ holds the §16 category dirs (so `cp -r dist/<id>/* static/assets/`).
    let dist_root = storage::project_dir(base, game_id)
        .join("dist")
        .join(game_id);
    let _ = fs::remove_dir_all(&dist_root);
    fs::create_dir_all(&dist_root).map_err(|e| format!("create dist failed: {e}"))?;

    let mut written = Vec::new();
    let mut missing = Vec::new();
    let mut waived = Vec::new();
    let mut manifest_entries: Vec<String> = Vec::new();

    for asset in &assets {
        if asset.production != Production::Raster {
            waived.push(asset.key.clone());
            continue;
        }
        // Template-only assets (e.g. an l_base plate other symbols compose onto) never ship.
        if let Some(record) = storage::read_asset_record(base, game_id, &asset.key)? {
            if record.template_only {
                waived.push(asset.key.clone());
                continue;
            }
        }
        // Fx scene assets ship their generated AI sheet as a PIXI spritesheet atlas.
        if project
            .config
            .scene
            .def_for_key(&asset.key)
            .is_some_and(|d| d.kind == crate::model::game_config::SceneKind::Fx)
        {
            if let Some(entry) = export_fx_sheet(
                base,
                game_id,
                asset,
                &dist_root,
                project.config.scene.webp_quality as f32,
            )? {
                written.push(asset.key.clone());
                manifest_entries.push(entry);
                continue;
            }
        }
        // Precedence: studio Spine export → cut layer plates → flat raster.
        if let Some(entry) = export_spine(base, game_id, asset, &dist_root)? {
            written.push(asset.key.clone());
            manifest_entries.push(entry);
            continue;
        }
        if let Some(entry) = export_layer_plates(base, game_id, asset, &dist_root)? {
            written.push(asset.key.clone());
            manifest_entries.push(entry);
            continue;
        }
        match export_one(base, game_id, asset, &dist_root)? {
            Some(primary_rel) => {
                written.push(asset.key.clone());
                manifest_entries.push(manifest_entry(&asset.key, &primary_rel));
            }
            None => missing.push(asset.key.clone()),
        }
    }

    // Scene placement manifest (scene.json) — the runtime's layer stack. Emitted only
    // when at least one scene asset carries placement metadata.
    if let Some(scene_json) = emit_scene_manifest(&project.config) {
        fs::write(dist_root.join("scene.json"), scene_json)
            .map_err(|e| format!("write scene.json failed: {e}"))?;
    }

    let snippet = render_manifest(&manifest_entries);
    fs::write(dist_root.join("assets.ts.snippet"), &snippet)
        .map_err(|e| format!("write manifest failed: {e}"))?;

    Ok(ExportReport {
        dist_path: dist_root.to_string_lossy().into_owned(),
        written,
        missing,
        waived,
        manifest_snippet: snippet,
    })
}

/// Emit the runtime scene manifest (`scene.json`): one layer entry per scene asset with
/// placement metadata, in config order (z = array order, back → front). Contract owned
/// by the game runtime: normalized 0–1 coords, parallax multiplier, per-orientation
/// `variants.{portrait,tablet}` shallow overrides (incl. the variant's own asset key,
/// since plates ship orientation-specific textures).
fn emit_scene_manifest(config: &crate::model::game_config::GameConfig) -> Option<String> {
    use crate::model::game_config::{SceneConfig, SceneKind, ScenePlacement};

    fn placement_fields(map: &mut serde_json::Map<String, serde_json::Value>, p: &ScenePlacement) {
        if !p.fit.trim().is_empty() {
            map.insert("fit".into(), p.fit.trim().into());
        }
        if let Some(a) = &p.anchor {
            map.insert("anchor".into(), serde_json::json!(a));
        }
        if let Some(v) = &p.pos {
            map.insert("pos".into(), serde_json::json!(v));
        }
        if let Some(v) = p.height {
            map.insert("height".into(), serde_json::json!(v));
        }
        if let Some(v) = p.parallax {
            map.insert("parallax".into(), serde_json::json!(v));
        }
        if let Some(v) = p.overscan {
            map.insert("overscan".into(), serde_json::json!(v));
        }
        if !p.blend.trim().is_empty() {
            map.insert("blend".into(), p.blend.trim().into());
        }
        if let Some(v) = p.fps {
            map.insert("fps".into(), serde_json::json!(v));
        }
        if let Some(v) = p.looped {
            map.insert("loop".into(), serde_json::json!(v));
        }
    }

    let mut layers = Vec::new();
    for def in &config.scene.assets {
        let Some(placement) = &def.placement else { continue };
        let base_key = SceneConfig::prefixed_key(def);
        // Base entry = the first variant's derived asset (or the bare key).
        let base_variant = def.variants.first();
        let base_asset = match base_variant {
            Some(v) if !v.key.trim().is_empty() => format!("{base_key}_{}", v.key.trim()),
            _ => base_key.clone(),
        };
        let kind = match def.kind {
            SceneKind::Fx => "fx",
            SceneKind::Particle => "particle",
            _ => "layer",
        };
        let default_fit = match def.kind {
            SceneKind::Plate | SceneKind::Layer => "cover",
            _ => "anchored",
        };

        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), def.key.clone().into());
        entry.insert("asset".into(), base_asset.into());
        entry.insert("kind".into(), kind.into());
        entry.insert("fit".into(), default_fit.into());
        placement_fields(&mut entry, placement);
        if def.wrap {
            entry.insert("wrap".into(), serde_json::json!(true));
        }

        // Orientation overrides: variants named portrait/tablet shallow-override the
        // base fields and carry their own texture key.
        let mut overrides = serde_json::Map::new();
        for v in &def.variants {
            let vkey = v.key.trim();
            if vkey != "portrait" && vkey != "tablet" {
                continue;
            }
            let mut o = serde_json::Map::new();
            o.insert("asset".into(), format!("{base_key}_{vkey}").into());
            if let Some(p) = &v.placement {
                placement_fields(&mut o, p);
            }
            overrides.insert(vkey.into(), o.into());
        }
        if !overrides.is_empty() {
            entry.insert("variants".into(), overrides.into());
        }
        layers.push(serde_json::Value::Object(entry));
    }
    if layers.is_empty() {
        return None;
    }
    serde_json::to_string_pretty(&serde_json::json!({ "schema": 1, "layers": layers })).ok()
}

/// If an fx scene asset has a generated AI sheet, ship it as a PIXI spritesheet:
/// `<key>.sheet.webp` + `<key>.sheet.json` (standard "frames"+"meta", frame_000… in
/// play order, one clip per atlas; fps/loop live in scene.json, NOT in meta).
fn export_fx_sheet(
    base: &Path,
    game_id: &str,
    asset: &AssetDescriptor,
    dist_root: &Path,
    webp_quality: f32,
) -> Result<Option<String>, String> {
    let sheet_dir = storage::asset_dir(base, game_id, &asset.key).join("ai_sheet");
    let img_path = sheet_dir.join("sheet.png");
    let meta_path = sheet_dir.join("sheet.json");
    if !img_path.is_file() || !meta_path.is_file() {
        return Ok(None);
    }
    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&meta_path).map_err(|e| format!("read sheet meta: {e}"))?,
    )
    .map_err(|e| format!("parse sheet meta: {e}"))?;
    let frames = meta.get("frames").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if frames < 2 {
        return Ok(None);
    }
    let img = image::open(&img_path).map_err(|e| format!("open sheet: {e}"))?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    let fw = (w / frames).max(1);

    let dest_dir = dist_root.join(&asset.category);
    fs::create_dir_all(&dest_dir).map_err(|e| format!("create {} failed: {e}", asset.category))?;
    let image_name = format!("{}.sheet.webp", asset.key);
    let webp = webp::Encoder::from_image(&image::DynamicImage::ImageRgba8(img))
        .map_err(|e| format!("sheet webp encode: {e}"))?
        .encode(webp_quality.clamp(10.0, 100.0))
        .to_vec();
    fs::write(dest_dir.join(&image_name), webp).map_err(|e| format!("write sheet webp: {e}"))?;

    // Standard PIXI Spritesheet JSON; key insertion order = play order.
    let mut frame_map = serde_json::Map::new();
    for i in 0..frames {
        frame_map.insert(
            format!("frame_{i:03}"),
            serde_json::json!({
                "frame": { "x": i * fw, "y": 0, "w": fw, "h": h },
                "rotated": false,
                "trimmed": false,
                "spriteSourceSize": { "x": 0, "y": 0, "w": fw, "h": h },
                "sourceSize": { "w": fw, "h": h },
            }),
        );
    }
    let atlas = serde_json::json!({
        "frames": frame_map,
        "meta": {
            "image": image_name,
            "size": { "w": w, "h": h },
            "scale": "1",
        },
    });
    let json_name = format!("{}.sheet.json", asset.key);
    fs::write(
        dest_dir.join(&json_name),
        serde_json::to_string_pretty(&atlas).map_err(|e| format!("atlas json: {e}"))?,
    )
    .map_err(|e| format!("write atlas: {e}"))?;

    Ok(Some(format!(
        "  {key}: {{ type: 'spriteSheet', src: new URL('../../assets/{category}/{json_name}', import.meta.url).href }},",
        key = asset.key,
        category = asset.category,
    )))
}

/// Copy one asset's processed finals into dist. Returns the primary (webp) path relative
/// to the assets root, or `None` if the asset isn't ready.
fn export_one(
    base: &Path,
    game_id: &str,
    asset: &AssetDescriptor,
    dist_root: &Path,
) -> Result<Option<String>, String> {
    let record = match storage::read_asset_record(base, game_id, &asset.key)? {
        Some(r) => r,
        None => return Ok(None),
    };
    let Some(active_id) = record.active_variation.clone() else {
        return Ok(None);
    };
    let Some(variation) = record.variations.iter().find(|v| v.id == active_id) else {
        return Ok(None);
    };
    // Must be processed (webp stage present).
    if !variation.stages.iter().any(|s| s.name == "webp") {
        return Ok(None);
    }

    let var_dir = storage::variation_dir(base, game_id, &asset.key, &active_id);
    let dest_dir = dist_root.join(&asset.category);
    fs::create_dir_all(&dest_dir).map_err(|e| format!("create {} failed: {e}", asset.category))?;

    let mut primary_rel: Option<String> = None;
    for stage in &variation.stages {
        let ext = match stage.name.as_str() {
            "webp" => "webp",
            "png" => "png",
            "jpg" => "jpg",
            "nineSlice" => "9.json",
            _ => continue,
        };
        let src = var_dir.join(&stage.file);
        let dest_name = format!("{}.{}", asset.key, ext);
        fs::copy(&src, dest_dir.join(&dest_name))
            .map_err(|e| format!("copy {} failed: {e}", stage.file))?;
        if stage.name == "webp" {
            primary_rel = Some(format!("{}/{}", asset.category, dest_name));
        }
    }

    // Sanity: descriptor claims a 9-slice but no json produced → still export, but the
    // caller can notice via the delivery checklist. (We don't hard-fail here.)
    let _ = asset.formats.contains(&Format::NineSliceJson);

    Ok(primary_rel)
}

/// If the Animation Studio has exported a Spine set for this asset, copy it into
/// `dist/spines/<category>/` and return the `type:'spine'` manifest entry.
fn export_spine(
    base: &Path,
    game_id: &str,
    asset: &AssetDescriptor,
    dist_root: &Path,
) -> Result<Option<String>, String> {
    let key = &asset.key;
    let src_dir = crate::studio::store::export_dir(base, game_id, key);
    if !src_dir.join("skeleton.json").is_file() {
        return Ok(None);
    }

    let dest_dir = dist_root.join("spines").join(&asset.category);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("create spines/{} failed: {e}", asset.category))?;

    // skeleton.json ships as <key>.json; atlas + webp pages keep their names (the atlas
    // text references pages by exact filename).
    fs::copy(src_dir.join("skeleton.json"), dest_dir.join(format!("{key}.json")))
        .map_err(|e| format!("copy skeleton.json failed: {e}"))?;
    for entry in fs::read_dir(&src_dir).map_err(|e| format!("read export dir failed: {e}"))? {
        let entry = entry.map_err(|e| format!("read export dir failed: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".atlas") || name.ends_with(".webp") {
            fs::copy(entry.path(), dest_dir.join(&name))
                .map_err(|e| format!("copy {name} failed: {e}"))?;
        }
    }

    let preload = if is_preload(key) { ", preload: true" } else { "" };
    Ok(Some(format!(
        "  {key}: {{ type: 'spine', src: {{ \
atlas: new URL('../../assets/spines/{category}/{key}.atlas', import.meta.url).href, \
skeleton: new URL('../../assets/spines/{category}/{key}.json', import.meta.url).href \
}}{preload} }},",
        category = asset.category,
    )))
}

/// If the Layers view has cut this asset into transparent layer plates, ship each
/// layer as its OWN flat image entry (`<asset>_<layer>`), back → front. The game's
/// code layer owns stacking and motion (scene.json) — the pipeline only cuts pixels.
fn export_layer_plates(
    base: &Path,
    game_id: &str,
    asset: &AssetDescriptor,
    dist_root: &Path,
) -> Result<Option<String>, String> {
    let key = &asset.key;
    let Some(manifest) = crate::layers::export::read_manifest(base, game_id, key)? else {
        return Ok(None);
    };
    let src_dir = crate::layers::store::export_dir(base, game_id, key);

    let dest_dir = dist_root.join("layers").join(&asset.category);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("create layers/{} failed: {e}", asset.category))?;

    let mut entries = Vec::new();
    for layer in &manifest.layers {
        fs::copy(src_dir.join(&layer.file), dest_dir.join(&layer.file))
            .map_err(|e| format!("copy {} failed: {e}", layer.file))?;
        entries.push(format!(
            "  {key}_{lid}: {{ type: 'image', src: new URL('../../assets/layers/{category}/{file}', import.meta.url).href }},",
            lid = layer.id,
            category = asset.category,
            file = layer.file,
        ));
    }
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(entries.join("
")))
}

fn manifest_entry(key: &str, primary_rel: &str) -> String {
    let preload = if is_preload(key) {
        ", preload: true"
    } else {
        ""
    };
    format!(
        "  {key}: {{ type: 'sprite', src: new URL('../../assets/{primary_rel}', import.meta.url).href{preload} }},"
    )
}

fn render_manifest(entries: &[String]) -> String {
    let mut s = String::new();
    s.push_str("// Generated by Wishfell Asset Pipeline — paste into apps/<game>/src/game/assets.ts\n");
    s.push_str("export default {\n");
    for e in entries {
        s.push_str(e);
        s.push('\n');
    }
    s.push_str("} as const;\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::asset_record::{AssetRecord, PromptState, Variation, VariationStatus};
    use crate::model::game_config::{GameConfig, SymbolDef, SymbolRole, WinType};
    use crate::model::project::Project;
    use crate::processing::StageOutput;

    fn config() -> GameConfig {
        GameConfig {
            game_id: "expgame".into(),
            name: "Exp".into(),
            brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
            win_type: WinType::Scatter,
            cols: 6,
            rows: 5,
            symbols: vec![SymbolDef {
                key: "h1".into(),
                role: SymbolRole::High,
                name: "H1".into(),
                description: String::new(), animation: String::new(),
                size_nudge: 1.0,
            }],
            has_feature_background: false,
            has_buy_bonus: false,
            buy_bonus_modes: vec![],
            has_meter: false,
            meter_thresholds: 0,
            has_mystery: false,
            hold_and_spin: false,
            has_mascot: false,
            mascot_description: String::new(),
            symbol_sizing: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
        }
    }

    #[test]
    fn exports_processed_asset_and_reports_missing() {
        let base =
            std::env::temp_dir().join(format!("wf_export_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let game_id = "expgame";

        storage::write_project(&base, &Project::new(config(), 0.0)).unwrap();

        // symbol_h1: one processed variation with a webp stage on disk.
        let var_dir = storage::variation_dir(&base, game_id, "symbol_h1", "v001");
        fs::create_dir_all(var_dir.join("stages")).unwrap();
        fs::write(var_dir.join("stages/final.webp"), b"fake-webp").unwrap();

        let record = AssetRecord {
            key: "symbol_h1".into(),
            prompt: PromptState {
                subject: "x".into(),
                ..Default::default()
            },
            variations: vec![Variation {
                id: "v001".into(),
                parent: None,
                created_at: 0.0,
                prompt_snapshot: "x".into(),
                provider: "openai_image".into(),
                model: None,
                seed: None,
                raw_file: "variations/v001/raw.png".into(),
                status: VariationStatus::Ready,
                background: Default::default(),
                mass_report: None,
                locked: false,
                stages: vec![StageOutput {
                    name: "webp".into(),
                    file: "stages/final.webp".into(),
                }],
            }],
            active_variation: Some("v001".into()),
            template_only: false,
        };
        storage::write_asset_record(&base, game_id, &record).unwrap();

        let report = build_dist(&base, game_id).unwrap();

        // The processed symbol landed in dist under its category with its key as filename.
        assert!(report.written.contains(&"symbol_h1".to_string()));
        let dist_file = Path::new(&report.dist_path).join("symbols/symbol_h1.webp");
        assert!(dist_file.is_file(), "expected {dist_file:?} to exist");

        // Manifest references it with the right relative path.
        assert!(report
            .manifest_snippet
            .contains("../../assets/symbols/symbol_h1.webp"));

        // Backgrounds etc. are required but unprocessed → reported missing.
        assert!(report.missing.contains(&"bg_base_landscape".to_string()));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn layer_plates_take_precedence_over_raster() {
        let base =
            std::env::temp_dir().join(format!("wf_export_pllx_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let game_id = "expgame";

        let mut cfg = config();
        cfg.symbols.clear(); // keep the manifest small; backgrounds derive regardless
        storage::write_project(&base, &Project::new(cfg, 0.0)).unwrap();

        // A processed flat background exists…
        let var_dir = storage::variation_dir(&base, game_id, "bg_base_landscape", "v001");
        fs::create_dir_all(var_dir.join("stages")).unwrap();
        fs::write(var_dir.join("stages/final.webp"), b"fake-webp").unwrap();
        let record = AssetRecord {
            key: "bg_base_landscape".into(),
            prompt: PromptState { subject: "x".into(), ..Default::default() },
            variations: vec![Variation {
                id: "v001".into(),
                parent: None,
                created_at: 0.0,
                prompt_snapshot: "x".into(),
                provider: "openai_image".into(),
                model: None,
                seed: None,
                raw_file: "variations/v001/raw.png".into(),
                status: VariationStatus::Ready,
                background: Default::default(),
                mass_report: None,
                locked: false,
                stages: vec![StageOutput { name: "webp".into(), file: "stages/final.webp".into() }],
            }],
            active_variation: Some("v001".into()),
            template_only: false,
        };
        storage::write_asset_record(&base, game_id, &record).unwrap();

        // …but a cut layer set also exists → the layer plates win.
        let pllx = crate::layers::store::export_dir(&base, game_id, "bg_base_landscape");
        fs::create_dir_all(&pllx).unwrap();
        fs::write(pllx.join("bg_base_landscape.sky.webp"), b"s").unwrap();
        fs::write(pllx.join("bg_base_landscape.fg.webp"), b"f").unwrap();
        let manifest = crate::layers::export::ParallaxManifest {
            width: 1600,
            height: 900,
            layers: vec![
                crate::layers::export::ParallaxLayerEntry {
                    id: "sky".into(),
                    file: "bg_base_landscape.sky.webp".into(),
                    speed: 0.05,
                },
                crate::layers::export::ParallaxLayerEntry {
                    id: "fg".into(),
                    file: "bg_base_landscape.fg.webp".into(),
                    speed: 1.0,
                },
            ],
        };
        fs::write(pllx.join("parallax.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();

        let report = build_dist(&base, game_id).unwrap();

        assert!(report.written.contains(&"bg_base_landscape".to_string()));
        let dist = Path::new(&report.dist_path);
        assert!(dist.join("layers/backgrounds/bg_base_landscape.sky.webp").is_file());
        assert!(dist.join("layers/backgrounds/bg_base_landscape.fg.webp").is_file());
        // Flat raster path not written for this asset.
        assert!(!dist.join("backgrounds/bg_base_landscape.webp").is_file());
        // Each layer ships as its OWN flat entry — the code layer owns stacking/motion.
        assert!(report.manifest_snippet.contains("bg_base_landscape_sky: { type: 'image'"));
        assert!(report.manifest_snippet.contains("bg_base_landscape_fg: { type: 'image'"));
        assert!(!report.manifest_snippet.contains("type: 'parallax'"));
        assert!(!report.manifest_snippet.contains("speed:"));
        assert!(report
            .manifest_snippet
            .contains("../../assets/layers/backgrounds/bg_base_landscape.sky.webp"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn studio_spine_export_takes_precedence_over_raster() {
        let base =
            std::env::temp_dir().join(format!("wf_export_spine_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let game_id = "expgame";

        storage::write_project(&base, &Project::new(config(), 0.0)).unwrap();

        // A processed raster variation exists...
        let var_dir = storage::variation_dir(&base, game_id, "symbol_h1", "v001");
        fs::create_dir_all(var_dir.join("stages")).unwrap();
        fs::write(var_dir.join("stages/final.webp"), b"fake-webp").unwrap();
        let record = AssetRecord {
            key: "symbol_h1".into(),
            prompt: PromptState { subject: "x".into(), ..Default::default() },
            variations: vec![Variation {
                id: "v001".into(),
                parent: None,
                created_at: 0.0,
                prompt_snapshot: "x".into(),
                provider: "openai_image".into(),
                model: None,
                seed: None,
                raw_file: "variations/v001/raw.png".into(),
                status: VariationStatus::Ready,
                background: Default::default(),
                mass_report: None,
                locked: false,
                stages: vec![StageOutput { name: "webp".into(), file: "stages/final.webp".into() }],
            }],
            active_variation: Some("v001".into()),
            template_only: false,
        };
        storage::write_asset_record(&base, game_id, &record).unwrap();

        // ...but a studio Spine export also exists → spine wins.
        let studio_export = crate::studio::store::export_dir(&base, game_id, "symbol_h1");
        fs::create_dir_all(&studio_export).unwrap();
        fs::write(studio_export.join("skeleton.json"), b"{}").unwrap();
        fs::write(studio_export.join("symbol_h1.atlas"), b"a").unwrap();
        fs::write(studio_export.join("symbol_h1.webp"), b"w").unwrap();
        fs::write(studio_export.join("symbol_h1.png"), b"p").unwrap();

        let report = build_dist(&base, game_id).unwrap();

        assert!(report.written.contains(&"symbol_h1".to_string()));
        let dist = Path::new(&report.dist_path);
        assert!(dist.join("spines/symbols/symbol_h1.json").is_file());
        assert!(dist.join("spines/symbols/symbol_h1.atlas").is_file());
        assert!(dist.join("spines/symbols/symbol_h1.webp").is_file());
        // Debug png copies stay out of dist; raster sprite path not written.
        assert!(!dist.join("spines/symbols/symbol_h1.png").is_file());
        assert!(!dist.join("symbols/symbol_h1.webp").is_file());
        assert!(report.manifest_snippet.contains("type: 'spine'"));
        assert!(report
            .manifest_snippet
            .contains("../../assets/spines/symbols/symbol_h1.atlas"));

        let _ = fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod scene_manifest_tests {
    use super::emit_scene_manifest;
    use crate::model::game_config::*;

    #[test]
    fn manifest_emits_contract_shape_with_orientation_overrides() {
        let mut cfg = GameConfig {
            game_id: "g".into(),
            name: "G".into(),
            brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
            win_type: WinType::Lines,
            cols: 5,
            rows: 3,
            symbols: vec![],
            has_feature_background: false,
            has_buy_bonus: false,
            buy_bonus_modes: vec![],
            has_meter: false,
            meter_thresholds: 0,
            has_mystery: false,
            hold_and_spin: false,
            has_mascot: false,
            mascot_description: String::new(),
            symbol_sizing: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
        };
        // No placements → no manifest at all.
        assert!(emit_scene_manifest(&cfg).is_none());

        cfg.scene.assets = vec![
            SceneAssetDef {
                key: "sky".into(),
                kind: SceneKind::Layer,
                name: String::new(),
                description: String::new(),
                provider: String::new(),
                cutouts: false,
                wrap: true,
                placement: Some(ScenePlacement {
                    parallax: Some(0.05),
                    overscan: Some(0.08),
                    ..Default::default()
                }),
                variants: vec![],
            },
            SceneAssetDef {
                key: "desk".into(),
                kind: SceneKind::Sprite,
                name: String::new(),
                description: String::new(),
                provider: String::new(),
                cutouts: false,
                wrap: false,
                placement: Some(ScenePlacement {
                    fit: "anchored".into(),
                    anchor: Some(vec![0.5, 1.0]),
                    pos: Some(vec![0.15, 1.0]),
                    height: Some(0.42),
                    parallax: Some(0.55),
                    ..Default::default()
                }),
                variants: vec![
                    SceneVariantDef {
                        key: "landscape".into(),
                        preset: String::new(),
                        extra_prompt: String::new(),
                        placement: None,
                    },
                    SceneVariantDef {
                        key: "portrait".into(),
                        preset: String::new(),
                        extra_prompt: String::new(),
                        placement: Some(ScenePlacement {
                            pos: Some(vec![0.30, 0.86]),
                            height: Some(0.30),
                            ..Default::default()
                        }),
                    },
                ],
            },
        ];
        let json: serde_json::Value =
            serde_json::from_str(&emit_scene_manifest(&cfg).expect("manifest")).unwrap();
        assert_eq!(json["schema"], 1);
        let layers = json["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2, "z-order = array order, config order kept");

        // Cover layer: auto bg_ prefix, default fit, wrap flag carried.
        assert_eq!(layers[0]["id"], "sky");
        assert_eq!(layers[0]["asset"], "bg_sky");
        assert_eq!(layers[0]["fit"], "cover");
        assert_eq!(layers[0]["wrap"], true);
        assert_eq!(layers[0]["parallax"], 0.05);

        // Anchored set-piece: base asset = first variant; portrait override carries its
        // own texture key + shallow field overrides.
        assert_eq!(layers[1]["asset"], "bg_desk_landscape");
        assert_eq!(layers[1]["anchor"], serde_json::json!([0.5, 1.0]));
        let portrait = &layers[1]["variants"]["portrait"];
        assert_eq!(portrait["asset"], "bg_desk_portrait");
        assert_eq!(portrait["pos"], serde_json::json!([0.30, 0.86]));
        assert_eq!(portrait["height"], 0.30);
        assert!(portrait.get("anchor").is_none(), "shallow override only");
    }
}
