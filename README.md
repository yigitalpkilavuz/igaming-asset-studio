# Wishfell Asset Pipeline

A desktop studio for slot-game art. You describe a game once. The app derives the
full asset list, generates the art with AI providers, corrects it, and exports it
in the Stake Engine format.

The app runs on macOS. The core is Rust (Tauri 2). The interface is Svelte 5.

## What the app does

- Reads one blueprint per game. Derives every required asset from it: symbols,
  scene layers, reel chrome, panels, and the mascot.
- Assembles prompts from a per-game style master. Prompt assembly is deterministic.
  No LLM writes your prompts.
- Generates takes with a choice of providers: Draw Things (local), OpenAI gpt-image,
  Google Gemini, SpriteCook, and Gamelab Studio.
- Processes each take into shippable files: background removal, interior cut-outs,
  tone correction, symbol fit, WebP with PNG or JPG twins, and 9-slice descriptors.
- Audits the shipped pixels: chroma remnants, edge contact, stray specks, halo,
  seam match, tonal band, and visual weight.
- Cuts characters into parts, rigs them, and exports Spine 4.2 skeletons or baked
  sprite sheets. A procedural engine turns a written motion brief into looping
  keyframes.
- Exports the Stake dist tree with a scene manifest. Publishes finals into any
  folder, for example a game repository.

## The value budget

The art in these games is flat and unlit. All light comes from the runtime. Layers
can only separate by tonal value, so every layer owns a band of the tonal range.

The blueprint declares the bands. The process step measures each asset and corrects
it into its band with a solved gamma curve. Export and publish run the same gate.
An asset outside its band blocks the gate and appears in a named report. You can
override the gate, but the report keeps the violation.

## Requirements

- macOS with the Xcode command-line tools.
- Rust (stable) and Cargo.
- Node 20 or later, and pnpm.
- Optional: `rembg`, for background removal of opaque generations.
- Optional: `realesrgan-ncnn-vulkan`, for local upscales.
- API keys for the cloud providers you use. Release builds keep keys in the macOS
  Keychain. Debug builds use a local dev store.

## Run

```bash
pnpm install
pnpm tauri dev
```

The window opens with a Vite dev server on port 1420.

## Checks and builds

```bash
pnpm check          # type-check the frontend
pnpm build          # static frontend build
cargo test          # run the Rust test suite (from src-tauri/)
pnpm tauri build    # produce the app bundle
```

The Rust command surface generates the TypeScript bindings:

```bash
cargo test export_bindings   # from src-tauri/ — writes src/lib/ipc/bindings.ts
```

Do not edit `src/lib/ipc/bindings.ts` by hand.

To validate a Spine export against the real web runtime:

```bash
WF_SAMPLE_EXPORT_DIR=<dir> cargo test write_sample_export
node scripts/validate-spine.mjs <dir>
```

## Workflow

1. Create a game in the Library. Fill the Blueprint: mechanics, symbols, scene
   layers, tonal bands, and the style master.
2. Open Produce. The ledger shows every derived asset and its state.
3. Generate takes. Attach the style anchor and reference images to keep one look
   across the set. Generate related symbols as one sheet when you want a tight set.
4. Process the best take. The pipeline writes the shippable files and two reports:
   tone and fit.
5. Read the Quality panel. Fix what it flags: regenerate, refine, or adjust.
6. Animate what needs motion: cut parts, rig, write a motion brief, export.
7. Export the dist tree, or publish finals into your game repository. The value
   gate checks the set on both paths.

## Headless use

`wfcli` drives the same pipeline without the window. It reads and writes the same
on-disk store, so the app and the CLI always agree.

```bash
cargo run -q --bin wfcli -- <command>    # from src-tauri/
```

| Command | Purpose |
|---|---|
| `games` | List the projects. |
| `assets <game>` | List every derived asset with its state. |
| `show <game> <asset>` | Print the full asset record. |
| `image <game> <asset> [take]` | Print the path of the best image of a take. |
| `providers` | List the providers and their key state. |
| `generate <game> <asset> [--provider id] [--count n] [--ref key]` | Generate takes. |
| `process <game> <asset> [take]` | Run the processing pipeline. |
| `quality <game> <asset> [take]` | Print the audit report. |
| `tonegate <game>` | Run the value gate alone. |
| `export <game> [--force]` | Build the dist tree through the gate. |

## Where the data lives

The app keeps all state on disk, as JSON and image files:

```
~/Library/Application Support/com.wishfell.assetpipeline/
  settings.json          app settings
  projects/<game>/
    project.json         the blueprint
    assets/<key>/        prompt, takes, processed stages
    dist/<game>/         the exported Stake tree
```

The repository holds no game data and no keys.

## Stack notes

- Tauri 2 with a Rust core. All pipeline logic lives in `src-tauri/src`.
- Svelte 5 with runes, SvelteKit with the static adapter, Vite.
- `tauri-specta` generates the typed IPC bindings.
- Design tokens live in `src/lib/styles/tokens.css`. Components consume tokens only.
- Spine exports validate against `@esotericsoftware/spine-core@4.2.74`, the exact
  runtime of the Stake web SDK. MobileSAM weights download on first use (~40 MB).

## Known limits

- Generation runs as async commands with a spinner, not a formal job queue.
- Storage is plain JSON. There is no database index.
- The rig editor has no manual weight painting and no IK. Auto-weights only.
- Some optional asset classes are not derived yet: effect sheets, reel decor,
  i18n text, fonts, audio, and the loader.
