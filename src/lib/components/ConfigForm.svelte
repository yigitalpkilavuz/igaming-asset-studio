<script lang="ts">
  import type { GameConfig, SymbolRole, WinType } from "$lib/ipc";
  import { DEFAULT_AUDIO_CUES } from "$lib/audioCues";
  import { DEFAULT_FONTS } from "$lib/fonts";
  import { DEFAULT_SYMBOL_SIZING, DEFAULT_SYMBOL_TONE } from "$lib/symbolDefaults";

  let {
    config = $bindable(),
    idLocked = false,
    section = "all",
  }: {
    config: GameConfig;
    idLocked?: boolean;
    /** Which Blueprint tab to render ("all" = the whole form). */
    section?: "all" | "identity" | "mechanics" | "symbols" | "scenes";
  } = $props();

  const winTypes: WinType[] = ["lines", "ways", "scatter", "cluster"];
  const roles: SymbolRole[] = ["high", "low", "wild", "expandingWild", "scatter", "bonus", "special"];

  // Symbol fit + tone — backfill for configs that predate these blocks (new games get them from
  // the backend's serde defaults). Single source: $lib/symbolDefaults (mirrors game_config.rs).
  config.symbolSizing ??= structuredClone(DEFAULT_SYMBOL_SIZING);
  const sizing = config.symbolSizing;
  config.symbolTone ??= structuredClone(DEFAULT_SYMBOL_TONE);
  const tone = config.symbolTone;

  // Scene-asset system — backfill for configs that predate it.
  config.scene ??= {
    presets: [
      { key: "landscape", width: 2048, height: 1152 },
      { key: "portrait", width: 1242, height: 2208 },
    ],
    guides: [],
    assets: [],
    defaultProvider: "",
    webpQuality: 90,
  };
  const scene = config.scene;
  scene.presets ??= [];

  // Audio system — backfill + seed the default cue list the first time it's enabled.
  config.audio ??= { cues: [], defaultProvider: "", stylePrompt: "" };
  function ensureAudioCues() {
    config.audio ??= { cues: [], defaultProvider: "", stylePrompt: "" };
    if (config.hasAudio && !(config.audio.cues?.length ?? 0)) {
      config.audio.cues = DEFAULT_AUDIO_CUES.map((c) => ({ ...c }));
    }
  }

  // Bitmap fonts — backfill + seed the default font set the first time it's enabled.
  config.fonts ??= [];
  function ensureFonts() {
    config.fonts ??= [];
    if (config.hasFonts && !config.fonts.length) {
      config.fonts = DEFAULT_FONTS.map((f) => ({ ...f }));
    }
  }
  scene.assets ??= [];
  function addPreset() {
    scene.presets = [...(scene.presets ?? []), { key: `preset${(scene.presets?.length ?? 0) + 1}`, width: 2048, height: 1152 }];
  }
  function removePreset(i: number) {
    scene.presets = (scene.presets ?? []).filter((_, idx) => idx !== i);
  }
  function addSceneAsset() {
    scene.assets = [
      ...(scene.assets ?? []),
      { key: `scene${(scene.assets?.length ?? 0) + 1}`, kind: "plate", name: "", description: "", provider: "", cutouts: false, variants: [] },
    ];
  }
  function removeSceneAsset(i: number) {
    scene.assets = (scene.assets ?? []).filter((_, idx) => idx !== i);
  }
  function addVariant(ai: number) {
    const a = scene.assets?.[ai];
    if (!a) return;
    a.variants = [...(a.variants ?? []), { key: "", preset: scene.presets?.[0]?.key ?? "", extraPrompt: "" }];
  }
  function removeVariant(ai: number, vi: number) {
    const a = scene.assets?.[ai];
    if (!a) return;
    a.variants = (a.variants ?? []).filter((_, idx) => idx !== vi);
  }
  /** Read a sizing field as a whole percent. */
  function pct(v: number | null | undefined, fallback: number): number {
    return Math.round((v ?? fallback) * 100);
  }

  function addSymbol() {
    config.symbols = [
      ...config.symbols,
      { key: `sym${config.symbols.length + 1}`, role: "high", name: "", description: "", animation: "", sizeNudge: 1 },
    ];
  }
  function removeSymbol(i: number) {
    config.symbols = config.symbols.filter((_, idx) => idx !== i);
  }

  function addMode() {
    config.buyBonusModes = [
      ...(config.buyBonusModes ?? []),
      { key: `mode${(config.buyBonusModes?.length ?? 0) + 1}`, name: "" },
    ];
  }
  function removeMode(i: number) {
    config.buyBonusModes = (config.buyBonusModes ?? []).filter((_, idx) => idx !== i);
  }
</script>

<div class="form">
{#if section === "all" || section === "identity"}
  <!-- Naming -->
  <div class="name-block">
    <label class="big-field">
      <span class="fl">Name</span>
      <input class="name-input" bind:value={config.name} placeholder="Babewyn Court" />
    </label>
    <label class="id-field">
      <span class="fl">Game id</span>
      <input class="mono" bind:value={config.gameId} placeholder="babewyn_court" disabled={idLocked} spellcheck="false" />
    </label>
  </div>

  <!-- Shape: win type + grid -->
  <div class="shape">
    <div class="shape-part">
      <span class="fl">Win type</span>
      <div class="seg">
        {#each winTypes as w (w)}
          <button type="button" class:on={config.winType === w} onclick={() => (config.winType = w)}>{w}</button>
        {/each}
      </div>
    </div>
    <div class="shape-part">
      <span class="fl">Grid</span>
      <div class="grid-ctl">
        <input type="number" min="1" max="10" bind:value={config.cols} aria-label="columns" />
        <span class="times">×</span>
        <input type="number" min="1" max="10" bind:value={config.rows} aria-label="rows" />
        <span class="grid-note">columns × rows</span>
      </div>
    </div>
  </div>

  <!-- Style master — the key creative field: kept first + roomy, no ornament. -->
  <label class="field">
    <span>Style master <em class="hint">— prepended to every asset prompt (your anti-slop lever)</em></span>
    <textarea
      bind:value={config.stylePrompt}
      rows="5"
      placeholder="illuminated medieval manuscript marginalia; iron-gall ink outlines with visible hand-drawn line weight; aged parchment; muted oxblood/lapis/forest-green with restrained gold-leaf; flat matte painterly; slightly wonky, hand-inked — NOT hyperrealistic, NOT 3D, NOT digital-clean"
    ></textarea>
  </label>

  <!-- Secondary -->
  <div class="field-row">
    <label class="field">
      <span>Negative master <em class="hint">— extra negatives (the studio floor always applies)</em></span>
      <textarea bind:value={config.negativePrompt} rows="2" placeholder="e.g. cute, chibi, saturated, modern clothing…"></textarea>
    </label>
    <label class="field">
      <span>Brief <em class="hint">— a note for yourself</em></span>
      <textarea bind:value={config.brief} rows="2" placeholder="One-line theme note…"></textarea>
    </label>
  </div>
{/if}

{#if section === "all" || section === "mechanics"}
  <fieldset class="mechanics">
    <legend>Mechanics</legend>
    <div class="tgroup">
      <span class="tg-label">Features</span>
      <div class="toggles">
        <label class="check"><input type="checkbox" bind:checked={config.hasFeatureBackground} /> Feature background</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasBuyBonus} /> Buy bonus</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasMystery} /> Mystery symbols</label>
        <label class="check"><input type="checkbox" bind:checked={config.holdAndSpin} /> Hold &amp; spin</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasMeter} /> Collection meter</label>
      </div>
    </div>
    <div class="tgroup">
      <span class="tg-label">Cast &amp; polish</span>
      <div class="toggles">
        <label class="check"><input type="checkbox" bind:checked={config.hasMascot} /> Mascot character</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasAudio} onchange={ensureAudioCues} /> Audio (music + SFX)</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasLoader} /> Loader art</label>
        <label class="check"><input type="checkbox" bind:checked={config.hasFonts} onchange={ensureFonts} /> Bitmap fonts</label>
      </div>
    </div>
  </fieldset>

  {#if config.hasMascot}
    <label class="field">
      <span>Mascot — who are they?</span>
      <textarea
        rows="2"
        bind:value={config.mascotDescription}
        placeholder="e.g. a gaunt plague-doctor raven clutching a dim lantern"
      ></textarea>
    </label>
  {/if}

  {#if config.hasAudio && config.audio}
    <label class="field">
      <span>Audio style master</span>
      <textarea
        rows="2"
        bind:value={config.audio.stylePrompt}
        placeholder="the sonic identity, prepended to every cue — e.g. dark baroque orchestral, harpsichord and low strings, ominous"
      ></textarea>
    </label>
    <p class="hint audio-hint">
      {config.audio.cues?.length ?? 0} cues seeded (base music + core SFX). Tune each sound's prompt in its
      bench, and add your Stable Audio / Vertex keys in Settings.
    </p>
  {/if}

  {#if config.hasMeter}
    <label class="field narrow">
      <span>Meter thresholds</span>
      <input type="number" min="0" max="12" bind:value={config.meterThresholds} />
    </label>
  {/if}

  {#if config.hasBuyBonus}
    <div class="symbols">
      <div class="symbols-head">
        <span>Buy-bonus modes <span class="muted">(2+ adds a selector screen)</span></span>
        <button type="button" onclick={addMode}>+ Add mode</button>
      </div>
      <div class="symbol-rows">
        {#each config.buyBonusModes ?? [] as mode, i (i)}
          <div class="mode-row">
            <input class="sk" bind:value={mode.key} placeholder="bonus" spellcheck="false" />
            <input class="sn" bind:value={mode.name} placeholder="Bonus" />
            <button type="button" class="rm" title="Remove" onclick={() => removeMode(i)}>×</button>
          </div>
        {/each}
      </div>
    </div>
  {/if}

{/if}

{#if section === "all" || section === "symbols"}
  <fieldset class="mechanics">
    <legend>Symbol fit — ink % of cell · height % of cell, per class</legend>
    <div class="fit-grid">
      <span class="muted tiny"></span>
      <span class="muted tiny">ink %</span>
      <span class="muted tiny">height %</span>
      <span class="muted tiny">tone lo %</span>
      <span class="muted tiny">tone hi %</span>
      {#each [{ name: "low", cls: sizing.low, tone: tone.low }, { name: "high", cls: sizing.high, tone: tone.high }, { name: "wild", cls: sizing.wild, tone: tone.wild }, { name: "scatter", cls: sizing.scatter, tone: tone.scatter }] as e (e.name)}
        <span class="fit-name">{e.name}</span>
        <input type="number" min="5" max="80" value={pct(e.cls?.ink, 30)} oninput={(ev) => e.cls && (e.cls.ink = +ev.currentTarget.value / 100)} />
        <input type="number" min="20" max="100" value={pct(e.cls?.height, 75)} oninput={(ev) => e.cls && (e.cls.height = +ev.currentTarget.value / 100)} />
        <input type="number" min="5" max="95" value={pct(e.tone?.min, 30)} oninput={(ev) => e.tone && (e.tone.min = +ev.currentTarget.value / 100)} title="tonal band floor — median perceptual luminance %" />
        <input type="number" min="5" max="95" value={pct(e.tone?.max, 40)} oninput={(ev) => e.tone && (e.tone.max = +ev.currentTarget.value / 100)} title="tonal band ceiling — median perceptual luminance %" />
      {/each}
    </div>
    <div class="field-row">
      <label class="field narrow">
        <span>Safe W %</span>
        <input type="number" min="50" max="100" value={pct(sizing.safeW, 92)} oninput={(e) => (sizing.safeW = +e.currentTarget.value / 100)} />
      </label>
      <label class="field narrow">
        <span>Safe H %</span>
        <input type="number" min="50" max="100" value={pct(sizing.safeH, 88)} oninput={(e) => (sizing.safeH = +e.currentTarget.value / 100)} />
      </label>
      <label class="field narrow">
        <span>Ink weight %</span>
        <input type="number" min="0" max="100" value={pct(sizing.areaWeight, 65)} oninput={(e) => (sizing.areaWeight = +e.currentTarget.value / 100)} title="0 = fit by height only (old broken behaviour), 100 = fit by ink only (slim objects grow huge). The taste dial." />
      </label>
    </div>
    <p class="muted note-line">
      The eye reads INK (covered pixels), not height — a hat and a hand at equal height
      differ ~3× in ink. Process blends height- and ink-based scales, clamps to the safe
      box, and flags UNDER/OVERWEIGHT symbols in Quality. Tone lo/hi = the value-budget band Process
      gamma-corrects each symbol into. Advanced knobs (centroidBias, alphaFloor, canvas,
      gamma clamp) live in the porter JSON.
    </p>
  </fieldset>

{/if}

{#if section === "all" || section === "scenes"}
  <div class="symbols">
    <div class="symbols-head">
      <span>Scene assets <span class="muted">({scene.assets?.length ?? 0}) — plates · layers · sprites</span></span>
      <button type="button" onclick={addSceneAsset}>+ Add scene asset</button>
    </div>
    {#if scene.assets?.length}
      <p class="muted note-line">
        Plates replace the stock backgrounds. Variants derive
        <span class="mono">key_variant</span> assets (e.g. <span class="mono">bg_base_landscape</span>);
        the description is the shared prompt core, each variant's extra prompt is appended.
        Guide zones are edited via the Blueprint porter JSON.
      </p>
      <div class="field-row">
        <label class="field narrow">
          <span>Scenes provider</span>
          <input bind:value={scene.defaultProvider} placeholder="e.g. spritecook" spellcheck="false" />
        </label>
        <label class="field narrow">
          <span>Symbols provider</span>
          <input bind:value={config.symbolProvider} placeholder="e.g. openai_image" spellcheck="false" />
        </label>
      </div>
      <div class="symbol-rows">
        {#each scene.assets ?? [] as sa, ai (ai)}
          <div class="scene-asset">
            <div class="mode-row scene-main">
              <input class="sk" bind:value={sa.key} placeholder="bg_base" spellcheck="false" />
              <select bind:value={sa.kind} title="plate = full-frame background · layer = transparent stack element · sprite = standalone alpha object · fx = still for a looping spritesheet · particle = tiny exact-size texture">
                <option value="plate">plate</option>
                <option value="layer">layer</option>
                <option value="sprite">sprite</option>
                <option value="fx">fx</option>
                <option value="particle">particle</option>
              </select>
              <input bind:value={sa.description} placeholder="shared prompt core — what this scene asset is…" />
              <label class="cut-chk" title="the element has interior see-through openings (window glass, arch gaps) — generated as flat chroma fills and keyed to real transparency">
                <input type="checkbox" bind:checked={sa.cutouts} />
                <span class="tiny">cut-outs</span>
              </label>
              <label class="tone-band" title="value-budget band this layer owns (median HSV value, % of full scale) — Process bakes a gamma correction; empty = no tone pass">
                <span class="tiny">tone</span>
                <input
                  type="number" min="1" max="95" placeholder="—"
                  value={sa.tonalTarget ? Math.round((sa.tonalTarget.min ?? 0) * 100) : ""}
                  oninput={(e) => {
                    const v = +e.currentTarget.value;
                    if (!v) { sa.tonalTarget = null; return; }
                    sa.tonalTarget = { min: v / 100, max: sa.tonalTarget?.max ?? v / 100 };
                  }}
                />–<input
                  type="number" min="1" max="95" placeholder="—"
                  value={sa.tonalTarget ? Math.round((sa.tonalTarget.max ?? 0) * 100) : ""}
                  oninput={(e) => {
                    const v = +e.currentTarget.value;
                    if (!v) return;
                    sa.tonalTarget = { min: sa.tonalTarget?.min ?? v / 100, max: v / 100 };
                  }}
                />
              </label>
              <button type="button" class="rm" title="Remove" onclick={() => removeSceneAsset(ai)}>×</button>
            </div>
            {#each sa.variants ?? [] as v, vi (vi)}
              <div class="variant-row">
                <input class="vk" bind:value={v.key} placeholder="landscape" spellcheck="false" />
                <select bind:value={v.preset}>
                  {#each scene.presets ?? [] as p (p.key)}<option value={p.key}>{p.key}</option>{/each}
                </select>
                <input bind:value={v.extraPrompt} placeholder="variant-only prompt addition…" />
                <button type="button" class="rm" title="Remove variant" onclick={() => removeVariant(ai, vi)}>×</button>
              </div>
            {/each}
            <button type="button" class="ghost tiny add-variant" onclick={() => addVariant(ai)}>
              + variant
            </button>
          </div>
        {/each}
      </div>
      <div class="symbols-head presets-head">
        <span class="muted">Presets</span>
        <button type="button" onclick={addPreset}>+ Add preset</button>
      </div>
      {#each scene.presets ?? [] as p, i (i)}
        <div class="preset-row">
          <input class="sk" bind:value={p.key} spellcheck="false" />
          <input type="number" min="256" max="4096" bind:value={p.width} />
          <span class="muted">×</span>
          <input type="number" min="256" max="4096" bind:value={p.height} />
          <button type="button" class="rm" title="Remove" onclick={() => removePreset(i)}>×</button>
        </div>
      {/each}
    {/if}
  </div>

{/if}

{#if section === "all" || section === "symbols"}
  <div class="symbols">
    <div class="symbols-head">
      <span>Symbols <span class="muted">({config.symbols.length})</span></span>
      <button type="button" onclick={addSymbol}>+ Add symbol</button>
    </div>
    <p class="sym-hint muted">
      The last field is <b>relative size</b> (footprint vs the pay class, 1 = default): a
      coin at 0.6 ships smaller than a dragon at 1.2. Judge it on the contact sheet.
      Symbols made with “Generate as set” keep the sheet's own scale.
    </p>
    <div class="symbol-rows">
      {#each config.symbols as symbol, i (i)}
        <div class="symbol-row">
          <input class="sk" bind:value={symbol.key} placeholder="h1" spellcheck="false" />
          <select bind:value={symbol.role}>
            {#each roles as r (r)}<option value={r}>{r === "expandingWild" ? "expanding wild" : r}</option>{/each}
          </select>
          <input class="sn" bind:value={symbol.name} placeholder="Snail-Knight" />
          <input class="sd" bind:value={symbol.description} placeholder="art-direction notes…" />
          <input
            class="sa"
            bind:value={symbol.animation}
            placeholder="motion (short loop)…"
            title="intended motion, one sentence — seeds the AI spritesheet (max 24 frames ≈ 2 s loop)"
          />
          <input
            class="sz"
            type="number"
            min="0.3"
            max="1.5"
            step="0.05"
            value={symbol.sizeNudge ?? 1}
            oninput={(e) => (symbol.sizeNudge = +e.currentTarget.value || 1)}
            title="Relative size vs this symbol's pay class — 1 = class default, 0.6 = a small object (coin), 1.3 = a big one (dragon). Judge it on the contact sheet. Symbols made with “Generate as set” keep the sheet's own scale and ignore this."
          />
          <button type="button" class="rm" title="Remove" onclick={() => removeSymbol(i)}>×</button>
        </div>
      {/each}
    </div>
  </div>
{/if}
</div>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 1.15rem;
  }
  .field-row {
    display: flex;
    gap: 0.75rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    flex: 1;
  }
  .field.narrow {
    flex: 0 0 6rem;
  }
  .field span {
    font-family: var(--font-mono);
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.18em;
    color: var(--ash);
  }
  .field span .hint {
    font-family: var(--font-sans);
    text-transform: none;
    letter-spacing: normal;
    font-style: normal;
    font-weight: 400;
    color: var(--ash-deep);
  }

  /* ── Identity: a composed first screen, not a flat stack ── */
  .fl {
    font-family: var(--font-mono);
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.16em;
    color: var(--ash);
  }
  .name-block {
    display: grid;
    grid-template-columns: 1fr 15rem;
    gap: 0.75rem;
    align-items: end;
  }
  .big-field,
  .id-field {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .name-input {
    font-size: 1.15rem;
    font-weight: 600;
    padding: 0.5rem 0.7rem;
  }

  /* Shape control: segmented win type + grid steppers */
  .shape {
    display: flex;
    gap: 2rem;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .shape-part {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    overflow: hidden;
    width: fit-content;
  }
  .seg button {
    border: none;
    border-radius: 0;
    border-right: 1px solid var(--line);
    background: transparent;
    color: var(--ash);
    font-size: 0.8rem;
    padding: 0.38rem 0.95rem;
    text-transform: capitalize;
  }
  .seg button:last-child {
    border-right: none;
  }
  .seg button:hover:not(.on) {
    color: var(--bone-dim);
    background: var(--wash);
  }
  .seg button.on {
    background: var(--wash);
    color: var(--bone);
  }
  .grid-ctl {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .grid-ctl input {
    width: 3.4rem;
    text-align: center;
  }
  .times {
    color: var(--ash);
  }
  .grid-note {
    font-family: var(--font-mono);
    font-size: 0.62rem;
    color: var(--ash-deep);
    margin-left: 0.35rem;
  }

  fieldset.mechanics {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem 1.5rem;
    padding: 0.9rem 1.1rem 1.1rem;
    margin: 0;
  }
  legend {
    font-family: var(--font-mono);
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.18em;
    color: var(--ash);
    padding: 0 0.4rem;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.92rem;
    color: var(--bone-dim);
  }
  .check input {
    width: auto;
    accent-color: var(--gold);
  }
  /* Grouped mechanics toggles — clusters, each a labelled grid (declutters the flat wall). */
  .tgroup {
    flex: 1 1 100%;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .tg-label {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    font-weight: 700;
    color: var(--gold-deep);
  }
  .toggles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 0.5rem 1.25rem;
  }
  .symbols-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.6rem;
  }
  .symbols-head > span {
    font-family: var(--font-sans);
    font-size: 1rem;
    color: var(--bone);
  }
  .sym-hint {
    margin: -0.2rem 0 0.7rem;
    font-size: 0.8rem;
    line-height: 1.45;
  }
  .symbol-rows {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    max-height: 42vh;
    overflow: auto;
    padding-right: 4px;
  }
  /* Two-line symbol row: identity on top, art + motion direction below (full width) —
     six columns on one line left every input too narrow to read. */
  .symbol-row {
    display: grid;
    grid-template-columns: 5rem 6.5rem 1fr 4rem auto;
    grid-template-areas:
      "key role name name rm"
      "desc desc anim sz sz";
    gap: 0.3rem 0.4rem;
    align-items: center;
    padding-bottom: 0.45rem;
    border-bottom: 1px solid var(--line);
  }
  .symbol-row:last-child {
    border-bottom: none;
  }
  .sk {
    grid-area: key;
  }
  .symbol-row select {
    grid-area: role;
  }
  .sn {
    grid-area: name;
  }
  .sd {
    grid-area: desc;
  }
  .sa {
    grid-area: anim;
  }
  .sz {
    grid-area: sz;
    width: 4.2rem;
    justify-self: end;
  }
  .rm {
    grid-area: rm;
  }
  .tone-band {
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    font-size: 0.7rem;
    color: var(--bone-dim);
    white-space: nowrap;
  }
  .tone-band input {
    width: 3.2rem;
    padding: 0.25rem 0.3rem;
    font-size: 0.72rem;
  }
  .fit-grid {
    display: grid;
    grid-template-columns: 5rem 4.4rem 4.4rem 4.4rem 4.4rem;
    gap: 0.35rem 0.5rem;
    align-items: center;
  }
  .fit-grid input {
    padding: 0.35rem 0.5rem;
    font-size: 0.78rem;
  }
  .fit-name {
    font-size: 0.75rem;
    color: var(--bone-dim);
  }
  .note-line {
    font-size: 0.72rem;
    margin: 0.4rem 0 0;
  }
  .scene-asset {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--line);
  }
  .scene-asset .mode-row.scene-main {
    grid-template-columns: 7rem 5.5rem 1fr auto auto;
  }
  .cut-chk {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--ash);
    white-space: nowrap;
  }
  .variant-row {
    display: grid;
    grid-template-columns: 6rem 6rem 1fr auto;
    gap: 0.4rem;
    align-items: center;
    padding-left: 1rem;
  }
  .variant-row input,
  .variant-row select,
  .preset-row input {
    padding: 0.35rem 0.5rem;
    font-size: 0.78rem;
  }
  .add-variant {
    align-self: flex-start;
    font-size: 0.68rem;
    margin-left: 1rem;
  }
  .presets-head {
    margin-top: 0.6rem;
  }
  .preset-row {
    display: grid;
    grid-template-columns: 7rem 5rem auto 5rem auto;
    gap: 0.4rem;
    align-items: center;
  }
  .mode-row {
    display: grid;
    grid-template-columns: 8rem 1fr auto;
    gap: 0.4rem;
    align-items: center;
  }
  .mode-row input,
  .symbol-row input,
  .symbol-row select {
    padding: 0.4rem 0.55rem;
    font-size: 0.82rem;
  }
  .rm {
    padding: 0.3rem 0.6rem;
    color: var(--ash);
    border-color: var(--line);
  }
  .rm:hover:not(:disabled) {
    color: var(--oxblood);
    border-color: var(--oxblood);
  }
</style>
