//! Typeface resolution — the bundled display faces (embedded in the binary) plus per-project
//! CUSTOM faces a producer imports (their brand font). Custom ids are `custom:<slug>` and live at
//! `<project>/typefaces/<slug>.{ttf,otf}`. Both the font export and the preview command resolve a
//! `FontDef.typeface` id to TTF bytes through here.

use std::path::{Path, PathBuf};

use crate::processing::font;
use crate::storage;

/// A typeface choice surfaced to the Fonts UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FontTypeface {
    pub id: String,
    pub name: String,
    /// True for a project-imported face (removable); false for a bundled one.
    pub custom: bool,
}

fn dir(base: &Path, game_id: &str) -> PathBuf {
    storage::project_dir(base, game_id).join("typefaces")
}

fn slug(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "font".into() } else { out }
}

/// Bundled faces + this project's imported faces.
pub fn list(base: &Path, game_id: &str) -> Vec<FontTypeface> {
    let mut out: Vec<FontTypeface> = font::typefaces()
        .into_iter()
        .map(|t| FontTypeface { id: t.id.into(), name: t.name.into(), custom: false })
        .collect();
    if let Ok(rd) = std::fs::read_dir(dir(base, game_id)) {
        let mut customs: Vec<FontTypeface> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str())?.to_lowercase();
                if ext != "ttf" && ext != "otf" {
                    return None;
                }
                let stem = p.file_stem().and_then(|s| s.to_str())?.to_string();
                Some(FontTypeface { id: format!("custom:{stem}"), name: stem, custom: true })
            })
            .collect();
        customs.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(customs);
    }
    out
}

/// Resolve a typeface id to TTF bytes: bundled → embedded, `custom:<slug>` → the project file.
pub fn resolve_bytes(base: &Path, game_id: &str, id: &str) -> Result<Vec<u8>, String> {
    if let Some(stem) = id.strip_prefix("custom:") {
        let d = dir(base, game_id);
        for ext in ["ttf", "otf"] {
            let p = d.join(format!("{stem}.{ext}"));
            if p.is_file() {
                return std::fs::read(&p).map_err(|e| format!("read custom typeface {stem}: {e}"));
            }
        }
        Err(format!("custom typeface \"{stem}\" not found"))
    } else {
        font::typeface_bytes(id).map(|b| b.to_vec())
    }
}

/// Import a `.ttf`/`.otf` from `src_path` into the project (validating it loads). Returns its id.
pub fn import(base: &Path, game_id: &str, src_path: &Path) -> Result<FontTypeface, String> {
    let bytes = std::fs::read(src_path).map_err(|e| format!("read {}: {e}", src_path.display()))?;
    ab_glyph::FontRef::try_from_slice(&bytes)
        .map_err(|_| "not a valid TrueType/OpenType font".to_string())?;
    let ext = match src_path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref() {
        Some("otf") => "otf",
        _ => "ttf",
    };
    let stem = slug(src_path.file_stem().and_then(|s| s.to_str()).unwrap_or("font"));
    let d = dir(base, game_id);
    std::fs::create_dir_all(&d).map_err(|e| format!("create typefaces dir: {e}"))?;
    std::fs::write(d.join(format!("{stem}.{ext}")), &bytes).map_err(|e| format!("write typeface: {e}"))?;
    Ok(FontTypeface { id: format!("custom:{stem}"), name: stem, custom: true })
}

/// Delete a project-imported face (bundled ids are ignored).
pub fn remove(base: &Path, game_id: &str, id: &str) -> Result<(), String> {
    if let Some(stem) = id.strip_prefix("custom:") {
        for ext in ["ttf", "otf"] {
            let p = dir(base, game_id).join(format!("{stem}.{ext}"));
            if p.is_file() {
                std::fs::remove_file(&p).map_err(|e| format!("remove typeface: {e}"))?;
            }
        }
    }
    Ok(())
}
