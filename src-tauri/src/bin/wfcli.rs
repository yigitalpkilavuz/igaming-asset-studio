//! wfcli — headless driver for the Wishfell Asset Pipeline.
//!
//! Same library, same on-disk store, no GUI: everything done here shows up in the
//! app and vice versa. Built for agent-driven production loops (generate → process →
//! quality → export) and for scripting.
//!
//! Output is JSON on stdout (one document per invocation); errors go to stderr with
//! a non-zero exit code. Debug builds read API keys from the dev secret store,
//! release builds from the macOS Keychain — identical to the app.
//!
//! Usage:
//!   wfcli games
//!   wfcli assets <game>
//!   wfcli show <game> <asset>
//!   wfcli image <game> <asset> [take]        # print path of the take's best image
//!   wfcli providers
//!   wfcli generate <game> <asset> [--provider id] [--count n] [--ref key]... [--guide key]
//!   wfcli process <game> <asset> [take]      # default: active take
//!   wfcli quality <game> <asset> [take]
//!   wfcli export <game>

use asset_pipeline_lib::commands::{
    generate::generate_variation_core, headless_config_dir, headless_projects_root,
    process::process_variation_core, quality::asset_quality_core,
};
use asset_pipeline_lib::{export, providers, settings, storage, taxonomy};

fn die(msg: &str) -> ! {
    eprintln!("wfcli: {msg}");
    std::process::exit(1)
}

fn ok<T: serde::Serialize>(value: &T) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_else(|e| die(&e.to_string())));
}

fn active_or(record: &asset_pipeline_lib::model::asset_record::AssetRecord, take: Option<&String>) -> String {
    take.cloned()
        .or_else(|| record.active_variation.clone())
        .unwrap_or_else(|| die("asset has no active take — pass a take id"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let cfg_dir = headless_config_dir().unwrap_or_else(|e| die(&e));
    let base = headless_projects_root().unwrap_or_else(|e| die(&e));
    let cfg = settings::load(&cfg_dir);

    // Positional args (non --flag) after the subcommand.
    let pos: Vec<&String> = args[1.min(args.len())..].iter().filter(|a| !a.starts_with("--")).collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == &format!("--{name}"))
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let flags_all = |name: &str| -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == &format!("--{name}"))
            .filter_map(|(i, _)| args.get(i + 1))
            .cloned()
            .collect()
    };

    match cmd {
        "games" => {
            let list = storage::list_projects(&base).unwrap_or_else(|e| die(&e));
            ok(&list);
        }
        "assets" => {
            let game = pos.first().unwrap_or_else(|| die("usage: wfcli assets <game>"));
            let project = storage::read_project(&base, game).unwrap_or_else(|e| die(&e));
            let assets = taxonomy::derive_assets(&project.config);
            let rows: Vec<serde_json::Value> = assets
                .iter()
                .map(|a| {
                    let rec = storage::read_asset_record(&base, game, &a.key).ok().flatten();
                    let (takes, active, processed) = rec
                        .map(|r| {
                            let active = r.active_variation.clone();
                            let processed = active
                                .as_ref()
                                .and_then(|id| r.variations.iter().find(|v| &v.id == id))
                                .is_some_and(|v| v.stages.iter().any(|s| s.name == "webp"));
                            (r.variations.len(), active, processed)
                        })
                        .unwrap_or((0, None, false));
                    serde_json::json!({
                        "key": a.key,
                        "category": a.category,
                        "production": a.production,
                        "authorW": a.author_w,
                        "authorH": a.author_h,
                        "takes": takes,
                        "active": active,
                        "processed": processed,
                    })
                })
                .collect();
            ok(&rows);
        }
        "show" => {
            let (game, asset) = match (pos.first(), pos.get(1)) {
                (Some(g), Some(a)) => (g, a),
                _ => die("usage: wfcli show <game> <asset>"),
            };
            let rec = storage::read_asset_record(&base, game, asset)
                .unwrap_or_else(|e| die(&e))
                .unwrap_or_else(|| die("asset has no record yet"));
            ok(&rec);
        }
        "image" => {
            let (game, asset) = match (pos.first(), pos.get(1)) {
                (Some(g), Some(a)) => (g, a),
                _ => die("usage: wfcli image <game> <asset> [take]"),
            };
            let rec = storage::read_asset_record(&base, game, asset)
                .unwrap_or_else(|e| die(&e))
                .unwrap_or_else(|| die("asset has no record yet"));
            let id = active_or(&rec, pos.get(2).copied());
            let var = rec
                .variations
                .iter()
                .find(|v| v.id == id)
                .unwrap_or_else(|| die("take not found"));
            // Best available: processed png stage, else the raw generation.
            let dir = storage::asset_dir(&base, game, asset);
            let path = var
                .stages
                .iter()
                .find(|s| s.name == "png")
                .map(|s| dir.join("variations").join(&var.id).join(&s.file))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| dir.join(&var.raw_file));
            ok(&serde_json::json!({ "take": id, "path": path }));
        }
        "providers" => {
            ok(&providers::list_providers(&cfg));
        }
        "generate" => {
            let (game, asset) = match (pos.first(), pos.get(1)) {
                (Some(g), Some(a)) => (g, a),
                _ => die("usage: wfcli generate <game> <asset> [--provider id] [--count n] [--ref key]... [--guide key]"),
            };
            let provider = flag("provider")
                .or_else(|| {
                    providers::list_providers(&cfg)
                        .into_iter()
                        .find(|p| p.configured)
                        .map(|p| p.id)
                })
                .unwrap_or_else(|| die("no configured provider — pass --provider"));
            let count: u32 = flag("count").map(|c| c.parse().unwrap_or(1)).unwrap_or(1);
            let refs = flags_all("ref");
            let guide = flag("guide").unwrap_or_default();
            let rec = tauri::async_runtime::block_on(generate_variation_core(
                cfg,
                base,
                game.to_string(),
                asset.to_string(),
                provider,
                refs,
                guide,
                count,
            ))
            .unwrap_or_else(|e| die(&e));
            ok(&rec);
        }
        "process" => {
            let (game, asset) = match (pos.first(), pos.get(1)) {
                (Some(g), Some(a)) => (g, a),
                _ => die("usage: wfcli process <game> <asset> [take]"),
            };
            let rec = storage::read_asset_record(&base, game, asset)
                .unwrap_or_else(|e| die(&e))
                .unwrap_or_else(|| die("asset has no record yet"));
            let id = active_or(&rec, pos.get(2).copied());
            let rec = tauri::async_runtime::block_on(process_variation_core(
                base,
                game.to_string(),
                asset.to_string(),
                id,
            ))
            .unwrap_or_else(|e| die(&e));
            ok(&rec);
        }
        "quality" => {
            let (game, asset) = match (pos.first(), pos.get(1)) {
                (Some(g), Some(a)) => (g, a),
                _ => die("usage: wfcli quality <game> <asset> [take]"),
            };
            let rec = storage::read_asset_record(&base, game, asset)
                .unwrap_or_else(|e| die(&e))
                .unwrap_or_else(|| die("asset has no record yet"));
            let id = active_or(&rec, pos.get(2).copied());
            let report = tauri::async_runtime::block_on(asset_quality_core(
                base,
                game.to_string(),
                asset.to_string(),
                id,
            ))
            .unwrap_or_else(|e| die(&e));
            ok(&report);
        }
        "export" => {
            let game = pos.first().unwrap_or_else(|| die("usage: wfcli export <game>"));
            let report = export::build_dist(&base, game).unwrap_or_else(|e| die(&e));
            ok(&report);
        }
        _ => {
            eprintln!(
                "wfcli — headless Wishfell Asset Pipeline\n\
                 commands: games | assets <game> | show <game> <asset> | image <game> <asset> [take]\n\
                 | providers | generate <game> <asset> [--provider id] [--count n] [--ref key]... [--guide key]\n\
                 | process <game> <asset> [take] | quality <game> <asset> [take] | export <game>"
            );
            std::process::exit(if cmd == "help" { 0 } else { 1 });
        }
    }
}
