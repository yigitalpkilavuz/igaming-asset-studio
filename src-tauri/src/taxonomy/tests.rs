use super::derive_assets;
use crate::model::asset::{AssetDescriptor, AssetKind, Production};
use crate::model::game_config::{BuyBonusMode, GameConfig, SymbolDef, SymbolRole, WinType};

fn sym(key: &str, role: SymbolRole) -> SymbolDef {
    SymbolDef {
        key: key.to_string(),
        role,
        name: key.to_uppercase(),
        description: String::new(),
        animation: String::new(),
        size_nudge: 1.0,
    }
}

fn find<'a>(assets: &'a [AssetDescriptor], key: &str) -> Option<&'a AssetDescriptor> {
    assets.iter().find(|a| a.key == key)
}

fn has(assets: &[AssetDescriptor], key: &str) -> bool {
    find(assets, key).is_some()
}

/// Babewyn Court: 6×5 scatter-pays, buy-bonus, feature background.
fn babewyn_config() -> GameConfig {
    use SymbolRole::*;
    GameConfig {
        game_id: "babewyn_court".into(),
        name: "Babewyn Court".into(),
        brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
        win_type: WinType::Scatter,
        cols: 6,
        rows: 5,
        symbols: vec![
            sym("h1", High),
            sym("h2", High),
            sym("h3", High),
            sym("h4", High),
            sym("l1", Low),
            sym("l2", Low),
            sym("l3", Low),
            sym("l4", Low),
            sym("l5", Low),
            sym("wild", Wild),
            sym("scatter", Scatter),
            sym("ink_drop", Special),
        ],
        has_feature_background: true,
        has_buy_bonus: true,
        buy_bonus_modes: vec![BuyBonusMode {
            key: "bonus".into(),
            name: "Bonus".into(),
        }],
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

/// A generic 5×3 lines game.
fn lines_config() -> GameConfig {
    use SymbolRole::*;
    GameConfig {
        game_id: "demo_lines".into(),
        name: "Demo Lines".into(),
        brief: String::new(),
            style_prompt: String::new(),
            negative_prompt: String::new(),
        win_type: WinType::Lines,
        cols: 5,
        rows: 3,
        symbols: vec![
            sym("h1", High),
            sym("h2", High),
            sym("h3", High),
            sym("h4", High),
            sym("h5", High),
            sym("l1", Low),
            sym("l2", Low),
            sym("l3", Low),
            sym("l4", Low),
            sym("l5", Low),
            sym("wild", Wild),
            sym("scatter", Scatter),
        ],
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
            symbol_provider: String::new(),
            scene: Default::default(),
    }
}

#[test]
fn expanding_wild_derives_cell_symbol_plus_full_column_twin() {
    let mut cfg = babewyn_config();
    cfg.symbols.push(sym("mega", SymbolRole::ExpandingWild));

    let assets = derive_assets(&cfg);
    // The normal cell-sized state…
    let base = find(&assets, "symbol_mega").expect("symbol_mega");
    let h1 = find(&assets, "symbol_h1").expect("symbol_h1");
    assert_eq!((base.author_w, base.author_h), (h1.author_w, h1.author_h));
    // …plus the expanded full-column twin: same cell width, rows× the height.
    let col = find(&assets, "symbol_mega_expanded").expect("symbol_mega_expanded");
    assert_eq!(col.author_w, h1.author_w);
    assert_eq!(col.author_h, h1.author_h.map(|h| h * cfg.rows));
    assert_eq!(col.kind, AssetKind::Symbol);
    assert!(col.description.contains("EXPANDED"), "{}", col.description);
}

#[test]
fn babewyn_derivation() {
    let assets = derive_assets(&babewyn_config());

    // 12 symbols + 4 backgrounds + 3 reel_chrome + 2 symbol_chrome + 6 panels
    // + 0 payline (scatter) + 3 branding/splash = 30.
    assert_eq!(assets.len(), 30, "unexpected asset count");

    // Symbols present and sized from the 6×5 cell (110 GU -> 220 author px).
    let h1 = find(&assets, "symbol_h1").expect("symbol_h1");
    assert_eq!((h1.author_w, h1.author_h), (Some(220), Some(220)));

    // Wild/scatter ship square like every other symbol (110 GU cell -> 220 author px).
    let wild = find(&assets, "symbol_wild").expect("symbol_wild");
    assert_eq!((wild.author_w, wild.author_h), (Some(220), Some(220)));
    assert!(has(&assets, "symbol_scatter"));
    assert!(has(&assets, "symbol_ink_drop"));

    // Feature background + buy-bonus button are conditionally present.
    assert!(has(&assets, "bg_feature_landscape"));
    assert!(has(&assets, "button_buy_bonus"));

    // Scatter game: no lines/ways/cluster indicators.
    assert!(!has(&assets, "payline_overlay"));
    assert!(!has(&assets, "way_indicator"));
    assert!(!has(&assets, "cluster_win_outline"));

    // reel_frame is a 1024² 9-slice with 128 px insets.
    let frame = find(&assets, "reel_frame").expect("reel_frame");
    assert_eq!((frame.author_w, frame.author_h), (Some(1024), Some(1024)));
    assert_eq!(frame.nine_slice.unwrap().left, 128);
}

#[test]
fn lines_derivation() {
    let assets = derive_assets(&lines_config());

    // 12 symbols + 4 backgrounds + 3 reel_chrome + 2 symbol_chrome + 5 panels
    // + 2 payline (lines) + 3 branding/splash = 31.
    assert_eq!(assets.len(), 31, "unexpected asset count");

    // 5×3 cell is 140 GU -> 280 author px symbols; wild/scatter ship square too.
    let h1 = find(&assets, "symbol_h1").expect("symbol_h1");
    assert_eq!((h1.author_w, h1.author_h), (Some(280), Some(280)));
    let scatter = find(&assets, "symbol_scatter").expect("symbol_scatter");
    assert_eq!((scatter.author_w, scatter.author_h), (Some(280), Some(280)));

    // Lines game: payline assets present, no way/cluster/buy-bonus.
    assert!(has(&assets, "payline_overlay"));
    assert!(has(&assets, "payline_number_tags"));
    assert!(!has(&assets, "button_buy_bonus"));
    assert!(!has(&assets, "way_indicator"));
}

#[test]
fn meter_and_multi_buybonus_emit_their_assets() {
    use SymbolRole::*;
    let mut cfg = babewyn_config();
    cfg.has_meter = true;
    cfg.meter_thresholds = 3;
    cfg.buy_bonus_modes = vec![
        BuyBonusMode { key: "bonus".into(), name: "Bonus".into() },
        BuyBonusMode { key: "super".into(), name: "Super".into() },
    ];
    // keep at least one symbol so the base set is unchanged in spirit
    cfg.symbols = vec![sym("h1", High)];

    let assets = derive_assets(&cfg);

    // Meter block present, including one reward icon per threshold.
    assert!(has(&assets, "meter_track"));
    assert!(has(&assets, "meter_fill"));
    assert!(has(&assets, "meter_tick"));
    assert!(has(&assets, "meter_reward_icon_1"));
    assert!(has(&assets, "meter_reward_icon_3"));
    assert!(!has(&assets, "meter_reward_icon_4"));

    // Buy-bonus selector present with one card per mode.
    assert!(has(&assets, "buy_bonus_selector_bg_landscape"));
    assert!(has(&assets, "buy_bonus_card_frame"));
    assert!(has(&assets, "buy_bonus_card_bonus"));
    assert!(has(&assets, "buy_bonus_card_super"));
}

#[test]
fn mascot_flag_emits_the_hero_with_its_description() {
    let mut cfg = babewyn_config();
    assert!(!has(&derive_assets(&cfg), "mascot_hero"));

    cfg.has_mascot = true;
    cfg.mascot_description = "a gaunt plague-doctor raven with a lantern".into();
    let assets = derive_assets(&cfg);
    let m = assets.iter().find(|a| a.key == "mascot_hero").expect("mascot emitted");
    assert_eq!(m.kind, AssetKind::Mascot);
    assert_eq!(m.category, "mascot");
    assert_eq!(m.production, Production::Raster);
    assert_eq!((m.author_w, m.author_h), (Some(1200), Some(1800)));
    assert!(m.required);
    // The description IS the prompt subject (subject_seed passthrough).
    assert!(m.description.contains("plague-doctor raven"));

    // Empty description falls back to a usable default subject.
    cfg.mascot_description = "  ".into();
    let assets = derive_assets(&cfg);
    let m = assets.iter().find(|a| a.key == "mascot_hero").unwrap();
    assert!(m.description.contains("mascot character"));
}

#[test]
fn single_buybonus_mode_has_no_selector() {
    // Babewyn has one buy-bonus mode -> button yes, selector no.
    let assets = derive_assets(&babewyn_config());
    assert!(has(&assets, "button_buy_bonus"));
    assert!(!has(&assets, "buy_bonus_selector_bg_landscape"));
    assert!(!has(&assets, "meter_track"));
}

#[test]
fn keys_are_unique_and_author_dims_are_even() {
    for cfg in [babewyn_config(), lines_config()] {
        let assets = derive_assets(&cfg);
        let mut keys: Vec<&str> = assets.iter().map(|a| a.key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "duplicate asset keys in {}", cfg.game_id);

        // author = GU × 2, so every emitted author dimension must be even.
        for a in &assets {
            if let Some(w) = a.author_w {
                assert_eq!(w % 2, 0, "{} author_w not even", a.key);
            }
            if let Some(h) = a.author_h {
                assert_eq!(h % 2, 0, "{} author_h not even", a.key);
            }
        }
    }
}

#[test]
fn scene_assets_derive_and_plates_replace_stock_backgrounds() {
    use crate::model::game_config::{SceneAssetDef, SceneKind, SceneVariantDef};
    let mut cfg = babewyn_config();
    cfg.scene.assets = vec![
        SceneAssetDef {
            key: "bg_base".into(),
            kind: SceneKind::Plate,
            name: "Base scene".into(),
            description: "a storm-lashed lighthouse interior".into(),
            provider: String::new(),
            cutouts: false,
            wrap: false,
            placement: None,
            variants: vec![
                SceneVariantDef { key: "landscape".into(), preset: "landscape".into(), extra_prompt: String::new(), placement: None },
                SceneVariantDef {
                    key: "portrait".into(),
                    preset: "portrait".into(),
                    extra_prompt: "tall composition, the beacon visible above".into(),
                    placement: None,
                },
            ],
        },
        SceneAssetDef {
            key: "window_storm".into(),
            kind: SceneKind::Layer,
            name: String::new(),
            description: "the storm sky inside the window arch".into(),
            provider: "spritecook".into(),
            cutouts: false,
            wrap: false,
            placement: None,
            variants: vec![SceneVariantDef { key: "tempest".into(), preset: "landscape".into(), extra_prompt: String::new(), placement: None }],
        },
        SceneAssetDef {
            key: "booth".into(),
            kind: SceneKind::Sprite,
            name: "Observation booth".into(),
            description: String::new(),
            provider: String::new(),
            cutouts: false,
            wrap: false,
            placement: None,
            variants: vec![],
        },
    ];
    let assets = derive_assets(&cfg);

    // Stock §3 backgrounds are replaced by the plates.
    assert!(!has(&assets, "bg_base_landscape") || find(&assets, "bg_base_landscape").unwrap().category == "scenes");
    assert!(!has(&assets, "bg_feature_landscape"), "stock feature bg suppressed");

    // Plate variants derive at their preset dims with Background kind.
    let land = find(&assets, "bg_base_landscape").expect("plate landscape");
    assert_eq!(land.kind, AssetKind::Background);
    assert_eq!((land.author_w, land.author_h), (Some(2048), Some(1152)));
    let port = find(&assets, "bg_base_portrait").expect("plate portrait");
    assert_eq!((port.author_w, port.author_h), (Some(1242), Some(2208)));
    // The variant's extra prompt rides in the seeded description.
    assert!(port.description.contains("beacon visible above"));
    assert!(port.description.contains("lighthouse interior"));

    // Layer + sprite kinds; a variant-less sprite derives under its own key, square.
    assert_eq!(find(&assets, "bg_window_storm_tempest").unwrap().kind, AssetKind::SceneLayer);
    let booth = find(&assets, "bg_booth").expect("variant-less sprite, auto-prefixed");
    assert_eq!(booth.kind, AssetKind::SceneSprite);
    assert_eq!((booth.author_w, booth.author_h), (Some(1024), Some(1024)));

    // Without plates, the stock backgrounds stay.
    cfg.scene.assets.retain(|a| a.kind != SceneKind::Plate);
    let assets = derive_assets(&cfg);
    assert!(has(&assets, "bg_base_landscape"));
    assert_eq!(find(&assets, "bg_base_landscape").unwrap().category, "backgrounds");
}
