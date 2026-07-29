//! Stake-format export command + one-click publish of finished finals.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::projects_root;
use crate::export::{self, ExportReport};
use crate::storage;

/// Build the dist tree + manifest snippet for a game (ASSETS.md §16/§17). Rebuilds from
/// scratch and reports which assets were written vs. still missing a processed variation.
#[tauri::command]
#[specta::specta]
pub fn export_dist(
    app: tauri::AppHandle,
    game_id: String,
    force: bool,
) -> Result<ExportReport, String> {
    let base = projects_root(&app)?;
    export::build_dist_gated(&base, &game_id, force)
}

/// One asset that couldn't be published, with why.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishSkip {
    pub key: String,
    pub reason: String,
}

/// Result of a publish run.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    pub dest: String,
    /// Value-budget gate outcome for the published set.
    #[serde(default)]
    pub tone: Option<export::ValueReport>,
    /// True when the gate refused and nothing was copied (re-run with force).
    #[serde(default)]
    pub blocked: bool,
    /// Asset keys whose `<key>.webp` + secondary twin were copied.
    pub copied: Vec<String>,
    pub skipped: Vec<PublishSkip>,
}

/// Copy READY assets' processed finals into `dest_dir`: `<asset_key>.webp` plus its
/// secondary twin — `.png` for alpha assets, `.jpg` for opaque ones (backgrounds,
/// scene plates, splash) — the "drop straight into the game repo" step. Existing files
/// of the same name are overwritten. When `keys` is non-empty only those assets are
/// considered (else every asset). Remembers `dest_dir` for one-click re-publish.
#[tauri::command]
#[specta::specta]
pub fn publish_finals(
    app: tauri::AppHandle,
    game_id: String,
    dest_dir: String,
    keys: Vec<String>,
    force: bool,
) -> Result<PublishReport, String> {
    let base = projects_root(&app)?;
    // Publish runs the SAME value-budget gate as export — both exits, one verdict.
    let tone = export::tone_gate(&base, &game_id, (!keys.is_empty()).then_some(&keys[..]))?;
    if !tone.ok && !force {
        return Ok(PublishReport {
            dest: dest_dir,
            tone: Some(tone),
            blocked: true,
            copied: Vec::new(),
            skipped: Vec::new(),
        });
    }
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("create dest folder failed: {e}"))?;
    let dest = std::path::Path::new(&dest_dir);

    let records = storage::list_asset_records(&base, &game_id)?;
    // Audio isn't a per-asset file — every cue bakes into ONE Howler audiosprite (below), so its
    // records are skipped in the per-asset loop.
    let audio_keys: std::collections::HashSet<String> = storage::read_project(&base, &game_id)
        .map(|p| p.config.audio.cues.iter().map(|c| c.key.clone()).collect())
        .unwrap_or_default();
    let mut copied = Vec::new();
    let mut skipped = Vec::new();

    for rec in &records {
        // When a selection was given, publish only those assets.
        if !keys.is_empty() && !keys.contains(&rec.key) {
            continue;
        }
        if audio_keys.contains(&rec.key) {
            continue;
        }
        if rec.template_only {
            skipped.push(PublishSkip { key: rec.key.clone(), reason: "template only — never shipped".into() });
            continue;
        }
        let Some(active_id) = rec.active_variation.as_ref() else {
            skipped.push(PublishSkip { key: rec.key.clone(), reason: "no image yet".into() });
            continue;
        };
        let Some(var) = rec.variations.iter().find(|v| &v.id == active_id) else {
            skipped.push(PublishSkip { key: rec.key.clone(), reason: "active variation missing".into() });
            continue;
        };
        let Some(webp) = var.stages.iter().find(|s| s.name == "webp") else {
            skipped.push(PublishSkip { key: rec.key.clone(), reason: "not processed yet".into() });
            continue;
        };
        // The secondary twin is PNG for alpha assets, JPG for opaque ones — take
        // whichever the pipeline produced (requiring PNG silently skipped every
        // background).
        let secondary = var
            .stages
            .iter()
            .find(|s| s.name == "png")
            .map(|s| (s, "png"))
            .or_else(|| var.stages.iter().find(|s| s.name == "jpg").map(|s| (s, "jpg")));

        let vdir = storage::variation_dir(&base, &game_id, &rec.key, &var.id);
        let mut copy = std::fs::copy(vdir.join(&webp.file), dest.join(format!("{}.webp", rec.key)))
            .map(|_| ());
        if let (Ok(()), Some((sec, ext))) = (&copy, secondary) {
            copy = std::fs::copy(vdir.join(&sec.file), dest.join(format!("{}.{ext}", rec.key)))
                .map(|_| ());
        }
        match copy {
            Ok(_) => copied.push(rec.key.clone()),
            Err(e) => skipped.push(PublishSkip { key: rec.key.clone(), reason: format!("copy failed: {e}") }),
        }
    }

    // Audio: bake every ready cue into one Howler audiosprite under <dest>/audio/ (all-or-nothing,
    // so it ignores a partial selection). No-op when the game ships no audio.
    match export::publish_audio_sprite(&base, &game_id, dest) {
        Ok(keys) => copied.extend(keys),
        Err(e) => skipped.push(PublishSkip { key: "audio".into(), reason: e }),
    }

    // Remember the destination for one-click re-publish.
    if let Ok(mut project) = storage::read_project(&base, &game_id) {
        project.publish_dir = Some(dest_dir.clone());
        let _ = storage::write_project(&base, &project);
    }

    Ok(PublishReport { dest: dest_dir, tone: Some(tone), blocked: false, copied, skipped })
}
