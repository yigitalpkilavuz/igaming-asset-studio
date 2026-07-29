<script lang="ts">
  /**
   * Game preview — the game's REAL assets composed into a representative slot screen so you can
   * judge how everything reads together (size, contrast, cohesion) without exporting into the game.
   * A faithful STILL: the background fills a canvas-aspect stage, symbols sit in the reel grid at
   * TRUE relative size (the same cell math the pipeline ships — taxonomy::dimensions), the mascot
   * stands beside the reels, and Shuffle re-rolls which symbols land where. Pure front-end compose
   * over the existing image commands — no export, instant shuffle.
   */
  import { onMount } from "svelte";
  import {
    commands,
    unwrap,
    type AssetRecord,
    type GameConfig,
    type SymbolRole,
  } from "$lib/ipc";
  import { assetStatus } from "$lib/assetStatus";

  let { gameId }: { gameId: string } = $props();

  // ── Cell geometry — mirror of src-tauri/src/taxonomy/dimensions.rs (ASSETS.md §2). Cells are
  // square; the board is `cols·cell × rows·cell` game-units centered in the canvas. ────────────
  function cellGu(cols: number, rows: number): number {
    const std: Record<string, number> = { "5,3": 140, "5,4": 130, "6,5": 110, "7,7": 90 };
    const hit = std[`${cols},${rows}`];
    if (hit) return hit;
    const cell = Math.min(Math.floor(1120 / Math.max(1, cols)), Math.floor(620 / Math.max(1, rows)));
    return Math.max(1, Math.floor(cell / 10)) * 10;
  }
  const CANVAS = { landscape: [1600, 900], portrait: [900, 1600] } as const;
  const MASCOT_AR = 1200 / 1800; // author size, portrait

  type Sym = { key: string; role: SymbolRole; name: string; url: string };

  let loading = $state(true);
  let error = $state("");
  let config = $state<GameConfig | null>(null);
  let symbols = $state<Sym[]>([]); // symbols that have processed/active art
  let missing = $state<string[]>([]); // symbol keys not ready yet
  let bgLandscape = $state<string | null>(null);
  let bgPortrait = $state<string | null>(null);
  let mascotUrl = $state<string | null>(null);
  let frameUrl = $state<string | null>(null);
  let reelBgUrl = $state<string | null>(null);
  let sceneArt = $state<Record<string, string>>({}); // concrete scene-asset key → data URL
  let grid = $state<Sym[]>([]); // cols*rows, row-major

  // Controls
  let orientation = $state<"landscape" | "portrait">("landscape");
  let hiddenLayers = $state<Set<string>>(new Set()); // scene-layer keys the user has hidden
  let layersOpen = $state(false);
  let layersEl = $state<HTMLElement | null>(null);
  let showFrame = $state(true);
  let showMascot = $state(true);
  let showWords = $state(true);
  let showCells = $state(true);
  let mascotSide = $state<"left" | "right">("right");

  // Stage pixel size (bound) → everything positions in px for crispness.
  let stageW = $state(0);
  let stageH = $state(0);

  const cols = $derived(config?.cols ?? 5);
  const rows = $derived(config?.rows ?? 3);
  const canvasGu = $derived(CANVAS[orientation]);
  const cell = $derived(cellGu(cols, rows));
  const boardWfrac = $derived((cols * cell) / canvasGu[0]);
  const boardHfrac = $derived((rows * cell) / canvasGu[1]);
  const boardLpx = $derived((stageW * (1 - boardWfrac)) / 2);
  const boardTpx = $derived((stageH * (1 - boardHfrac)) / 2);
  const boardWpx = $derived(stageW * boardWfrac);
  const boardHpx = $derived(stageH * boardHfrac);
  const cellPx = $derived(boardWpx / Math.max(1, cols));
  // The reel panel wraps the grid with a margin; the grid itself is cols×rows sockets with a
  // gutter, so symbols read as seated in real cells (not floating on a bare grid).
  const gap = $derived(cellPx * 0.08);
  const reelPad = $derived(cellPx * 0.16);
  const panelLpx = $derived(boardLpx - reelPad);
  const panelTpx = $derived(boardTpx - reelPad);
  const panelWpx = $derived(boardWpx + 2 * reelPad);
  const panelHpx = $derived(boardHpx + 2 * reelPad);
  const bg = $derived(orientation === "landscape" ? bgLandscape : bgPortrait);

  // ── Scene layer stack ── config.scene.assets is the ordered back→front stack; each layer's
  // concrete asset key = class prefix (bg_/fx_/p_) + key + optional `_variant` (taxonomy::derive).
  function sceneBaseKey(sa: { key: string; kind: string }): string {
    const prefix = sa.kind === "fx" ? "fx_" : sa.kind === "particle" ? "p_" : "bg_";
    return sa.key.startsWith(prefix) ? sa.key : prefix + sa.key;
  }
  function blendMode(b?: string): string {
    return b === "add" ? "plus-lighter" : b === "screen" ? "screen" : "normal";
  }
  type Placement = {
    fit?: string;
    anchor?: (number | null)[] | null;
    pos?: (number | null)[] | null;
    height?: number | null;
    overscan?: number | null;
    blend?: string;
  };
  type SceneLayer = { id: string; name: string; kind: string; url: string | null; placement: Placement };
  // Every placed scene asset for the current orientation (url null = declared but not generated).
  // `id` = the orientation-independent base key, so hiding a layer persists across orientations.
  const sceneLayers = $derived.by<SceneLayer[]>(() => {
    const assets = config?.scene?.assets ?? [];
    const out: SceneLayer[] = [];
    for (const sa of assets) {
      if (!sa.placement) continue; // no placement → not part of the composited scene
      const vlist = sa.variants ?? [];
      const v = vlist.find((x) => x.key === orientation) ?? vlist[0];
      const base = sceneBaseKey(sa);
      const key = v?.key ? `${base}_${v.key}` : base;
      out.push({
        id: base,
        name: sa.name?.trim() || base,
        kind: sa.kind,
        url: sceneArt[key] ?? null,
        placement: { ...sa.placement, ...(v?.placement ?? {}) },
      });
    }
    return out;
  });
  // What actually paints: generated layers the user hasn't hidden, in z-order (back → front).
  const visibleScene = $derived(sceneLayers.filter((l) => l.url && !hiddenLayers.has(l.id)));
  const hasScene = $derived(sceneLayers.some((l) => l.url));
  // Reassign a new Set — a plain Set in $state isn't deeply reactive.
  function toggleLayer(key: string) {
    const next = new Set(hiddenLayers);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    hiddenLayers = next;
  }

  const mascotHpx = $derived(stageH * 0.9);
  const mascotWpx = $derived(mascotHpx * MASCOT_AR);
  const mascotLeftPx = $derived(
    mascotSide === "right" ? stageW - mascotWpx - stageW * 0.01 : stageW * 0.01,
  );

  // A believable spread: pip/low symbols common, wild/scatter/bonus rare.
  const WEIGHT: Record<SymbolRole, number> = {
    low: 6,
    high: 4,
    special: 2,
    wild: 1,
    scatter: 1,
    bonus: 1,
    expandingWild: 1,
  };
  function featureWord(role: SymbolRole, name: string): string | null {
    switch (role) {
      case "wild":
      case "expandingWild":
        return "WILD";
      case "scatter":
        return "SCATTER";
      case "bonus":
        return "BONUS";
      case "special":
        return name.toUpperCase().slice(0, 12);
      default:
        return null;
    }
  }

  function shuffle() {
    if (!symbols.length) {
      grid = [];
      return;
    }
    const pool: Sym[] = [];
    for (const s of symbols) for (let i = 0; i < (WEIGHT[s.role] ?? 3); i++) pool.push(s);
    grid = Array.from({ length: cols * rows }, () => pool[Math.floor(Math.random() * pool.length)]);
  }

  // Active art as a displayable data URL — the webp-else-raw recipe used across the app.
  async function art(key: string, rec: AssetRecord | undefined): Promise<string | null> {
    if (!rec) return null;
    const active = assetStatus(rec).activeVariation;
    if (!active) return null;
    try {
      return assetStatus(rec).processed
        ? await unwrap(commands.getVariationStageImage(gameId, key, active.id, "webp"))
        : await unwrap(commands.getVariationImage(gameId, key, active.id));
    } catch {
      return null;
    }
  }

  onMount(() => {
    void (async () => {
      try {
        const project = await unwrap(commands.getProject(gameId));
        config = project.config;
        const descs = await commands.deriveAssets(project.config);
        const recs = await unwrap(commands.listAssetRecords(gameId));
        const recMap = new Map(recs.map((r) => [r.key, r]));

        // Symbols — iterate the config's declared symbols (base only; role known). Loaded in
        // parallel so first paint isn't gated on a serial chain of base64 reads.
        const loaded = await Promise.all(
          project.config.symbols.map(async (sd) => {
            const key = `symbol_${sd.key}`;
            return { sd, url: await art(key, recMap.get(key)) };
          }),
        );
        symbols = loaded
          .filter((x) => x.url)
          .map((x) => ({ key: x.sd.key, role: x.sd.role, name: x.sd.name, url: x.url as string }));
        missing = loaded.filter((x) => !x.url).map((x) => x.sd.key);

        // Background — prefer a base plate; scene plates map to kind "background" too.
        const bgDescs = descs.filter((d) => d.kind === "background");
        const pickBg = (orient: string) =>
          bgDescs.find((d) => d.key.includes(orient) && !d.key.includes("feature")) ??
          bgDescs.find((d) => d.key.includes(orient)) ??
          bgDescs.find((d) => recMap.has(d.key));
        const lDesc = pickBg("landscape");
        const pDesc = pickBg("portrait");
        bgLandscape = lDesc ? await art(lDesc.key, recMap.get(lDesc.key)) : null;
        bgPortrait = pDesc ? await art(pDesc.key, recMap.get(pDesc.key)) : null;

        // Scene layer stack — load every declared scene variant that has art (both orientations),
        // so the composited background shows all its plates/layers/sprites, not just the base.
        const sceneKeys = new Set<string>();
        for (const sa of project.config.scene?.assets ?? []) {
          if (!sa.placement) continue;
          const base = sceneBaseKey(sa);
          if (sa.variants?.length) for (const v of sa.variants) sceneKeys.add(v.key ? `${base}_${v.key}` : base);
          else sceneKeys.add(base);
        }
        const sceneEntries = await Promise.all(
          [...sceneKeys].map(async (k) => [k, await art(k, recMap.get(k))] as const),
        );
        sceneArt = Object.fromEntries(sceneEntries.filter(([, url]) => url)) as Record<string, string>;

        // Mascot + reel chrome.
        const mDesc = descs.find((d) => d.kind === "mascot");
        mascotUrl = mDesc ? await art(mDesc.key, recMap.get(mDesc.key)) : null;
        frameUrl = await art("reel_frame", recMap.get("reel_frame"));
        reelBgUrl = await art("reel_background", recMap.get("reel_background"));

        shuffle();
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        loading = false;
      }
    })();
  });
</script>

<svelte:window
  onclick={(e) => {
    if (layersOpen && layersEl && !layersEl.contains(e.target as Node)) layersOpen = false;
  }}
  onkeydown={(e) => e.key === "Escape" && (layersOpen = false)}
/>

<div class="preview density-compact">
  <div class="topbar">
    <div class="lead">
      <span class="u-label">Game preview</span>
      <h2 class="title">{config?.name || gameId}</h2>
      {#if config}
        <span class="grid-tag mono">{cols}×{rows}</span>
      {/if}
    </div>

    <div class="controls">
      <button class="gold shuffle" onclick={shuffle} disabled={!symbols.length}>⤮ Shuffle</button>
      <div class="seg" role="group" aria-label="orientation">
        <button class:on={orientation === "landscape"} onclick={() => (orientation = "landscape")}>
          Landscape
        </button>
        <button class:on={orientation === "portrait"} onclick={() => (orientation = "portrait")}>
          Portrait
        </button>
      </div>
      <div class="toggles">
        {#if hasScene}
          <div class="layers-menu" bind:this={layersEl}>
            <button
              class="ghost lyr-btn"
              class:on={layersOpen}
              onclick={() => (layersOpen = !layersOpen)}
              title="show / hide individual scene layers"
            >
              ▤ Layers ({visibleScene.length}/{sceneLayers.filter((l) => l.url).length})
            </button>
            {#if layersOpen}
              <div class="layers-pop card">
                <div class="pop-head">Scene layers <span class="dim">front on top</span></div>
                {#each [...sceneLayers].reverse() as l (l.id)}
                  <label class="layer-row" class:missing={!l.url}>
                    <input
                      type="checkbox"
                      checked={!!l.url && !hiddenLayers.has(l.id)}
                      disabled={!l.url}
                      onchange={() => toggleLayer(l.id)}
                    />
                    <span class="layer-name">{l.name}</span>
                    <span class="layer-kind">{l.url ? l.kind : "not generated"}</span>
                  </label>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
        {#if frameUrl}
          <label class="tog"><input type="checkbox" bind:checked={showFrame} /> Frame</label>
        {/if}
        {#if mascotUrl}
          <label class="tog"><input type="checkbox" bind:checked={showMascot} /> Mascot</label>
          {#if showMascot}
            <button
              class="ghost side"
              title="flip mascot side"
              onclick={() => (mascotSide = mascotSide === "left" ? "right" : "left")}>⇄</button
            >
          {/if}
        {/if}
        <label class="tog"><input type="checkbox" bind:checked={showCells} /> Cells</label>
        <label class="tog"><input type="checkbox" bind:checked={showWords} /> Feature words</label>
      </div>
    </div>
  </div>

  <div class="viewport">
    {#if loading}
      <p class="msg">Loading the game's assets…</p>
    {:else if error}
      <p class="msg err">{error}</p>
    {:else}
      <div
        class="stage"
        class:portrait={orientation === "portrait"}
        bind:clientWidth={stageW}
        bind:clientHeight={stageH}
      >
        {#if hasScene}
          <!-- The scene layer stack, composited back → front (z = array order), minus hidden. -->
          {#each visibleScene as layer (layer.id)}
            {@const p = layer.placement}
            {@const anchored =
              (p.fit || (layer.kind === "sprite" || layer.kind === "fx" || layer.kind === "particle" ? "anchored" : "cover")) === "anchored"}
            {#if anchored}
              <img
                class="scene-sprite"
                src={layer.url}
                alt=""
                draggable="false"
                style:left="{(p.pos?.[0] ?? 0.5) * stageW}px"
                style:top="{(p.pos?.[1] ?? 0.5) * stageH}px"
                style:height="{(p.height ?? 0.3) * stageH}px"
                style:transform="translate({-(p.anchor?.[0] ?? 0.5) * 100}%, {-(p.anchor?.[1] ?? 0.5) * 100}%)"
                style:mix-blend-mode={blendMode(p.blend)}
              />
            {:else}
              <img
                class="scene-cover"
                src={layer.url}
                alt=""
                draggable="false"
                style:transform="scale({1 + (p.overscan ?? 0)})"
                style:mix-blend-mode={blendMode(p.blend)}
              />
            {/if}
          {/each}
        {:else if bg}
          <img class="bg" src={bg} alt="" draggable="false" />
        {:else}
          <div class="bg nobg"></div>
        {/if}

        {#if stageW > 0}
          {#if reelBgUrl}
            <div
              class="reelbg"
              style:left="{panelLpx}px"
              style:top="{panelTpx}px"
              style:width="{panelWpx}px"
              style:height="{panelHpx}px"
              style:border-width="{Math.min(panelWpx, panelHpx) * 0.094}px"
              style:border-image-source="url({reelBgUrl})"
            ></div>
          {:else}
            <div
              class="panel"
              style:left="{panelLpx}px"
              style:top="{panelTpx}px"
              style:width="{panelWpx}px"
              style:height="{panelHpx}px"
            ></div>
          {/if}

          {#each grid as s, i (i)}
            {@const c = i % cols}
            {@const r = Math.floor(i / cols)}
            <div
              class="cell"
              class:socket={showCells}
              style:left="{boardLpx + c * cellPx + gap / 2}px"
              style:top="{boardTpx + r * cellPx + gap / 2}px"
              style:width="{cellPx - gap}px"
              style:height="{cellPx - gap}px"
            >
              {#if s}
                <img src={s.url} alt={s.name} draggable="false" />
                {#if showWords && featureWord(s.role, s.name)}
                  <span class="word">{featureWord(s.role, s.name)}</span>
                {/if}
              {/if}
            </div>
          {/each}

          {#if frameUrl && showFrame}
            {@const tx = panelWpx * 0.1667}
            {@const ty = panelHpx * 0.1667}
            <div
              class="frame"
              style:left="{panelLpx - tx}px"
              style:top="{panelTpx - ty}px"
              style:width="{panelWpx + 2 * tx}px"
              style:height="{panelHpx + 2 * ty}px"
              style:border-width="{ty}px {tx}px"
              style:border-image-source="url({frameUrl})"
            ></div>
          {/if}

          {#if mascotUrl && showMascot}
            <img
              class="mascot"
              src={mascotUrl}
              alt="mascot"
              draggable="false"
              style:left="{mascotLeftPx}px"
              style:height="{mascotHpx}px"
              style:width="{mascotWpx}px"
            />
          {/if}
        {/if}
      </div>

      {#if !symbols.length}
        <p class="hint floating">
          No processed symbols yet — generate &amp; process some symbols, then they'll fill the reels.
        </p>
      {:else}
        <p class="hint">
          {symbols.length} symbol{symbols.length > 1 ? "s" : ""} at true cell size
          {#if missing.length}
            · {missing.length} not ready ({missing.slice(0, 3).join(", ")}{missing.length > 3
              ? "…"
              : ""}){/if}
          {#if !bg} · no background yet{/if}
        </p>
      {/if}
    {/if}
  </div>
</div>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--ink-2);
  }

  .topbar {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
    background: var(--ink);
  }
  .lead {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: 0.95rem;
    font-weight: var(--weight-semibold);
    color: var(--bone);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .grid-tag {
    font-size: var(--text-xs);
    color: var(--ash);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .shuffle {
    white-space: nowrap;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .seg button {
    background: var(--ink-3);
    border: none;
    border-radius: 0;
    color: var(--ash);
    font-size: var(--text-xs);
    padding: 0.28rem 0.6rem;
  }
  .seg button.on {
    background: var(--wash);
    color: var(--bone);
  }
  .toggles {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }
  .tog {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
    color: var(--bone-dim);
    cursor: pointer;
    white-space: nowrap;
  }
  .side {
    padding: 0.15rem 0.4rem;
    font-size: 0.8rem;
  }

  .layers-menu {
    position: relative;
  }
  .lyr-btn {
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .lyr-btn.on {
    color: var(--bone);
    background: var(--wash);
  }
  .layers-pop {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: var(--z-dock);
    min-width: 224px;
    max-height: 62vh;
    overflow: auto;
    padding: var(--space-2);
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .pop-head {
    display: flex;
    justify-content: space-between;
    font-size: var(--text-xs);
    color: var(--bone-dim);
    padding: 0.15rem 0.35rem 0.4rem;
    border-bottom: 1px solid var(--line);
    margin-bottom: 0.2rem;
  }
  .pop-head .dim {
    color: var(--ash-deep);
  }
  .layer-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0.3rem 0.35rem;
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .layer-row:hover {
    background: var(--wash);
  }
  .layer-name {
    flex: 1;
    color: var(--bone);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .layer-kind {
    color: var(--ash);
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    flex: none;
  }
  .layer-row.missing {
    cursor: default;
  }
  .layer-row.missing .layer-name {
    color: var(--ash-deep);
  }

  .viewport {
    flex: 1;
    min-height: 0;
    container-type: size;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    position: relative;
  }

  /* Fit a canvas-aspect box inside the viewport (whichever dimension binds first). */
  .stage {
    position: relative;
    width: min(100cqw, 100cqh * 16 / 9);
    aspect-ratio: 16 / 9;
    background: var(--void);
    border-radius: var(--radius-sm);
    overflow: hidden;
    box-shadow: var(--elev-2);
  }
  .stage.portrait {
    width: min(100cqw, 100cqh * 9 / 16);
    aspect-ratio: 9 / 16;
  }

  .bg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    user-select: none;
    z-index: 0;
  }
  .bg.nobg {
    background: radial-gradient(120% 90% at 50% 25%, var(--ink-4), var(--void));
  }
  /* Scene stack: full-bleed plates/layers and anchored set-piece sprites, back → front. */
  .scene-cover {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    user-select: none;
    z-index: 0;
  }
  .scene-sprite {
    position: absolute;
    width: auto;
    object-fit: contain;
    user-select: none;
    pointer-events: none;
    z-index: 0;
  }

  .reelbg {
    position: absolute;
    box-sizing: border-box;
    border-style: solid;
    border-color: transparent;
    border-image-slice: 96 fill;
    border-image-repeat: stretch;
    pointer-events: none;
    z-index: 1;
  }
  /* Default reel container when the game has no reel-background asset yet. */
  .panel {
    position: absolute;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--void) 55%, transparent);
    box-shadow:
      inset 0 2px 14px rgb(0 0 0 / 0.45),
      0 1px 0 rgb(255 255 255 / 0.04);
    border: 1px solid color-mix(in srgb, var(--bone) 6%, transparent);
    pointer-events: none;
    z-index: 1;
  }
  .frame {
    position: absolute;
    box-sizing: border-box;
    border-style: solid;
    border-color: transparent;
    border-image-slice: 128;
    border-image-repeat: stretch;
    pointer-events: none;
    z-index: 3;
  }

  .cell {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
    z-index: 2;
  }
  /* The grid made explicit: each cell is a seated socket (pure paint — no layout impact). */
  .cell.socket {
    border-radius: calc(var(--radius-sm) - 1px);
    background: color-mix(in srgb, var(--void) 24%, transparent);
    border: 1px solid color-mix(in srgb, var(--bone) 8%, transparent);
    box-shadow: inset 0 1px 4px rgb(0 0 0 / 0.35);
  }
  .cell img {
    max-width: 90%;
    max-height: 90%;
    object-fit: contain;
    user-select: none;
  }
  .word {
    position: absolute;
    bottom: 6%;
    left: 50%;
    transform: translateX(-50%);
    font-family: var(--font-sans);
    font-weight: var(--weight-semibold);
    font-size: clamp(0.5rem, 1.4cqw, 0.9rem);
    letter-spacing: 0.06em;
    color: #fff;
    background: color-mix(in srgb, var(--void) 62%, transparent);
    padding: 1px 6px;
    border-radius: 3px;
    text-shadow: 0 1px 2px rgb(0 0 0 / 0.9);
    white-space: nowrap;
  }

  .mascot {
    position: absolute;
    bottom: 0;
    object-fit: contain;
    object-position: bottom center;
    pointer-events: none;
    user-select: none;
    filter: drop-shadow(0 4px 14px rgb(0 0 0 / 0.45));
    z-index: 4;
  }

  .msg {
    color: var(--bone-dim);
    font-size: 0.85rem;
  }
  .msg.err {
    color: var(--oxblood);
  }
  .hint {
    position: absolute;
    bottom: var(--space-3);
    left: 50%;
    transform: translateX(-50%);
    font-size: var(--text-xs);
    color: var(--ash);
    background: color-mix(in srgb, var(--ink) 80%, transparent);
    padding: 2px 10px;
    border-radius: var(--radius-sm);
    white-space: nowrap;
  }
  .hint.floating {
    color: var(--bone-dim);
  }
</style>
