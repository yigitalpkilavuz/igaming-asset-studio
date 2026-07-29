//! Deterministic prompt assembly: style master + composition template + subject.
//! No LLM. The studio owns the aesthetic; assets only supply a subject.

use crate::model::asset::{AssetDescriptor, AssetKind};
use crate::model::asset_record::PromptState;
use crate::model::game_config::{GameConfig, SymbolRole};

use super::templates::{
    background_composition, category_negative, composition_override, composition_template,
    cutout_clause, key_negative, parallax_layers_clause, scene_fx_composition,
    scene_layer_composition, scene_layer_negative, scene_particle_composition,
    scene_sprite_composition, special_symbol_clause, symbol_fill_clause,
};

/// The assembled prompt actually sent to a provider.
pub struct Assembled {
    pub positive: String,
    pub negative: String,
}

/// Assemble the final (positive, negative) prompt for an asset.
pub fn assemble(config: &GameConfig, descriptor: &AssetDescriptor, prompt: &PromptState) -> Assembled {
    let positive = if !prompt.override_prompt.trim().is_empty() {
        prompt.override_prompt.trim().to_string()
    } else {
        let subject = effective_subject(config, descriptor, prompt);
        // Backgrounds (stock §3 AND scene plates) get an orientation/feature-aware,
        // reels-safe composition; scene layers/sprites have kind-level templates; a few
        // keys override their category template (a filled reel panel is not a hollow
        // frame); everything else uses its per-category template.
        let is_plate = descriptor.category == "backgrounds"
            || (descriptor.category == "scenes" && descriptor.kind == AssetKind::Background);
        let mut composed = if is_plate {
            let key = &descriptor.key;
            // Scene plates carry their preset dims — trust those over key naming
            // (square or missing dims fall back to the key).
            let portrait = match (descriptor.author_w, descriptor.author_h) {
                (Some(w), Some(h)) if h != w => h > w,
                _ => key.contains("portrait"),
            };
            let mut bg = background_composition(&subject, portrait, key.contains("feature"));
            // Destined for the parallax cut → ask for cleanly separable depth planes.
            if prompt.layered {
                bg.push_str(parallax_layers_clause());
            }
            bg
        } else if descriptor.kind == AssetKind::SceneLayer {
            scene_layer_composition().replace("{subject}", &subject)
        } else if descriptor.kind == AssetKind::SceneSprite {
            // Fx and particle scene classes share the sprite ASSET kind but want their
            // own compositions — resolve the declared class from the scene config.
            use crate::model::game_config::SceneKind;
            let template = match config.scene.def_for_key(&descriptor.key).map(|d| d.kind) {
                Some(SceneKind::Fx) => scene_fx_composition(),
                Some(SceneKind::Particle) => scene_particle_composition(),
                _ => scene_sprite_composition(),
            };
            template.replace("{subject}", &subject)
        } else if let Some(template) = composition_override(&descriptor.key) {
            template.replace("{subject}", &subject)
        } else {
            composition_template(&descriptor.category).replace("{subject}", &subject)
        };

        // Symbols carry a `{fill}` slot: how much of the frame the subject fills, by tier
        // (high ~85%, low ~60%). Resolve it from the symbol's role; fall back to the hero
        // fraction for anything without a matching config symbol.
        if descriptor.kind == AssetKind::Symbol {
            let fill = if config.symbol_for_asset(&descriptor.key).is_some_and(|(_, exp)| exp) {
                // Expanded state of an expanding wild: the composition must OWN the
                // tall frame, not float a small emblem in it.
                "a tall FULL-HEIGHT composition that fills the frame vertically from top edge to bottom edge — designed to cover an entire reel column as one continuous piece, with only slim side margins"
            } else {
                let role = symbol_role_and_name(config, descriptor).map(|(r, _)| r);
                symbol_fill_clause(role.unwrap_or(SymbolRole::High))
            };
            composed = composed.replace("{fill}", fill);
        }

        // Special symbols (wild/scatter/bonus/special) get a role-specific addendum:
        // premium framing. Feature words are NEVER lettered into the art (localization —
        // the game typesets them onto the separate `_banner` asset at runtime).
        if descriptor.kind == AssetKind::Symbol {
            if let Some((role, name)) = symbol_role_and_name(config, descriptor) {
                if let Some(clause) = special_symbol_clause(role, &name) {
                    composed.push_str(&clause);
                }
            }
        }

        // Interior cut-outs: openings render as flat chroma fills; Process keys them
        // to real transparency (chroma keying works anywhere, not just the border).
        if descriptor.category == "scenes" {
            if let Some(def) = config.scene.def_for_key(&descriptor.key) {
                if def.cutouts {
                    composed.push_str(cutout_clause());
                }
                if def.wrap {
                    composed.push_str(super::templates::tileable_clause());
                }
            }
        }

        let style = config.style_prompt.trim();
        if style.is_empty() {
            composed
        } else {
            format!("{}. {}", style.trim_end_matches('.'), composed)
        }
    };

    // The studio anti-slop floor is ALWAYS applied; the per-asset override (or, failing
    // that, the game's negative master) is appended on top of it. For a labelled special
    // symbol the "caption" suppressor is dropped so its feature word can render.
    let extra = if !prompt.negative_override.trim().is_empty() {
        prompt.negative_override.trim()
    } else {
        config.negative_prompt.trim()
    };
    let is_plate = descriptor.category == "backgrounds"
        || (descriptor.category == "scenes" && descriptor.kind == AssetKind::Background);
    let mut floor = if is_plate {
        category_negative("backgrounds")
    } else if descriptor.kind == AssetKind::SceneLayer {
        scene_layer_negative()
    } else if descriptor.kind == AssetKind::SceneSprite {
        category_negative("symbols") // the isolated-object floor fits a set-piece
    } else {
        category_negative(&descriptor.category)
    };
    if prompt.layered && is_plate {
        // Blur and cross-plane atmosphere are what ruin a parallax cut.
        floor.push_str(", depth of field, bokeh, soft focus, motion blur, atmospheric haze blending distant and near elements together");
    }
    if let Some(extra) = key_negative(&descriptor.key) {
        floor.push_str(extra);
    }
    let negative = if extra.is_empty() {
        floor
    } else {
        format!("{floor}, {extra}")
    };

    Assembled { positive, negative }
}

/// The (role, name) of the config symbol behind a `symbol_*` descriptor, if any.
fn symbol_role_and_name(config: &GameConfig, descriptor: &AssetDescriptor) -> Option<(SymbolRole, String)> {
    // Resolves the `_expanded` twin of an expanding wild to its base symbol, so the
    // role addendum (WILD banner, premium framing) applies to both states.
    let (sym, _) = config.symbol_for_asset(&descriptor.key)?;
    Some((sym.role, sym.name.clone()))
}

/// The subject to use: the explicit one, else a sensible seed from the config/descriptor.
fn effective_subject(config: &GameConfig, descriptor: &AssetDescriptor, prompt: &PromptState) -> String {
    if !prompt.subject.trim().is_empty() {
        return prompt.subject.trim().to_string();
    }
    subject_seed(config, descriptor)
}

/// A deterministic starting subject for an asset: for symbols, the symbol's own
/// description (or name) from the config; otherwise the descriptor's description.
pub fn subject_seed(config: &GameConfig, descriptor: &AssetDescriptor) -> String {
    if descriptor.kind == AssetKind::Symbol {
        if let Some(sym_key) = descriptor.key.strip_prefix("symbol_") {
            if let Some(sym) = config.symbols.iter().find(|s| s.key == sym_key) {
                let d = sym.description.trim();
                return if d.is_empty() { sym.name.trim().to_string() } else { d.to_string() };
            }
        }
    }
    descriptor.description.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::asset::{AssetKind, Production};
    use crate::model::game_config::{SymbolDef, SymbolRole, WinType};

    fn config() -> GameConfig {
        GameConfig {
            game_id: "g".into(),
            name: "G".into(),
            brief: String::new(),
            style_prompt: "illuminated medieval marginalia, iron-gall ink, aged parchment".into(),
            negative_prompt: String::new(),
            win_type: WinType::Scatter,
            cols: 6,
            rows: 5,
            symbols: vec![SymbolDef {
                key: "h1".into(),
                role: SymbolRole::High,
                name: "Snail-Knight".into(),
                description: "an armored knight with a spiral snail shell, lance and shield".into(),
                animation: String::new(),
            size_nudge: 1.0,
            tone_target: None,
        }],
            has_feature_background: true,
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

    fn symbol_descriptor() -> AssetDescriptor {
        AssetDescriptor {
            key: "symbol_h1".into(),
            kind: AssetKind::Symbol,
            category: "symbols".into(),
            spec_ref: "§2".into(),
            production: Production::Raster,
            author_w: Some(220),
            author_h: Some(220),
            formats: vec![],
            nine_slice: None,
            required: true,
            description: "H1".into(),
        }
    }

    #[test]
    fn scene_assets_get_kind_appropriate_compositions() {
        let cfg = config();
        let prompt = PromptState::default();

        // A scene PLATE (portrait preset dims, no "portrait" in the key) reads as a
        // portrait reels-safe background.
        let plate = AssetDescriptor {
            key: "bg_base_tower".into(),
            kind: AssetKind::Background,
            category: "scenes".into(),
            author_w: Some(1242),
            author_h: Some(2208),
            description: "a storm-lashed lighthouse interior".into(),
            ..symbol_descriptor()
        };
        let a = assemble(&cfg, &plate, &prompt);
        assert!(a.positive.contains("portrait"), "dims drive orientation");
        assert!(a.positive.to_lowercase().contains("reels"));
        assert!(!a.negative.contains("environment"), "scene plate gets the background floor");

        // A LAYER wants transparency + plate-matched perspective, and its negative must
        // not fight off-centre placement.
        let layer = AssetDescriptor {
            key: "window_storm_tempest".into(),
            kind: AssetKind::SceneLayer,
            category: "scenes".into(),
            description: "the storm sky seen inside the window arch".into(),
            ..symbol_descriptor()
        };
        let a = assemble(&cfg, &layer, &prompt);
        assert!(a.positive.contains("transparent background"));
        assert!(a.positive.contains("stack over"));
        assert!(!a.negative.contains("off-center"));
        assert!(a.negative.contains("opaque background"));

        // A SPRITE is a natural set-piece, not an emblem.
        let sprite = AssetDescriptor {
            key: "booth".into(),
            kind: AssetKind::SceneSprite,
            category: "scenes".into(),
            description: "an observation booth".into(),
            ..symbol_descriptor()
        };
        let a = assemble(&cfg, &sprite, &prompt);
        assert!(a.positive.contains("set-piece object"));
        assert!(a.positive.contains("not an emblem"));
    }

    #[test]
    fn reel_background_is_a_filled_panel_not_a_frame() {
        let cfg = config();
        let mk = |key: &str| AssetDescriptor {
            key: key.into(),
            kind: AssetKind::ReelChrome,
            category: "reel_chrome".into(),
            ..symbol_descriptor()
        };
        let prompt = PromptState { subject: "worn brass plating".into(), ..Default::default() };

        // The panel behind the symbols: filled edge to edge, framing forbidden.
        let bg = assemble(&cfg, &mk("reel_background"), &prompt);
        assert!(bg.positive.contains("backdrop panel"));
        assert!(bg.positive.contains("edge to edge"));
        assert!(!bg.positive.contains("hollow"), "panel must not ask for a hollow frame");
        assert!(bg.negative.contains("hollow centre"));

        // The frame keeps the hollow-border composition.
        let frame = assemble(&cfg, &mk("reel_frame"), &prompt);
        assert!(frame.positive.contains("hollow"));
        assert!(!frame.negative.contains("hollow centre"));

        // The anticipation glow is a light strip, not chrome.
        let glow = assemble(&cfg, &mk("reel_anticipation_glow"), &prompt);
        assert!(glow.positive.contains("glow overlay"));
        assert!(!glow.positive.contains("hollow"));
    }

    #[test]
    fn assembles_style_plus_composition_plus_seeded_subject() {
        let cfg = config();
        let d = symbol_descriptor();
        let a = assemble(&cfg, &d, &PromptState::default());
        // Style master leads.
        assert!(a.positive.starts_with("illuminated medieval marginalia"));
        // Symbol composition + the seeded subject from the config symbol description.
        assert!(a.positive.contains("slot-machine game symbol"));
        assert!(a.positive.contains("silhouette"));
        assert!(a.positive.contains("spiral snail shell"));
        assert!(a.positive.contains("plain empty background"));
        // Negative falls back to the studio floor.
        assert!(a.negative.contains("photorealistic"));
    }

    #[test]
    fn symbol_fill_fraction_tracks_tier() {
        let mut cfg = config();
        cfg.symbols.push(SymbolDef {
            key: "l1".into(),
            role: SymbolRole::Low,
            name: "Copper Coin".into(),
            description: "a worn copper coin".into(), animation: String::new(),
            size_nudge: 1.0,
            tone_target: None,
        });
        // High tier reads big (~85%); low tier reads smaller (~60%).
        let high = assemble(&cfg, &descriptor_for("symbol_h1"), &PromptState::default());
        assert!(high.positive.contains("roughly 85% of the frame"), "high → 85%");
        let low = assemble(&cfg, &descriptor_for("symbol_l1"), &PromptState::default());
        assert!(low.positive.contains("roughly 60% of the frame"), "low → 60%");
        // The placeholder is always resolved.
        assert!(!high.positive.contains("{fill}") && !low.positive.contains("{fill}"));
    }

    fn descriptor_for(key: &str) -> AssetDescriptor {
        AssetDescriptor { key: key.into(), ..symbol_descriptor() }
    }

    /// End-to-end guard for the bring-your-own-agent flow: a JSON blueprint pasted back from an
    /// external agent, applied via `apply_json`, must feed the deterministic assembly so the
    /// generated prompt carries the pasted style master + the pasted symbol art direction.
    #[test]
    fn pasted_blueprint_json_assembles_a_correct_prompt() {
        let base = config();
        let json = r#"{
            "name": "Ashen Court",
            "gameId": "ashen_court",
            "winType": "lines",
            "cols": 5, "rows": 3,
            "symbols": [
              { "key": "h1", "role": "high", "name": "Snail-Knight",
                "description": "an armored knight with a spiral snail shell and a lance" }
            ],
            "stylePrompt": "grim baroque woodcut, iron-gall ink, aged vellum"
        }"#;
        let cfg = crate::assistant::apply_json(&base, json).expect("apply pasted JSON");

        let a = assemble(&cfg, &descriptor_for("symbol_h1"), &PromptState::default());
        // Pasted style master leads the prompt.
        assert!(a.positive.starts_with("grim baroque woodcut"), "style master leads");
        // Pasted symbol art direction is the subject.
        assert!(a.positive.contains("spiral snail shell"), "symbol description flows in");
        // Composition scaffolding + tier fill are still applied (nothing left as a placeholder).
        assert!(a.positive.contains("slot-machine game symbol"));
        assert!(a.positive.contains("roughly 85% of the frame"), "high tier fill");
        assert!(!a.positive.contains("{fill}"));
    }

    #[test]
    fn wild_symbol_gets_no_lettering_and_keeps_text_suppressors() {
        // Localization: the feature word must NOT bake into the art — the game
        // typesets the translated word onto the separate `_banner` asset at runtime.
        let mut cfg = config();
        cfg.symbols.push(SymbolDef {
            key: "wild".into(),
            role: SymbolRole::Wild,
            name: "Anomaly".into(),
            description: "a glitching heraldic beast".into(), animation: String::new(),
            size_nudge: 1.0,
            tone_target: None,
        });
        let a = assemble(&cfg, &descriptor_for("symbol_wild"), &PromptState::default());
        assert!(a.positive.contains("special feature symbol"));
        assert!(!a.positive.contains("\"WILD\""), "must not letter the word into the art");
        assert!(a.positive.contains("Do NOT letter"), "explicit no-lettering order present");
        assert!(a.positive.contains("no watermark or caption"), "caption rule KEPT");
        assert!(a.negative.contains("caption"), "caption suppressor KEPT in negative");
    }

    #[test]
    fn scatter_gets_no_lettering_and_high_is_unchanged() {
        let mut cfg = config();
        cfg.symbols.push(SymbolDef {
            key: "scat".into(),
            role: SymbolRole::Scatter,
            name: "Reliquary".into(),
            description: "a radiant reliquary".into(), animation: String::new(),
            size_nudge: 1.0,
            tone_target: None,
        });
        let scatter = assemble(&cfg, &descriptor_for("symbol_scat"), &PromptState::default());
        assert!(!scatter.positive.contains("\"SCATTER\""), "word stays out of the art");
        assert!(scatter.positive.contains("special feature symbol"));
        // A High symbol gets no special clause and keeps the caption rule + negative.
        let high = assemble(&cfg, &descriptor_for("symbol_h1"), &PromptState::default());
        assert!(!high.positive.contains("special feature symbol"));
        assert!(high.positive.contains("no watermark or caption"));
        assert!(high.negative.contains("caption"));
    }

    #[test]
    fn background_is_reels_safe_and_not_fought_by_negatives() {
        let cfg = config();
        let bg = AssetDescriptor {
            key: "bg_base_landscape".into(),
            kind: AssetKind::Background,
            category: "backgrounds".into(),
            ..symbol_descriptor()
        };
        let a = assemble(&cfg, &bg, &PromptState::default());
        assert!(a.positive.contains("landscape"));
        assert!(a.positive.to_lowercase().contains("reels"));
        assert!(a.positive.contains("full-bleed"));
        // The scene-hostile negatives must NOT be applied to a background.
        for bad in ["environment", "horizon", "cluttered background", "landscape,"] {
            assert!(!a.negative.contains(bad), "bg negative should not contain \"{bad}\"");
        }
        // Feature portrait background reads as portrait + free-spins mood.
        let feat = AssetDescriptor {
            key: "bg_feature_portrait".into(),
            kind: AssetKind::Background,
            category: "backgrounds".into(),
            ..symbol_descriptor()
        };
        let fa = assemble(&cfg, &feat, &PromptState::default());
        assert!(fa.positive.contains("portrait"));
        assert!(fa.positive.contains("free-spins"));
    }

    #[test]
    fn layered_background_asks_for_separable_planes() {
        let cfg = config();
        let bg = AssetDescriptor {
            key: "bg_base_landscape".into(),
            kind: AssetKind::Background,
            category: "backgrounds".into(),
            ..symbol_descriptor()
        };
        let layered = PromptState { layered: true, ..Default::default() };
        let a = assemble(&cfg, &bg, &layered);
        assert!(a.positive.contains("parallax depth planes"), "positive gets the clause");
        assert!(a.negative.contains("depth of field"), "negative fights blur");

        // Off by default; and never applied to non-backgrounds even if the flag is set.
        let plain = assemble(&cfg, &bg, &PromptState::default());
        assert!(!plain.positive.contains("parallax depth planes"));
        assert!(!plain.negative.contains("depth of field"));
        let sym = assemble(&cfg, &symbol_descriptor(), &layered);
        assert!(!sym.positive.contains("parallax depth planes"));
    }

    #[test]
    fn override_bypasses_assembly() {
        let cfg = config();
        let d = symbol_descriptor();
        let p = PromptState {
            override_prompt: "exactly this".into(),
            ..Default::default()
        };
        let a = assemble(&cfg, &d, &p);
        assert_eq!(a.positive, "exactly this");
    }
}
