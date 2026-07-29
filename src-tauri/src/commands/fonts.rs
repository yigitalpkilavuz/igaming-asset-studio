//! Font commands: list the bundled typefaces and render a live preview of a `FontDef`. Fonts
//! themselves are baked deterministically at export (`export::export_font`); these just power the
//! Fonts studio UI (no persistence, no provider).

use base64::Engine;

use crate::model::game_config::FontDef;
use crate::processing::font;

/// A bundled typeface the producer can pick for a font.
#[derive(serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FontTypeface {
    pub id: String,
    pub name: String,
}

/// The typefaces bundled with the app (OFL / Apache, redistributable in exports).
#[tauri::command]
#[specta::specta]
pub fn list_font_typefaces() -> Vec<FontTypeface> {
    font::typefaces()
        .into_iter()
        .map(|t| FontTypeface { id: t.id.into(), name: t.name.into() })
        .collect()
}

/// Rasterize `sample` in the given font → a PNG `data:` URL for the live preview.
#[tauri::command]
#[specta::specta]
pub fn preview_font(def: FontDef, sample: String) -> Result<String, String> {
    let text = if sample.trim().is_empty() {
        "WIN $1,234.56  x25".to_string()
    } else {
        sample
    };
    let png = font::preview_png(&def, &text)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    ))
}
