//! Stake-format export: assemble processed finals into the ASSETS.md §16 dist tree and
//! emit the §17 `assets.ts` manifest snippet. Also runs the §18 readiness check.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// Value-budget gate outcome (None only for legacy callers).
    #[serde(default)]
    pub tone: Option<ValueReport>,
    /// True when the gate refused and nothing was written (re-run with force to override).
    #[serde(default)]
    pub blocked: bool,
}

/// Preload set (§17) — assets needed before first interaction. (Audio is one `sound` audiosprite
/// asset, always preloaded — see `audio_asset_entry` — so it isn't listed here.)
fn is_preload(key: &str) -> bool {
    key.starts_with("bg_base_") || key.starts_with("splash_hero_")
}

/// One asset's row in the value-budget gate report.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ToneGateRow {
    pub key: String,
    pub median: f64,
    pub band_min: f64,
    pub band_max: f64,
    /// "in_band" | "corrected" | "needs_regen" | "unmeasured"
    pub flag: String,
}

/// The value-budget gate: per-asset band conformance + cross-asset ordering.
/// `ok == false` blocks export/publish unless the user forces.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ValueReport {
    pub ok: bool,
    pub rows: Vec<ToneGateRow>,
    /// Human-readable violations, most severe first.
    pub violations: Vec<String>,
}

/// Measure a shipped asset's median value straight off its processed png/jpg twin
/// (fallback when no tone report exists — e.g. processed before the tone pass).
fn shipped_median(base: &Path, game_id: &str, key: &str, alpha_floor: u8) -> Option<f64> {
    let rec = storage::read_asset_record(base, game_id, key).ok().flatten()?;
    let active = rec.active_variation.as_ref()?;
    let var = rec.variations.iter().find(|v| &v.id == active)?;
    let stage = var
        .stages
        .iter()
        .find(|s| s.name == "png")
        .or_else(|| var.stages.iter().find(|s| s.name == "jpg"))?;
    let path = storage::variation_dir(base, game_id, key, &var.id).join(&stage.file);
    let img = image::open(path).ok()?.to_rgba8();
    crate::processing::tone::median_value(&img, alpha_floor)
}

/// The SHARED value-budget gate — export and publish both run exactly this, so both
/// exits give the same verdict. Checks every banded asset in `keys` (None = all
/// derived assets) against its band, then asserts the blueprint's tone-ordering rules
/// on shipped medians.
pub fn tone_gate(
    base: &Path,
    game_id: &str,
    keys: Option<&[String]>,
) -> Result<ValueReport, String> {
    let project = storage::read_project(base, game_id)?;
    let cfg = &project.config;
    let t = cfg.symbol_tone;
    let assets = taxonomy::derive_assets(cfg);

    let mut rows = Vec::new();
    let mut violations = Vec::new();
    let mut medians: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    for asset in &assets {
        if asset.production != Production::Raster {
            continue;
        }
        if keys.is_some_and(|ks| !ks.contains(&asset.key)) {
            continue;
        }
        // Band: symbols → class band / per-symbol override; scenes → tonalTarget.
        let band = if let Some((sym, _)) = cfg.symbol_for_asset(&asset.key) {
            Some(match sym.tone_target {
                Some(m) => (m, m),
                None => {
                    let b = t.class_for(sym.role);
                    (b.min, b.max)
                }
            })
        } else {
            cfg.scene
                .def_for_key(&asset.key)
                .and_then(|d| d.tonal_target)
                .map(|b| (b.min, b.max))
        };
        let Some((min, max)) = band else { continue };

        let rec = storage::read_asset_record(base, game_id, &asset.key)?;
        let var = rec.as_ref().and_then(|r| {
            r.active_variation
                .as_ref()
                .and_then(|id| r.variations.iter().find(|v| &v.id == id))
        });
        // Unprocessed / missing assets are the export "missing" list's business,
        // not the tone gate's.
        if var.is_none_or(|v| !v.stages.iter().any(|s| s.name == "webp")) {
            continue;
        }
        let var = var.unwrap();

        let (median, flag) = match &var.tone_report {
            Some(r) => (
                r.median_after,
                if r.flag == "needs_regen" { "needs_regen" } else if r.flag == "corrected" { "corrected" } else { "in_band" },
            ),
            None => match shipped_median(base, game_id, &asset.key, t.alpha_floor) {
                // No tone report = processed before the tone pass. Out-of-band here
                // is only a REGEN case when the needed gamma exceeds the clamp —
                // otherwise a plain Reprocess bakes the correction.
                Some(m) => {
                    let flag = if m >= min && m <= max {
                        "in_band"
                    } else if m > 1.0 / 255.0 && m < 1.0 {
                        let g = m.clamp(min, max).ln() / m.ln();
                        if g >= t.gamma_lo && g <= t.gamma_hi { "reprocess" } else { "needs_regen" }
                    } else {
                        "needs_regen"
                    };
                    (m, flag)
                }
                None => (0.0, "unmeasured"),
            },
        };
        medians.insert(asset.key.clone(), median);
        match flag {
            "needs_regen" => violations.push(format!(
                "{}: median {:.3} vs band {:.3}–{:.3} — regenerate (correction beyond the gamma clamp)",
                asset.key, median, min, max
            )),
            "reprocess" => violations.push(format!(
                "{}: median {:.3} vs band {:.3}–{:.3} — run Process to bake the tone correction",
                asset.key, median, min, max
            )),
            _ => {}
        }
        rows.push(ToneGateRow {
            key: asset.key.clone(),
            median,
            band_min: min,
            band_max: max,
            flag: flag.into(),
        });
    }

    // Cross-asset ordering (e.g. back wall < storm sky, or windows read as holes).
    for rule in &cfg.scene.tone_ordering {
        let (Some(&lo), Some(&hi)) = (medians.get(&rule.darker), medians.get(&rule.brighter))
        else {
            continue; // one side not shipping in this run — nothing to assert
        };
        if lo >= hi {
            violations.push(format!(
                "ordering broken: {} ({:.3}) must stay darker than {} ({:.3})",
                rule.darker, lo, rule.brighter, hi
            ));
        }
    }

    Ok(ValueReport { ok: violations.is_empty(), rows, violations })
}

/// Build the dist tree for a game and return a report. Rebuilds dist from scratch.
pub fn build_dist(base: &Path, game_id: &str) -> Result<ExportReport, String> {
    build_dist_gated(base, game_id, true)
}

/// `force = false` → the value-budget gate can refuse: nothing is written and the
/// report comes back `blocked` with the violations. `force = true` writes regardless
/// (the report still carries the violations + value_report.json lands in dist).
pub fn build_dist_gated(base: &Path, game_id: &str, force: bool) -> Result<ExportReport, String> {
    let tone = tone_gate(base, game_id, None)?;
    if !tone.ok && !force {
        return Ok(ExportReport {
            dist_path: String::new(),
            written: Vec::new(),
            missing: Vec::new(),
            waived: Vec::new(),
            manifest_snippet: String::new(),
            tone: Some(tone),
            blocked: true,
        });
    }
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
    let mut audio_takes: Vec<AudioTake> = Vec::new();

    for asset in &assets {
        // Audio cues aren't image-processed — collect each ready take; they bake into ONE
        // Howler audiosprite (dist/audio/sounds.*) after the loop.
        if asset.production == Production::Audio {
            if let Some(cue) = project.config.audio.cues.iter().find(|c| c.key == asset.key) {
                match audio_take(base, game_id, asset, cue)? {
                    Some(take) => {
                        written.push(asset.key.clone());
                        audio_takes.push(take);
                    }
                    None => missing.push(asset.key.clone()),
                }
            }
            continue;
        }
        // Fonts bake deterministically from their FontDef (rasterize → BMFont .xml + .webp page).
        if asset.production == Production::Font {
            if let Some(def) = project.config.fonts.iter().find(|f| f.key == asset.key) {
                manifest_entries.push(export_font(def, &dist_root)?);
                written.push(asset.key.clone());
            } else {
                missing.push(asset.key.clone());
            }
            continue;
        }
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

    // Audio: bake ALL cues into ONE Howler audiosprite (dist/audio/sounds.{ogg,m4a,mp3,ac3,json}) —
    // the exact shape Stake Engine's `utils-sound` loads. Registered as a single assets.ts entry.
    if !audio_takes.is_empty() {
        let entry = bake_audio_sprite(&audio_takes, &dist_root)?;
        manifest_entries.push(entry);
    }

    let snippet = render_manifest(&manifest_entries);
    fs::write(dist_root.join("assets.ts.snippet"), &snippet)
        .map_err(|e| format!("write manifest failed: {e}"))?;

    // The value-budget verdict ships WITH the dist so the game agent can read it.
    if let Ok(json) = serde_json::to_vec_pretty(&tone) {
        let _ = fs::write(dist_root.join("value_report.json"), json);
    }

    Ok(ExportReport {
        dist_path: dist_root.to_string_lossy().into_owned(),
        written,
        missing,
        waived,
        manifest_snippet: snippet,
        tone: Some(tone),
        blocked: false,
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

// ── Audio export: one Howler audiosprite (dist/audio/sounds.*) ──────────────────
// Stake Engine games load audio as a SINGLE audiosprite — `sounds.ogg/m4a/mp3/ac3` (all cues
// concatenated, four encodings) + a `sounds.json` sprite-map — registered as one `type: 'audio'`
// asset and played via `utils-sound` (Howler). We bake it with the `audiosprite` CLI (ffmpeg-based),
// exactly the tool that produced the web-sdk's own files, then reshape its JSON to Stake's contract.

/// A processed cue ready to bake: its key (→ sprite name), the normalized wav on disk, whether it
/// loops (→ sprite loop flag + `music`/`loop` player), and its gain (→ `config` volume).
struct AudioTake {
    key: String,
    wav: PathBuf,
    looped: bool,
    gain: f32,
}

/// Resolve a cue's active, processed take (the normalized `final.wav`), or `None` if not ready.
fn audio_take(
    base: &Path,
    game_id: &str,
    asset: &AssetDescriptor,
    cue: &crate::model::game_config::AudioCueDef,
) -> Result<Option<AudioTake>, String> {
    let Some(record) = storage::read_asset_record(base, game_id, &asset.key)? else {
        return Ok(None);
    };
    let Some(active_id) = record.active_variation.clone() else {
        return Ok(None);
    };
    let Some(variation) = record.variations.iter().find(|v| v.id == active_id) else {
        return Ok(None);
    };
    // Must be processed (the normalized wav exists).
    let Some(stage) = variation.stages.iter().find(|s| s.name == "wav") else {
        return Ok(None);
    };
    let wav = storage::variation_dir(base, game_id, &asset.key, &active_id).join(&stage.file);
    if !wav.is_file() {
        return Ok(None);
    }
    Ok(Some(AudioTake { key: asset.key.clone(), wav, looped: cue.looped, gain: cue.gain }))
}

/// Concatenate every processed cue into ONE Howler audiosprite via the `audiosprite` CLI and write
/// `dist/audio/sounds.{ogg,m4a,mp3,ac3,json}`. Returns the single `assets.ts` entry that registers
/// it. `ogg` (Vorbis) is included only when this ffmpeg has libvorbis — audiosprite aborts the whole
/// bake if any requested encoder is missing, and m4a+mp3+ac3 already cover every browser.
fn bake_audio_sprite(takes: &[AudioTake], dist_root: &Path) -> Result<String, String> {
    if crate::processing::which("audiosprite").is_none() {
        return Err("audiosprite is not installed — required to bake the game audio sprite (`npm i -g audiosprite`; needs ffmpeg).".into());
    }
    let audio_dir = dist_root.join("audio");
    fs::create_dir_all(&audio_dir).map_err(|e| format!("create audio dir: {e}"))?;

    // audiosprite names each sprite after its input file's basename → stage each take as `<key>.wav`
    // in a temp inputs dir, in config order so the sprite offsets are deterministic.
    let in_dir = audio_dir.join("_in");
    let _ = fs::remove_dir_all(&in_dir);
    fs::create_dir_all(&in_dir).map_err(|e| format!("create audio inputs dir: {e}"))?;
    let mut inputs: Vec<PathBuf> = Vec::new();
    for t in takes {
        let named = in_dir.join(format!("{}.wav", t.key));
        fs::copy(&t.wav, &named).map_err(|e| format!("stage {} for sprite: {e}", t.key))?;
        inputs.push(named);
    }

    let formats = if crate::processing::ffmpeg_has_libvorbis() {
        "ogg,m4a,mp3,ac3"
    } else {
        "m4a,mp3,ac3"
    };

    let out_base = audio_dir.join("sounds");
    let mut cmd = Command::new("audiosprite");
    cmd.args(["--format", "howler2", "--export", formats, "--channels", "2", "--path", "./assets/audio"])
        .arg("--output")
        .arg(&out_base);
    for t in takes.iter().filter(|t| t.looped) {
        cmd.args(["--loop", &t.key]);
    }
    for i in &inputs {
        cmd.arg(i);
    }

    let out = cmd.output().map_err(|e| format!("audiosprite failed to launch: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(4).collect::<Vec<_>>().join(" ").chars().take(400).collect();
        return Err(format!("audiosprite error: {tail}"));
    }
    let _ = fs::remove_dir_all(&in_dir);

    // audiosprite emits howler2 JSON ({src, sprite}); reshape to Stake's exact {src, sprite, config}.
    finalize_sounds_json(&out_base.with_extension("json"), takes)?;
    Ok(audio_asset_entry())
}

/// Reshape audiosprite's howler2 JSON into Stake's contract: keep the `sprite` map, rebuild `src` to
/// canonical `./assets/audio/sounds.<ext>` for the formats actually written, and add the per-cue
/// `config` volume map from each cue's gain.
fn finalize_sounds_json(json_path: &Path, takes: &[AudioTake]) -> Result<(), String> {
    let raw = fs::read_to_string(json_path).map_err(|e| format!("read sounds.json: {e}"))?;
    let mut doc: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse sounds.json: {e}"))?;

    let dir = json_path.parent().unwrap_or_else(|| Path::new("."));
    let src: Vec<serde_json::Value> = ["ogg", "m4a", "mp3", "ac3"]
        .iter()
        .filter(|ext| dir.join(format!("sounds.{ext}")).is_file())
        .map(|ext| serde_json::Value::String(format!("./assets/audio/sounds.{ext}")))
        .collect();

    let config: serde_json::Map<String, serde_json::Value> = takes
        .iter()
        .map(|t| {
            // Round to 3 dp so f32 gains serialize cleanly (0.7, not 0.69999998).
            let volume = ((t.gain as f64) * 1000.0).round() / 1000.0;
            (t.key.clone(), serde_json::json!({ "volume": volume }))
        })
        .collect();

    if let Some(obj) = doc.as_object_mut() {
        obj.insert("src".into(), serde_json::Value::Array(src));
        obj.insert("config".into(), serde_json::Value::Object(config));
    }
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| format!("serialize sounds.json: {e}"))?;
    fs::write(json_path, pretty).map_err(|e| format!("write sounds.json: {e}"))
}

/// The single `assets.ts` entry registering the audiosprite (pixi-svelte `type: 'audio'`), exactly
/// as the web-sdk example apps declare it. Always preloaded.
fn audio_asset_entry() -> String {
    "  sound: { type: 'audio', src: new URL('../../assets/audio/sounds.json', import.meta.url).href, preload: true },".to_string()
}

/// Bake the game's ready audio cues into a Howler audiosprite under `<dest>/audio/` — the same
/// artifact `build_dist` writes to `dist/audio/`. Returns the baked cue keys (empty when the game
/// ships no ready audio). Used by the one-click publish path; the sprite is all-or-nothing, so it
/// always bakes the full ready set regardless of a partial selection.
pub fn publish_audio_sprite(base: &Path, game_id: &str, dest: &Path) -> Result<Vec<String>, String> {
    let project = storage::read_project(base, game_id)?;
    if !project.config.has_audio {
        return Ok(Vec::new());
    }
    let mut takes = Vec::new();
    for asset in taxonomy::derive_assets(&project.config) {
        if asset.production != Production::Audio {
            continue;
        }
        if let Some(cue) = project.config.audio.cues.iter().find(|c| c.key == asset.key) {
            if let Some(take) = audio_take(base, game_id, &asset, cue)? {
                takes.push(take);
            }
        }
    }
    if takes.is_empty() {
        return Ok(Vec::new());
    }
    let keys = takes.iter().map(|t| t.key.clone()).collect();
    bake_audio_sprite(&takes, dest)?;
    Ok(keys)
}

// ── Font export: deterministic BMFont bake (dist/fonts/<key>/<key>.{xml,webp}) ──────────────────

/// Rasterize a `FontDef` and write its BMFont pair to `dist/fonts/<key>/`, returning the single
/// `type: 'font'` manifest entry that registers it (pixi-svelte loads the `.xml`, which references
/// the sibling `.webp` page).
fn export_font(def: &crate::model::game_config::FontDef, dist_root: &Path) -> Result<String, String> {
    let (xml, webp) = crate::processing::font::rasterize_font(def)?;
    let dir = dist_root.join("fonts").join(&def.key);
    fs::create_dir_all(&dir).map_err(|e| format!("create fonts/{}: {e}", def.key))?;
    fs::write(dir.join(format!("{}.xml", def.key)), xml).map_err(|e| format!("write font xml: {e}"))?;
    fs::write(dir.join(format!("{}.webp", def.key)), webp).map_err(|e| format!("write font page: {e}"))?;
    Ok(font_asset_entry(&def.key))
}

fn font_asset_entry(key: &str) -> String {
    format!(
        "  {key}: {{ type: 'font', src: new URL('../../assets/fonts/{key}/{key}.xml', import.meta.url).href }},"
    )
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
        // Each layer plate is a single alpha WebP → pixi-svelte `sprite` (one Texture). There is
        // NO `image` asset type in the web-sdk (RawType = spine|sprite|sprites|spriteSheet|font|audio).
        entries.push(format!(
            "  {key}_{lid}: {{ type: 'sprite', src: new URL('../../assets/layers/{category}/{file}', import.meta.url).href }},",
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
mod audio_sprite_tests {
    use super::*;

    /// `export_font` rasterizes a FontDef into the BMFont pair the web-sdk loads.
    #[test]
    fn export_font_writes_bmfont_pair() {
        let dir = std::env::temp_dir().join(format!("wf_fontexport_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let def = crate::model::game_config::default_fonts().remove(0); // font_white
        let entry = export_font(&def, &dir).expect("export_font");
        // Files land under fonts/<key>/, xml + webp page.
        assert!(dir.join(format!("fonts/{k}/{k}.xml", k = def.key)).is_file(), "xml written");
        assert!(dir.join(format!("fonts/{k}/{k}.webp", k = def.key)).is_file(), "webp page written");
        // Manifest entry is a valid pixi-svelte `type: 'font'` line pointing at the .xml.
        assert!(entry.contains(&format!("{}: {{ type: 'font'", def.key)));
        assert!(entry.contains(&format!("assets/fonts/{k}/{k}.xml", k = def.key)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// `finalize_sounds_json` reshapes howler2 output into Stake's {sprite, src, config} — no
    /// external tools needed (fakes the audiosprite JSON + touches the format files).
    #[test]
    fn sounds_json_is_stake_audiosprite_shaped() {
        let dir = std::env::temp_dir().join(format!("wf_sprite_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Pretend audiosprite already wrote its howler2 json + the m4a/mp3/ac3 (no ogg on this box).
        let json_path = dir.join("sounds.json");
        fs::write(
            &json_path,
            r#"{"src":["assets/audio/sounds.m4a"],"sprite":{"bgm_main":[0,2000,true],"sfx_btn_spin":[3000,1000]}}"#,
        )
        .unwrap();
        for ext in ["m4a", "mp3", "ac3"] {
            fs::write(dir.join(format!("sounds.{ext}")), b"x").unwrap();
        }
        let takes = vec![
            AudioTake { key: "bgm_main".into(), wav: dir.join("x.wav"), looped: true, gain: 0.7 },
            AudioTake { key: "sfx_btn_spin".into(), wav: dir.join("y.wav"), looped: false, gain: 1.0 },
        ];
        finalize_sounds_json(&json_path, &takes).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
        // sprite preserved (loop flag on bgm_main only).
        assert_eq!(v["sprite"]["bgm_main"][2], true);
        assert_eq!(v["sprite"]["sfx_btn_spin"].as_array().unwrap().len(), 2);
        // src rebuilt canonically for the formats that exist (no ogg → 3 entries, ./ prefix).
        assert_eq!(v["src"][0], "./assets/audio/sounds.m4a");
        assert_eq!(v["src"].as_array().unwrap().len(), 3);
        // config volume map from cue gains.
        assert_eq!(v["config"]["bgm_main"]["volume"], 0.7);
        assert_eq!(v["config"]["sfx_btn_spin"]["volume"], 1.0);

        // The single assets.ts entry the web-sdk expects.
        let entry = audio_asset_entry();
        assert!(entry.contains("sound: { type: 'audio'"));
        assert!(entry.contains("assets/audio/sounds.json"));
        assert!(entry.contains("preload: true"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// End-to-end bake through the real `audiosprite` CLI (skipped when it or ffmpeg is absent).
    #[test]
    fn bakes_a_real_audiosprite() {
        if crate::processing::which("audiosprite").is_none()
            || !crate::processing::audio::ffmpeg_available()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("wf_bake_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        let mut takes = Vec::new();
        for (key, freq, looped, gain) in
            [("bgm_main", "220", true, 0.7f32), ("sfx_btn_spin", "880", false, 1.0)]
        {
            let wav = src.join(format!("{key}.wav"));
            Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration=1")])
                .arg(&wav)
                .output()
                .unwrap();
            takes.push(AudioTake { key: key.into(), wav, looped, gain });
        }
        let dist = dir.join("dist");
        let entry = bake_audio_sprite(&takes, &dist).expect("bake");

        let audio = dist.join("audio");
        assert!(audio.join("sounds.mp3").is_file(), "mp3 written");
        assert!(audio.join("sounds.json").is_file(), "sounds.json written");
        assert!(!audio.join("_in").exists(), "temp inputs cleaned up");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(audio.join("sounds.json")).unwrap()).unwrap();
        assert!(v["sprite"]["bgm_main"].is_array(), "cue in sprite map");
        assert_eq!(v["sprite"]["bgm_main"][2], true, "looped cue flagged");
        assert_eq!(v["config"]["bgm_main"]["volume"], 0.7);
        assert!(entry.contains("type: 'audio'"));

        let _ = fs::remove_dir_all(&dir);
    }
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
            tone_target: None,
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
            symbol_tone: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
            has_audio: false,
            audio: Default::default(),
            has_loader: true,
            has_fonts: false,
            fonts: Vec::new(),
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
            tone_report: None,
            audio_report: None,
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
    fn build_dist_bakes_fonts_deterministically() {
        let base = std::env::temp_dir().join(format!("wf_export_fonts_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let game_id = "expgame";
        let mut cfg = config();
        cfg.has_fonts = true;
        cfg.fonts = crate::model::game_config::default_fonts();
        storage::write_project(&base, &Project::new(cfg, 0.0)).unwrap();

        let report = build_dist(&base, game_id).unwrap();

        // Each font baked its BMFont pair from config alone (no records needed).
        for key in ["font_gold", "font_white"] {
            assert!(report.written.contains(&key.to_string()), "{key} written");
            let dir = Path::new(&report.dist_path).join(format!("fonts/{key}"));
            assert!(dir.join(format!("{key}.xml")).is_file(), "{key}.xml");
            assert!(dir.join(format!("{key}.webp")).is_file(), "{key}.webp");
        }
        // Manifest registers them as pixi-svelte `type: 'font'`.
        assert!(report.manifest_snippet.contains("font_gold: { type: 'font'"));
        assert!(report.manifest_snippet.contains("../../assets/fonts/font_gold/font_gold.xml"));

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
            tone_report: None,
            audio_report: None,
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
        // Each layer ships as its OWN `sprite` entry — the code layer owns stacking/motion.
        assert!(report.manifest_snippet.contains("bg_base_landscape_sky: { type: 'sprite'"));
        assert!(report.manifest_snippet.contains("bg_base_landscape_fg: { type: 'sprite'"));
        assert!(!report.manifest_snippet.contains("type: 'parallax'"));
        assert!(!report.manifest_snippet.contains("type: 'image'"));
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
            tone_report: None,
            audio_report: None,
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
            symbol_tone: Default::default(),
            symbol_provider: String::new(),
            scene: Default::default(),
            has_audio: false,
            audio: Default::default(),
            has_loader: true,
            has_fonts: false,
            fonts: Vec::new(),
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
                tonal_target: None,
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
                tonal_target: None,
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
