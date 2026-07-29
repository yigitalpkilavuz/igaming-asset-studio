//! Font commands: list typefaces (bundled + this project's imported faces), preview a `FontDef`,
//! and import / remove a custom TTF. Fonts themselves bake deterministically at export
//! (`export::export_font`); these power the Fonts studio UI.

use std::path::Path;

use base64::Engine;

use super::projects_root;
use crate::model::game_config::FontDef;
use crate::processing::font;
use crate::typefaces::{self, FontTypeface};

/// Bundled display faces + the game's imported custom faces.
#[tauri::command]
#[specta::specta]
pub fn list_font_typefaces(app: tauri::AppHandle, game_id: String) -> Result<Vec<FontTypeface>, String> {
    Ok(typefaces::list(&projects_root(&app)?, &game_id))
}

/// Rasterize `sample` in the given font → a PNG `data:` URL for the live preview.
#[tauri::command]
#[specta::specta]
pub fn preview_font(
    app: tauri::AppHandle,
    game_id: String,
    def: FontDef,
    sample: String,
) -> Result<String, String> {
    let ttf = typefaces::resolve_bytes(&projects_root(&app)?, &game_id, &def.typeface)?;
    let text = if sample.trim().is_empty() {
        "WIN $1,234.56  x25".to_string()
    } else {
        sample
    };
    let png = font::preview_png(&def, &text, &ttf)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    ))
}

/// Import a `.ttf`/`.otf` file into the project as a custom typeface. Returns the new face.
#[tauri::command]
#[specta::specta]
pub fn import_typeface(
    app: tauri::AppHandle,
    game_id: String,
    src_path: String,
) -> Result<FontTypeface, String> {
    typefaces::import(&projects_root(&app)?, &game_id, Path::new(&src_path))
}

/// Remove a project-imported custom typeface (bundled faces are ignored).
#[tauri::command]
#[specta::specta]
pub fn remove_typeface(app: tauri::AppHandle, game_id: String, id: String) -> Result<(), String> {
    typefaces::remove(&projects_root(&app)?, &game_id, &id)
}
