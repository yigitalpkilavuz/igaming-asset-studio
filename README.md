# Wishfell Asset Pipeline

A Tauri desktop app to manage Wishfell's slot games and their art assets for Stake Engine:
game config → derived asset list → AI prompts → image generation (local + cloud) → post-processing
→ Stake-ready export.

See the full plan in `~/.claude/plans/` and the specs in `../docs/` (`ASSETS.md`, `HUD_SPEC.md`,
`GAME_PLAYBOOK.md`, `BRAND.md`).

## Stack

- **Tauri 2** (Rust core) + **Svelte 5 / SvelteKit (adapter-static, SPA)** + **Vite** frontend.
- **tauri-specta** generates typed IPC bindings from the Rust command surface into
  `src/lib/ipc/bindings.ts` — never edit that file by hand.

## Develop

```bash
pnpm install
pnpm tauri dev        # launches the desktop window (Vite dev on :1420)
```

## Useful commands

```bash
pnpm check            # svelte-check (type-check frontend)
pnpm build            # SvelteKit static build -> build/
cargo test export_bindings --manifest-path src-tauri/Cargo.toml   # regenerate TS bindings headlessly
pnpm tauri info       # environment / version report
```

## Layout

```
src/                  # Svelte frontend (routes/, lib/ipc, lib/components, app.css)
src-tauri/            # Rust core (commands, model, taxonomy, providers, prompts, processing, jobs, export)
```

## Milestones

- **M0 — Scaffold** ✅ Tauri + Svelte 5 shell, typed bindings, `ping` round-trip.
- **M1 — Project config + taxonomy engine** ✅ config form → derived asset list (ASSETS.md-pinned).
- **M2 — Prompts (deterministic master system)** ✅ No LLM. Prompts assemble from a per-game
  **style master** + **negative master** + per-category composition template + a per-asset **subject**.
  Studio owns the aesthetic (anti-slop); the OpenAI key is only used for gpt-image-1 image generation.
- **M3 — Providers + generate + history** ✅ Draw Things (local) + gpt-image-1 (cloud), variations + lineage + active pick.
- **M4 — Post-processing** ✅ Rust-native resize→WebP+PNG/JPG + 9-slice `.9.json`; rembg + Real-ESRGAN optional (preflight-detected).
- **M5 — Stake-format export** ✅ ASSETS.md §16 dist tree + §17 `assets.ts` snippet + §18 readiness check.
- **M6 — Animation Studio** ✅ In-app Spine-style studio (full-screen per asset): local-SAM
  segmentation of the ORIGINAL pixels with click/brush correction → AI occlusion inpainting
  (band-limited, anti-drift) → auto-rig (AI parent tree + joint-band pivots) with bone gizmos
  and wiggle test → keyframe timeline (curves, onion skin, stage-gizmo auto-key) → AI motion
  drafting/in-betweens → deformable auto-weighted meshes → **Spine 4.2** JSON + atlas export
  (validated against `@esotericsoftware/spine-core@4.2.74`, the exact web-sdk runtime), with a
  PNG spritesheet bake as fallback. `dist` prefers a studio Spine set over the raster sprite.
  MobileSAM weights auto-download (~40 MB) to app data; runtime validation:
  `WF_SAMPLE_EXPORT_DIR=<dir> cargo test write_sample_export && node scripts/validate-spine.mjs <dir>`.

### Known simplifications (revisit later)
- Generation runs as async commands (spinner), not a formal job queue with progress events.
- Storage is JSON-only; no SQLite index yet.
- Variation images are shipped to the UI as base64 data URLs (fine for small counts).
- Studio v1 cuts: no key multi-select/copy-paste, no shear/draw-order keys/IK, no manual
  weight painting (auto-weights only), single handle pair per key in the curve editor.

### Taxonomy coverage vs ASSETS.md
Covered (config-driven): symbols (§2), backgrounds (§3), reel_background/frame/anticipation (§4),
symbol_frame/win_highlight/mystery/hold (§5), panels + meter + buy-bonus selector (§9), payline
indicators (§8, win-type conditional), game_logo + splash (§10).
Not yet emitted (deferred, need extra tooling or a driving game): symbol modifier icons (§5),
effects spritesheets (§7), optional reel decor — divider/top/bottom/pillars (§4), feature hero (§10),
i18n text (§12), fonts (§14), audio (§15), loader (§13), ambient (§11).
