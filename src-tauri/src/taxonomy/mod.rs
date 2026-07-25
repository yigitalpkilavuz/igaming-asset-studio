//! Asset taxonomy engine: derive the full required-asset list from a `GameConfig`.
//! A direct, config-driven encoding of ASSETS.md's core categories — nothing here is
//! specific to any single game.

pub mod dimensions;

use crate::model::asset::{AssetDescriptor, AssetKind, Format, Insets, Production};
use crate::model::game_config::{GameConfig, WinType};

use dimensions::{
    symbol_author, win_highlight_author, CANVAS_LANDSCAPE, CANVAS_PORTRAIT, MASCOT_AUTHOR,
};

/// Formats for a standard raster asset.
fn raster_formats() -> Vec<Format> {
    vec![Format::Webp, Format::Png]
}

/// Formats for a 9-slice raster asset (adds the `.9.json` sidecar).
fn nine_slice_formats() -> Vec<Format> {
    vec![Format::Webp, Format::Png, Format::NineSliceJson]
}

/// Formats for a full-canvas photographic asset (WebP + JPG twin, ASSETS.md §3).
fn canvas_formats() -> Vec<Format> {
    vec![Format::Webp, Format::Jpg]
}

/// Small builder to keep the rules below readable.
struct Rule;

impl Rule {
    #[allow(clippy::too_many_arguments)]
    fn make(
        key: impl Into<String>,
        kind: AssetKind,
        category: &str,
        spec_ref: &str,
        production: Production,
        dims: Option<(u32, u32)>,
        formats: Vec<Format>,
        nine_slice: Option<Insets>,
        required: bool,
        description: impl Into<String>,
    ) -> AssetDescriptor {
        let (author_w, author_h) = match dims {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };
        AssetDescriptor {
            key: key.into(),
            kind,
            category: category.to_string(),
            spec_ref: spec_ref.to_string(),
            production,
            author_w,
            author_h,
            formats,
            nine_slice,
            required,
            description: description.into(),
        }
    }
}

/// Derive the required-asset list for a game config. Grouped by ASSETS.md section.
pub fn derive_assets(config: &GameConfig) -> Vec<AssetDescriptor> {
    let mut out: Vec<AssetDescriptor> = Vec::new();
    let (cols, rows) = (config.cols, config.rows);

    // ── §2 Symbols ────────────────────────────────────────────────────────────
    for sym in &config.symbols {
        // Every symbol — wild/scatter included — ships square (1×1 cell). The spec ALLOWS
        // wilds/scatters to break out to 1.30×1.15 for bleed, but the studio prefers them
        // square and grid-aligned; `symbol_oversize_author` stays available if that changes.
        let dims = symbol_author(cols, rows);
        let desc = if sym.description.trim().is_empty() {
            sym.name.clone()
        } else {
            sym.description.clone()
        };
        out.push(Rule::make(
            format!("symbol_{}", sym.key),
            AssetKind::Symbol,
            "symbols",
            "§2",
            Production::Raster,
            Some(dims),
            raster_formats(),
            None,
            true,
            desc.clone(),
        ));
        // An expanding wild is TWO assets: the normal cell symbol above, plus the
        // full-column art it expands into (cell width × rows tall; the fit pass and
        // the per-cell composition clause don't apply to the twin).
        if sym.role == crate::model::game_config::SymbolRole::ExpandingWild {
            out.push(Rule::make(
                format!("symbol_{}_expanded", sym.key),
                AssetKind::Symbol,
                "symbols",
                "§2",
                Production::Raster,
                Some((dims.0, dims.1 * rows.max(1))),
                raster_formats(),
                None,
                true,
                format!(
                    "{} — its EXPANDED state, grown to fill the entire reel column",
                    desc.trim_end_matches('.')
                ),
            ));
        }
    }

    // ── §3 Backgrounds ────────────────────────────────────────────────────────
    // When the game defines scene PLATES, they take over as the backgrounds — the
    // stock §3 set would double up in the ledger and the delivery checklist.
    let has_scene_plates = config
        .scene
        .assets
        .iter()
        .any(|a| a.kind == crate::model::game_config::SceneKind::Plate);
    if !has_scene_plates {
    out.push(Rule::make(
        "bg_base_landscape",
        AssetKind::Background,
        "backgrounds",
        "§3",
        Production::Raster,
        Some(CANVAS_LANDSCAPE),
        canvas_formats(),
        None,
        true,
        "Base-game background, landscape.",
    ));
    out.push(Rule::make(
        "bg_base_portrait",
        AssetKind::Background,
        "backgrounds",
        "§3",
        Production::Raster,
        Some(CANVAS_PORTRAIT),
        canvas_formats(),
        None,
        true,
        "Base-game background, portrait.",
    ));
    if config.has_feature_background {
        out.push(Rule::make(
            "bg_feature_landscape",
            AssetKind::Background,
            "backgrounds",
            "§3",
            Production::Raster,
            Some(CANVAS_LANDSCAPE),
            canvas_formats(),
            None,
            true,
            "Feature / free-spins background, landscape.",
        ));
        out.push(Rule::make(
            "bg_feature_portrait",
            AssetKind::Background,
            "backgrounds",
            "§3",
            Production::Raster,
            Some(CANVAS_PORTRAIT),
            canvas_formats(),
            None,
            true,
            "Feature / free-spins background, portrait.",
        ));
    }
    } // end !has_scene_plates

    // ── Scene assets (Part I): plates / layers / set-piece sprites ───────────
    for sa in &config.scene.assets {
        let shared = if sa.description.trim().is_empty() {
            sa.name.clone()
        } else {
            sa.description.clone()
        };
        // No variants declared → the asset itself is the single variant.
        let single = [crate::model::game_config::SceneVariantDef {
            key: String::new(),
            preset: String::new(),
            extra_prompt: String::new(),
            placement: None,
        }];
        let variants: &[_] = if sa.variants.is_empty() { &single } else { &sa.variants };
        // Class prefixes (bg_/fx_/p_) apply automatically so exports match the
        // game-side naming contract without hand-typed prefixes.
        let base_key = crate::model::game_config::SceneConfig::prefixed_key(sa);
        for v in variants {
            let key = if v.key.trim().is_empty() {
                base_key.clone()
            } else {
                format!("{base_key}_{}", v.key.trim())
            };
            use crate::model::game_config::SceneKind;
            let dims = match sa.kind {
                // Sprites are any-aspect; without a preset they get a square canvas.
                SceneKind::Sprite if v.preset.trim().is_empty() => (1024, 1024),
                // Effects author small (they become ≤512² sheet frames)…
                SceneKind::Fx if v.preset.trim().is_empty() => (512, 512),
                // …particles tiny (exact-size export; generation upscales internally).
                SceneKind::Particle if v.preset.trim().is_empty() => (256, 256),
                _ => config.scene.preset_dims(&v.preset),
            };
            let (kind, formats) = match sa.kind {
                SceneKind::Plate => (AssetKind::Background, canvas_formats()),
                SceneKind::Layer => (AssetKind::SceneLayer, raster_formats()),
                SceneKind::Sprite | SceneKind::Fx | SceneKind::Particle => {
                    (AssetKind::SceneSprite, raster_formats())
                }
            };
            // The variant's prompt addition rides in the seeded description (the bench
            // subject falls back to it; a hand-typed subject replaces the whole thing).
            let desc = if v.extra_prompt.trim().is_empty() {
                shared.clone()
            } else {
                format!("{}. {}", shared.trim_end_matches('.'), v.extra_prompt.trim())
            };
            out.push(Rule::make(
                key,
                kind,
                "scenes",
                "scene",
                Production::Raster,
                Some(dims),
                formats,
                None,
                true,
                desc,
            ));
        }
    }

    // ── §4 Reel chrome ────────────────────────────────────────────────────────
    out.push(Rule::make(
        "reel_background",
        AssetKind::ReelChrome,
        "reel_chrome",
        "§4",
        Production::Raster,
        Some((1024, 1024)),
        nine_slice_formats(),
        Some(Insets::uniform(96)),
        true,
        "9-slice panel behind the symbols.",
    ));
    out.push(Rule::make(
        "reel_frame",
        AssetKind::ReelChrome,
        "reel_chrome",
        "§4",
        Production::Raster,
        Some((1024, 1024)),
        nine_slice_formats(),
        Some(Insets::uniform(128)),
        true,
        "9-slice ornamental frame around the reels.",
    ));
    out.push(Rule::make(
        "reel_anticipation_glow",
        AssetKind::ReelChrome,
        "reel_chrome",
        "§4",
        Production::Raster,
        Some((400, 1200)),
        nine_slice_formats(),
        Some(Insets {
            top: 128,
            right: 64,
            bottom: 128,
            left: 64,
        }),
        false,
        "Full-reel glow when a trigger symbol is about to reveal.",
    ));

    // ── §5 Symbol chrome ──────────────────────────────────────────────────────
    out.push(Rule::make(
        "symbol_win_highlight",
        AssetKind::SymbolChrome,
        "symbol_chrome",
        "§5",
        Production::Procedural,
        Some(win_highlight_author(cols, rows)),
        vec![],
        None,
        false,
        "Glow behind winning symbols — procedural (PIXI filter) by default.",
    ));
    out.push(Rule::make(
        "symbol_frame",
        AssetKind::SymbolChrome,
        "symbol_chrome",
        "§5",
        Production::Raster,
        Some(symbol_author(cols, rows)),
        raster_formats(),
        None,
        false,
        "Optional static frame drawn behind each symbol cell.",
    ));
    if config.has_mystery {
        out.push(Rule::make(
            "modifier_mystery",
            AssetKind::SymbolChrome,
            "symbol_chrome",
            "§5",
            Production::Raster,
            Some(symbol_author(cols, rows)),
            raster_formats(),
            None,
            true,
            "Closed mystery-symbol cover.",
        ));
    }
    if config.hold_and_spin {
        out.push(Rule::make(
            "symbol_hold_indicator",
            AssetKind::SymbolChrome,
            "symbol_chrome",
            "§5",
            Production::Raster,
            Some(symbol_author(cols, rows)),
            nine_slice_formats(),
            Some(Insets::uniform(48)),
            true,
            "Hold-and-spin lock indicator.",
        ));
    }

    // ── §10 Mascot (feature hero): the on-screen character beside the reels ──
    if config.has_mascot {
        let subject = config.mascot_description.trim();
        out.push(Rule::make(
            "mascot_hero",
            AssetKind::Mascot,
            "mascot",
            "§10",
            Production::Raster,
            Some(MASCOT_AUTHOR),
            raster_formats(),
            None,
            true,
            if subject.is_empty() {
                "The game's mascot character — full body, expressive, ready to be rigged."
                    .to_string()
            } else {
                subject.to_string()
            },
        ));
    }

    // ── §9 Panels ─────────────────────────────────────────────────────────────
    let panels: &[(&str, (u32, u32), u32, &str)] = &[
        (
            "panel_freespin_counter",
            (512, 256),
            64,
            "Free-spins counter frame.",
        ),
        ("panel_win_counter", (512, 256), 64, "Win counter frame."),
        ("panel_multiplier", (256, 256), 48, "Multiplier readout frame."),
        ("panel_total_win", (1024, 400), 128, "Total-win hero frame."),
        ("panel_info", (1024, 1024), 128, "Paytable / info panel."),
    ];
    for (key, dims, inset, desc) in panels {
        out.push(Rule::make(
            *key,
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some(*dims),
            nine_slice_formats(),
            Some(Insets::uniform(*inset)),
            true,
            *desc,
        ));
    }
    if config.has_buy_bonus {
        out.push(Rule::make(
            "button_buy_bonus",
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some((400, 200)),
            nine_slice_formats(),
            Some(Insets::uniform(64)),
            true,
            "Buy-bonus button chrome.",
        ));
    }

    // ── §9 Bonus meter (feature-collection mechanics) ─────────────────────────
    if config.has_meter {
        let meter_hz = Insets {
            top: 0,
            right: 64,
            bottom: 0,
            left: 64,
        };
        out.push(Rule::make(
            "meter_track",
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some((2048, 128)),
            nine_slice_formats(),
            Some(meter_hz),
            true,
            "Bonus meter track (9-slice horizontal).",
        ));
        out.push(Rule::make(
            "meter_fill",
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some((2048, 128)),
            nine_slice_formats(),
            Some(meter_hz),
            true,
            "Bonus meter fill overlay (9-slice horizontal).",
        ));
        out.push(Rule::make(
            "meter_frame",
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some((2048, 200)),
            nine_slice_formats(),
            Some(meter_hz),
            false,
            "Optional meter frame.",
        ));
        out.push(Rule::make(
            "meter_tick",
            AssetKind::Panel,
            "panels",
            "§9",
            Production::Raster,
            Some((64, 128)),
            raster_formats(),
            None,
            true,
            "Meter milestone tick.",
        ));
        for n in 1..=config.meter_thresholds {
            out.push(Rule::make(
                format!("meter_reward_icon_{n}"),
                AssetKind::Panel,
                "panels",
                "§9",
                Production::Raster,
                Some((128, 128)),
                raster_formats(),
                None,
                true,
                format!("Meter reward icon for threshold {n}."),
            ));
        }
    }

    // ── §9 Buy-bonus selector (two or more buy-bonus modes) ───────────────────
    if config.buy_bonus_modes.len() >= 2 {
        out.push(Rule::make(
            "buy_bonus_selector_bg_landscape",
            AssetKind::Panel,
            "buy_bonus",
            "§9",
            Production::Raster,
            Some(CANVAS_LANDSCAPE),
            raster_formats(),
            None,
            true,
            "Buy-bonus selector overlay background, landscape.",
        ));
        out.push(Rule::make(
            "buy_bonus_selector_bg_portrait",
            AssetKind::Panel,
            "buy_bonus",
            "§9",
            Production::Raster,
            Some(CANVAS_PORTRAIT),
            raster_formats(),
            None,
            true,
            "Buy-bonus selector overlay background, portrait.",
        ));
        out.push(Rule::make(
            "buy_bonus_card_frame",
            AssetKind::Panel,
            "buy_bonus",
            "§9",
            Production::Raster,
            Some((480, 720)),
            nine_slice_formats(),
            Some(Insets::uniform(96)),
            true,
            "Buy-bonus card frame (9-slice).",
        ));
        out.push(Rule::make(
            "buy_bonus_card_hover_glow",
            AssetKind::Panel,
            "buy_bonus",
            "§9",
            Production::Raster,
            Some((480, 720)),
            raster_formats(),
            None,
            false,
            "Optional buy-bonus card hover glow.",
        ));
        for mode in &config.buy_bonus_modes {
            out.push(Rule::make(
                format!("buy_bonus_card_{}", mode.key),
                AssetKind::Panel,
                "buy_bonus",
                "§9",
                Production::Raster,
                Some((400, 600)),
                raster_formats(),
                None,
                true,
                format!("Buy-bonus card for the \"{}\" mode.", mode.name),
            ));
        }
    }

    // ── §8 Payline / win indicators (win-type conditional) ────────────────────
    match config.win_type {
        WinType::Lines => {
            out.push(Rule::make(
                "payline_overlay",
                AssetKind::Payline,
                "payline",
                "§8",
                Production::Procedural,
                None,
                vec![],
                None,
                false,
                "Payline overlays — drawn with PIXI.Graphics.",
            ));
            out.push(Rule::make(
                "payline_number_tags",
                AssetKind::Payline,
                "payline",
                "§8",
                Production::RuntimeFont,
                Some((64, 64)),
                vec![],
                None,
                false,
                "Payline number tags — BitmapText at runtime.",
            ));
        }
        WinType::Ways => {
            out.push(Rule::make(
                "way_indicator",
                AssetKind::Payline,
                "payline",
                "§8",
                Production::Raster,
                Some((512, 128)),
                nine_slice_formats(),
                Some(Insets {
                    top: 0,
                    right: 128,
                    bottom: 0,
                    left: 128,
                }),
                true,
                "\"N WAYS\" indicator panel.",
            ));
        }
        WinType::Cluster => {
            out.push(Rule::make(
                "cluster_win_outline",
                AssetKind::Payline,
                "payline",
                "§8",
                Production::Procedural,
                None,
                vec![],
                None,
                false,
                "Cluster win outline — runtime border trace.",
            ));
        }
        WinType::Scatter => {}
    }

    // ── §10 Branding + splash ─────────────────────────────────────────────────
    out.push(Rule::make(
        "game_logo",
        AssetKind::Branding,
        "branding",
        "§10",
        Production::Raster,
        Some((2048, 800)),
        vec![Format::Png],
        None,
        true,
        "Game logo / wordmark master (transparent).",
    ));
    out.push(Rule::make(
        "splash_hero_landscape",
        AssetKind::Splash,
        "splash",
        "§10",
        Production::Raster,
        Some(CANVAS_LANDSCAPE),
        canvas_formats(),
        None,
        true,
        "Splash hero art, landscape.",
    ));
    out.push(Rule::make(
        "splash_hero_portrait",
        AssetKind::Splash,
        "splash",
        "§10",
        Production::Raster,
        Some(CANVAS_PORTRAIT),
        canvas_formats(),
        None,
        true,
        "Splash hero art, portrait.",
    ));

    out
}

#[cfg(test)]
mod tests;
