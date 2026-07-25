//! Post-finish quality audit: a battery of standards every shipped take should meet,
//! computed from the ACTUAL processed pixels + the record/config. Surfaced in the
//! bench's Quality section — pass/warn/fail per check, with the reason and the fix.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{config_dir, find_descriptor, projects_root};
use crate::model::asset::AssetKind;
use crate::storage;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QualityCheck {
    pub id: String,
    pub label: String,
    /// "pass" | "warn" | "fail"
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub checks: Vec<QualityCheck>,
    /// warn + fail count (the badge number).
    pub issues: u32,
}

fn check(id: &str, label: &str, status: &str, detail: impl Into<String>) -> QualityCheck {
    QualityCheck {
        id: id.into(),
        label: label.into(),
        status: status.into(),
        detail: detail.into(),
    }
}

/// Audit one variation of an asset. Cheap enough to run on every Finish.
#[tauri::command]
#[specta::specta]
pub async fn asset_quality(
    app: tauri::AppHandle,
    game_id: String,
    asset_key: String,
    variation_id: String,
) -> Result<QualityReport, String> {
    let _ = config_dir(&app)?; // (settings not needed yet; keeps the signature future-proof)
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let descriptor = find_descriptor(&project.config, &asset_key)?;
    let record = storage::read_asset_record(&base, &game_id, &asset_key)?
        .ok_or_else(|| "asset has no record yet".to_string())?;
    let var = record
        .variations
        .iter()
        .find(|v| v.id == variation_id)
        .ok_or_else(|| format!("variation \"{variation_id}\" not found"))?
        .clone();

    let mut checks: Vec<QualityCheck> = Vec::new();

    // ── Processed at all? Without stages there is nothing to audit. ──
    let has_webp = var.stages.iter().any(|s| s.name == "webp");
    if !has_webp {
        checks.push(check(
            "processed",
            "Processed",
            "warn",
            "Not processed yet — run Process, then the quality checks audit the shipped pixels.",
        ));
        return Ok(finish(checks));
    }
    checks.push(check("processed", "Processed", "pass", "WebP + twin present."));

    // ── Visual mass (symbols) — the tier's perceived-size contract. ──
    if descriptor.kind == AssetKind::Symbol {
        if let Some(m) = &var.mass_report {
            let pct = |v: f64| (v * 100.0).round();
            match m.flag.as_str() {
                "underweight" => checks.push(check(
                    "mass",
                    "Visual weight",
                    "warn",
                    format!(
                        "UNDERWEIGHT: ink {}% vs the {}% class target{} — too slim to reach class weight inside the cell. An art fix: add a base, mount or bracket at generation, or re-pose more compactly.",
                        pct(m.achieved),
                        pct(m.target),
                        if m.clamped_by == "box" { " (clamped by the safe box)" } else { "" },
                    ),
                )),
                "overweight" => checks.push(check(
                    "mass",
                    "Visual weight",
                    "warn",
                    format!(
                        "OVERWEIGHT: ink {}% vs the {}% class target — the object over-fills its bbox for its class. Consider a slimmer composition or a per-symbol nudge < 1.",
                        pct(m.achieved),
                        pct(m.target),
                    ),
                )),
                _ => checks.push(check(
                    "mass",
                    "Visual weight",
                    "pass",
                    format!(
                        "ink {}% of the {}% class target (scale ×{:.2}).",
                        pct(m.achieved),
                        pct(m.target),
                        m.scale_final,
                    ),
                )),
            }
            if m.upscaled {
                checks.push(check(
                    "fit-upscale",
                    "Fit upscale",
                    "warn",
                    format!(
                        "The fit scaled ×{:.2} (> 1.15) — the source lacks resolution for its class. Regenerate larger or run ✦ AI upscale before Process.",
                        m.scale_final
                    ),
                ));
            }
        }
    }

    // ── Pixel checks on the shipped PNG (alpha twin) or raw fallback. ──
    let bytes = var
        .stages
        .iter()
        .find(|s| s.name == "png")
        .and_then(|s| {
            let rel = format!("variations/{}/{}", var.id, s.file);
            storage::read_asset_file(&base, &game_id, &asset_key, &rel).ok()
        })
        .or_else(|| {
            std::fs::read(storage::asset_dir(&base, &game_id, &asset_key).join(&var.raw_file)).ok()
        });
    let Some(bytes) = bytes else {
        checks.push(check("pixels", "Pixel audit", "warn", "Could not read the shipped image."));
        return Ok(finish(checks));
    };

    let scene_def = project.config.scene.def_for_key(&asset_key).cloned();
    // The full-column `_expanded` twin of an expanding wild legitimately runs
    // edge-to-edge vertically — audit it with the scene tolerance, not the symbol one.
    let expanded = project
        .config
        .symbol_for_asset(&asset_key)
        .is_some_and(|(_, exp)| exp);
    let is_symbol = descriptor.kind == AssetKind::Symbol && !expanded;
    let author_long = descriptor.author_w.unwrap_or(0).max(descriptor.author_h.unwrap_or(0));

    let mut px = tauri::async_runtime::spawn_blocking(move || {
        pixel_checks(&bytes, is_symbol, author_long, scene_def.as_ref())
    })
    .await
    .map_err(|e| format!("quality task failed: {e}"))??;
    checks.append(&mut px);

    Ok(finish(checks))
}

fn finish(checks: Vec<QualityCheck>) -> QualityReport {
    let issues = checks.iter().filter(|c| c.status != "pass").count() as u32;
    QualityReport { checks, issues }
}

/// All checks that read pixels. Pure so it's unit-testable.
fn pixel_checks(
    bytes: &[u8],
    is_symbol: bool,
    author_long: u32,
    scene_def: Option<&crate::model::game_config::SceneAssetDef>,
) -> Result<Vec<QualityCheck>, String> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| format!("decode failed: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut out = Vec::new();

    // ── Resolution: shipping far below the author target reads soft in game. ──
    let long = w.max(h);
    if author_long > 0 && is_symbol && long < 900 {
        out.push(check(
            "resolution",
            "Resolution",
            "warn",
            format!("Ships at {long}px long side — under the ~1024px working target. Run ✦ AI upscale (before Process) for a crisper final."),
        ));
    } else {
        out.push(check("resolution", "Resolution", "pass", format!("{w}×{h}.")));
    }

    let has_alpha = img.pixels().any(|p| p.0[3] < 250);
    if has_alpha {
        let opaque: usize = img.pixels().filter(|p| p.0[3] > 127).count();
        if opaque > 0 {
            // ── Chroma remnants: leftover keying magenta reads as a hard bug. ──
            let magenta = img
                .pixels()
                .filter(|p| p.0[3] > 64 && p.0[0] > 200 && p.0[2] > 200 && p.0[1] < 80)
                .count();
            if magenta > 50 {
                out.push(check(
                    "chroma",
                    "Background keying",
                    "fail",
                    format!("{magenta} magenta chroma pixels survived keying — Edit alpha to erase them, or reprocess."),
                ));
            } else {
                out.push(check("chroma", "Background keying", "pass", "No chroma remnants."));
            }

            // ── Edge contact: a clipped subject or unkeyed border band. ──
            let mut border = 0usize;
            let mut border_hit = 0usize;
            for x in 0..w {
                for y in [0, h - 1] {
                    border += 1;
                    if img.get_pixel(x, y).0[3] > 32 {
                        border_hit += 1;
                    }
                }
            }
            for y in 0..h {
                for x in [0, w - 1] {
                    border += 1;
                    if img.get_pixel(x, y).0[3] > 32 {
                        border_hit += 1;
                    }
                }
            }
            let edge_frac = border_hit as f64 / border.max(1) as f64;
            // Scene layers/sprites may legitimately touch edges; symbols must not.
            let edge_limit = if is_symbol { 0.02 } else { 0.60 };
            if edge_frac > edge_limit {
                out.push(check(
                    "edges",
                    "Canvas edges",
                    "warn",
                    format!(
                        "{:.0}% of the border carries opaque pixels — the subject is clipped or the background didn't key. Check the silhouette.",
                        edge_frac * 100.0
                    ),
                ));
            } else {
                out.push(check("edges", "Canvas edges", "pass", "Clean margin."));
            }

            // ── Stray specks: keying noise shows as floating dots in game. ──
            let specks = count_specks(&img, (opaque / 5000).max(16));
            if specks > 5 {
                out.push(check(
                    "specks",
                    "Stray specks",
                    "warn",
                    format!("{specks} tiny disconnected fragments — keying noise. Edit alpha (lasso) clears them fast."),
                ));
            } else {
                out.push(check("specks", "Stray specks", "pass", "No floating fragments."));
            }

            // ── Halo fringe: a wide soft rim looks muddy on any background. ──
            let fringe = img.pixels().filter(|p| p.0[3] > 0 && p.0[3] < 40).count();
            let fringe_frac = fringe as f64 / opaque as f64;
            if fringe_frac > 0.18 {
                out.push(check(
                    "halo",
                    "Halo fringe",
                    "warn",
                    format!(
                        "{:.0}% near-transparent fringe around the subject — reprocess or tighten with Edit alpha.",
                        fringe_frac * 100.0
                    ),
                ));
            } else {
                out.push(check("halo", "Halo fringe", "pass", "Crisp edge."));
            }
        }
    }

    // ── Scene-class contracts. ──
    if let Some(def) = scene_def {
        use crate::model::game_config::SceneKind;
        if def.cutouts {
            if has_interior_holes(&img) {
                out.push(check("cutouts", "Interior cut-outs", "pass", "See-through openings present."));
            } else {
                out.push(check(
                    "cutouts",
                    "Interior cut-outs",
                    "warn",
                    "Marked as having cut-outs, but no interior transparency detected — the openings didn't render as chroma, or keying missed them.",
                ));
            }
        }
        if def.wrap && matches!(def.kind, SceneKind::Layer | SceneKind::Plate) {
            let seam = wrap_seam_diff(&img);
            if seam > 0.12 {
                out.push(check(
                    "wrap",
                    "Wrap seam",
                    "warn",
                    format!("{:.0}% left↔right edge difference — the horizontal tiling will show a seam. Reroll with the tileable clause, or check with the seam-check toggle.", seam * 100.0),
                ));
            } else {
                out.push(check("wrap", "Wrap seam", "pass", format!("{:.0}% edge difference — tiles cleanly.", seam * 100.0)));
            }
        }
    }

    Ok(out)
}

/// Disconnected opaque components smaller than `min_area` (4-connected).
fn count_specks(img: &image::RgbaImage, min_area: usize) -> usize {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mask: Vec<bool> = img.pixels().map(|p| p.0[3] > 127).collect();
    let mut seen = vec![false; mask.len()];
    let mut specks = 0;
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut area = 0usize;
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if area < min_area {
            specks += 1;
        }
    }
    specks
}

/// Fully-transparent regions NOT reachable from the border = interior cut-outs.
fn has_interior_holes(img: &image::RgbaImage) -> bool {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let transparent: Vec<bool> = img.pixels().map(|p| p.0[3] < 16).collect();
    let mut reach = vec![false; transparent.len()];
    let mut stack = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let i = y * w + x;
            if transparent[i] && !reach[i] {
                reach[i] = true;
                stack.push(i);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let i = y * w + x;
            if transparent[i] && !reach[i] {
                reach[i] = true;
                stack.push(i);
            }
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if transparent[j] && !reach[j] {
                reach[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    // Any transparent pixel the border flood never reached is an interior hole
    // (require a few, not a lone pixel).
    transparent.iter().zip(&reach).filter(|(t, r)| **t && !**r).count() > 32
}

/// Mean absolute RGB difference between the first and last pixel columns (0..1).
fn wrap_seam_diff(img: &image::RgbaImage) -> f64 {
    let (w, h) = img.dimensions();
    if w < 2 {
        return 0.0;
    }
    let mut sum = 0f64;
    for y in 0..h {
        let a = img.get_pixel(0, y).0;
        let b = img.get_pixel(w - 1, y).0;
        sum += (a[0] as f64 - b[0] as f64).abs()
            + (a[1] as f64 - b[1] as f64).abs()
            + (a[2] as f64 - b[2] as f64).abs();
    }
    sum / (h as f64 * 3.0 * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specks_holes_and_seam_detectors() {
        // Big blob + 3 specks.
        let mut img = image::RgbaImage::new(100, 100);
        for y in 20..80 {
            for x in 20..80 {
                img.put_pixel(x, y, image::Rgba([100, 100, 100, 255]));
            }
        }
        for (sx, sy) in [(2, 2), (95, 5), (5, 95)] {
            for d in 0..2 {
                img.put_pixel(sx + d, sy, image::Rgba([50, 50, 50, 255]));
            }
        }
        assert_eq!(count_specks(&img, 16), 3);
        assert!(!has_interior_holes(&img));

        // Punch an interior window into the blob → hole detected.
        for y in 40..55 {
            for x in 40..55 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        assert!(has_interior_holes(&img));

        // Seam: identical edge columns → ~0; opposite colours → high.
        let mut tile = image::RgbaImage::from_pixel(10, 10, image::Rgba([50, 50, 50, 255]));
        assert!(wrap_seam_diff(&tile) < 0.01);
        for y in 0..10 {
            tile.put_pixel(9, y, image::Rgba([250, 250, 250, 255]));
        }
        assert!(wrap_seam_diff(&tile) > 0.5);
    }
}

// ── Contact sheet: the only reliable review surface for symbol scale ─────────────

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ContactCell {
    pub key: String,
    /// Cell origin on the sheet, px.
    pub x: u32,
    pub y: u32,
    /// Post-fit ink fraction (0 when unknown).
    pub ink_pct: f64,
    /// "ok" | "underweight" | "overweight" | "missing" | "".
    pub flag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ContactSheet {
    pub png: String,
    pub cell_w: u32,
    pub cell_h: u32,
    pub cols: u32,
    pub rows: u32,
    pub cells: Vec<ContactCell>,
}

/// Compose every symbol at TRUE cell pixel size into the actual grid, over a flat
/// dark field — judging symbols at 1024 px against white is exactly how scale
/// problems survive to production. Labels/boxes are drawn by the UI as overlays.
#[tauri::command]
#[specta::specta]
pub async fn symbol_contact_sheet(
    app: tauri::AppHandle,
    game_id: String,
) -> Result<ContactSheet, String> {
    let base = projects_root(&app)?;
    let project = storage::read_project(&base, &game_id)?;
    let (cell_w, cell_h) =
        crate::taxonomy::dimensions::symbol_author(project.config.cols, project.config.rows);
    let cols = project.config.cols.max(1);

    // Gather each symbol's shipped image + fit report.
    let mut items: Vec<(String, Option<Vec<u8>>, f64, String)> = Vec::new();
    for sym in &project.config.symbols {
        let key = format!("symbol_{}", sym.key);
        let rec = storage::read_asset_record(&base, &game_id, &key)?;
        let var = rec.as_ref().and_then(|r| {
            r.variations.iter().find(|v| Some(&v.id) == r.active_variation.as_ref()).cloned()
        });
        let bytes = var.as_ref().and_then(|v| {
            v.stages
                .iter()
                .find(|s| s.name == "png")
                .and_then(|s| {
                    let rel = format!("variations/{}/{}", v.id, s.file);
                    storage::read_asset_file(&base, &game_id, &key, &rel).ok()
                })
                .or_else(|| {
                    std::fs::read(storage::asset_dir(&base, &game_id, &key).join(&v.raw_file)).ok()
                })
        });
        let (ink, flag) = var
            .as_ref()
            .and_then(|v| v.mass_report.as_ref())
            .map(|m| (m.achieved, m.flag.clone()))
            .unwrap_or((0.0, String::new()));
        let flag = if bytes.is_none() { "missing".into() } else { flag };
        items.push((sym.key.clone(), bytes, ink, flag));
    }

    tauri::async_runtime::spawn_blocking(move || {
        use base64::Engine;
        let n = items.len().max(1) as u32;
        let rows = n.div_ceil(cols);
        let (sheet_w, sheet_h) = (cols * cell_w, rows * cell_h);
        // #0A0D12 — the review field the game agent specified.
        let mut sheet =
            image::RgbaImage::from_pixel(sheet_w, sheet_h, image::Rgba([0x0a, 0x0d, 0x12, 255]));

        let mut cells = Vec::new();
        for (i, (key, bytes, ink, flag)) in items.iter().enumerate() {
            let (cx, cy) = ((i as u32 % cols) * cell_w, (i as u32 / cols) * cell_h);
            if let Some(bytes) = bytes {
                if let Ok(img) = image::load_from_memory(bytes) {
                    let fitted = img.resize(cell_w, cell_h, image::imageops::FilterType::Lanczos3);
                    let ox = cx + (cell_w - fitted.width()) / 2;
                    let oy = cy + (cell_h - fitted.height()) / 2;
                    image::imageops::overlay(&mut sheet, &fitted.to_rgba8(), ox as i64, oy as i64);
                }
            }
            cells.push(ContactCell {
                key: key.clone(),
                x: cx,
                y: cy,
                ink_pct: *ink,
                flag: flag.clone(),
            });
        }

        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(sheet)
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("encode sheet failed: {e}"))?;
        Ok(ContactSheet {
            png: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
            ),
            cell_w,
            cell_h,
            cols,
            rows,
            cells,
        })
    })
    .await
    .map_err(|e| format!("contact sheet task failed: {e}"))?
}
