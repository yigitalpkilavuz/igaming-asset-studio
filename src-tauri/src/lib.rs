pub mod assistant;
pub mod commands;
pub mod export;
pub mod glyphs;
pub mod layers;
pub mod model;
pub mod processing;
pub mod prompts;
pub mod providers;
pub mod secrets;
pub mod settings;
pub mod storage;
pub mod studio;
pub mod taxonomy;
pub mod typefaces;

use tauri_specta::{collect_commands, Builder};

/// Trivial round-trip command used to prove the IPC + typed-bindings wiring (M0).
#[tauri::command]
#[specta::specta]
fn ping() -> String {
    "pong".to_string()
}

/// The single source of truth for the command surface. Both `run()` (to serve the
/// app) and the `export_bindings` test (to emit `src/lib/ipc/bindings.ts`) build from
/// this, so the TS bindings can never drift from the registered commands.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        ping,
        commands::project::derive_assets,
        commands::project::create_project,
        commands::project::delete_project,
        commands::project::save_project_config,
        commands::project::get_project,
        commands::project::list_projects,
        commands::project::projects_root_path,
        commands::prompts::set_openai_key,
        commands::prompts::openai_key_present,
        commands::prompts::set_gemini_key,
        commands::prompts::gemini_key_present,
        commands::prompts::set_spritecook_key,
        commands::prompts::spritecook_key_present,
        commands::prompts::set_gamelab_key,
        commands::prompts::gamelab_key_present,
        commands::sheet::generate_ai_sheet,
        commands::sheet::get_ai_sheet,
        commands::prompts::get_asset_record,
        commands::prompts::list_asset_records,
        commands::prompts::save_prompt,
        commands::prompts::assemble_prompt,
        commands::prompts::seed_subjects,
        commands::assistant::assistant_chat,
        commands::assistant::assistant_chat_stream,
        commands::assistant::assistant_fill_config,
        commands::assistant::config_draft_prompt,
        commands::assistant::apply_config_json,
        commands::generate::get_settings,
        commands::generate::save_settings,
        commands::generate::list_image_providers,
        commands::generate::generate_variation,
        commands::audio::list_audio_providers,
        commands::audio::generate_audio_variation,
        commands::audio::process_audio_variation,
        commands::audio::get_variation_audio,
        commands::audio::set_stability_key,
        commands::audio::stability_key_present,
        commands::audio::set_vertex_token,
        commands::audio::vertex_token_present,
        commands::fonts::list_font_typefaces,
        commands::fonts::preview_font,
        commands::fonts::import_typeface,
        commands::fonts::remove_typeface,
        commands::generate::plan_symbol_set,
        commands::generate::generate_symbol_set_sheet,
        commands::generate::import_symbol_set_sheet,
        commands::generate::list_symbol_set_sheets,
        commands::generate::select_symbol_set_sheet,
        commands::generate::adjust_symbol_set,
        commands::generate::commit_symbol_set,
        commands::generate::alpha_erase,
        commands::generate::generate_style_anchor,
        commands::generate::style_anchor_prompt,
        commands::generate::get_style_anchor,
        commands::generate::style_anchor_present,
        commands::generate::import_style_anchor,
        commands::generate::remove_style_anchor,
        commands::generate::import_source_image,
        commands::generate::upscale_variation,
        commands::generate::refine_variation,
        commands::generate::set_active_variation,
        commands::generate::delete_variation,
        commands::generate::set_variation_locked,
        commands::generate::set_template_only,
        commands::generate::get_variation_image,
        commands::quality::asset_quality,
        commands::quality::symbol_contact_sheet,
        commands::process::preflight,
        commands::process::process_variation,
        commands::process::get_variation_stage_image,
        commands::export::export_dist,
        commands::export::publish_finals,
        commands::studio::studio_open,
        commands::studio::studio_save,
        commands::studio::studio_reimport_source,
        commands::studio::studio_get_image,
        commands::studio::studio_preview_bundle,
        commands::studio::studio_skeleton_only,
        commands::studio::studio_export,
        commands::studio::studio_sam_status,
        commands::studio::studio_sam_download,
        commands::studio::studio_propose_parts,
        commands::studio::studio_auto_cut,
        commands::studio::studio_autocut_undo_available,
        commands::studio::studio_undo_auto_cut,
        commands::studio::studio_segment,
        commands::studio::studio_cloud_cut,
        commands::studio::studio_set_mask,
        commands::studio::studio_cut_parts,
        commands::studio::studio_auto_rig,
        commands::studio::studio_inpaint_status,
        commands::studio::studio_inpaint_part,
        commands::studio::studio_inpaint_all,
        commands::studio::studio_ai_draft_clip,
        commands::studio::studio_ai_polish_clip,
        commands::studio::studio_ai_inbetween,
        commands::studio::studio_set_mesh,
        commands::studio::studio_bake_turn,
        commands::studio::studio_save_spritesheet,
        commands::studio::studio_generate_fx,
        commands::studio::studio_add_alternate,
        commands::studio::studio_generate_alternate,
        commands::studio::studio_remove_alternate,
        commands::layers::layers_open,
        commands::layers::layers_save,
        commands::layers::layers_reimport_source,
        commands::layers::layers_get_image,
        commands::layers::layers_propose,
        commands::layers::layers_depth_status,
        commands::layers::layers_depth_download,
        commands::layers::layers_propose_depth,
        commands::layers::layers_segment,
        commands::layers::layers_cloud_cut,
        commands::layers::layers_set_mask,
        commands::layers::layers_trace_outline,
        commands::layers::layers_cut,
        commands::layers::layers_fill_status,
        commands::layers::layers_fill,
        commands::layers::layers_export,
        commands::glyphs::glyph_list,
        commands::glyphs::glyph_image,
        commands::glyphs::glyph_generate,
        commands::glyphs::glyph_import,
        commands::glyphs::glyph_delete,
        commands::glyphs::glyph_get,
        commands::glyphs::glyph_save,
        commands::glyphs::glyph_preview,
        commands::glyphs::glyph_compose,
    ])
}

/// Path the generated TypeScript bindings are written to (relative to `src-tauri/`).
const BINDINGS_PATH: &str = "../src/lib/ipc/bindings.ts";

/// TS exporter config. Timestamps are `f64` (epoch millis, exact well past 2100), so
/// the default exporter needs no BigInt handling.
fn ts_exporter() -> specta_typescript::Typescript {
    specta_typescript::Typescript::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // In dev, regenerate the TS bindings on every launch so the frontend types
    // track the Rust command surface automatically.
    #[cfg(debug_assertions)]
    builder
        .export(ts_exporter(), BINDINGS_PATH)
        .expect("failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{specta_builder, ts_exporter, BINDINGS_PATH};

    /// Headless bindings generation — lets `cargo test` produce/verify
    /// `src/lib/ipc/bindings.ts` without launching a GUI window.
    #[test]
    fn export_bindings() {
        specta_builder()
            .export(ts_exporter(), BINDINGS_PATH)
            .expect("failed to export typescript bindings");
    }
}
