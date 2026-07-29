<script lang="ts">
  /**
   * The Animate section: everything about making assets MOVE. A left rail lists the
   * game's animatable assets; the rest hosts the right tool for the selection —
   * the Spine studio (Cut · Rig · Animate) for symbols/mascots, the parallax Layers
   * view for backgrounds and scene plates. One section, two techniques.
   */
  import { commands, unwrap, type AssetDescriptor, type AssetRecord } from "$lib/ipc";
  import { assetStatus } from "$lib/assetStatus";
  import { appState, goSection, selectAnimateAsset } from "$lib/stores/app.svelte";
  import { jobsState } from "$lib/stores/jobs.svelte";
  import LayersView from "../layers/LayersView.svelte";
  import Studio from "../studio/Studio.svelte";
  import MotionLoop from "../producer/MotionLoop.svelte";

  let { gameId }: { gameId: string } = $props();

  let assets = $state<AssetDescriptor[]>([]);
  let records = $state<Record<string, AssetRecord>>({});
  let error = $state("");
  let loading = $state(true);

  $effect(() => {
    void gameId;
    (async () => {
      try {
        const project = await unwrap(commands.getProject(gameId));
        assets = await commands.deriveAssets(project.config);
        const recs = await unwrap(commands.listAssetRecords(gameId));
        records = Object.fromEntries(recs.map((r) => [r.key, r]));
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        loading = false;
      }
    })();
  });

  // Records written by background jobs keep the rail's dots current.
  let seenRev = jobsState.rev;
  $effect(() => {
    void jobsState.rev;
    const fresh = jobsState.recordUpdates.filter((u) => u.rev > seenRev);
    seenRev = jobsState.rev;
    if (fresh.length) {
      const merged = { ...records };
      for (const u of fresh) merged[u.record.key] = u.record;
      records = merged;
    }
  });

  /** Rigged animation (Spine studio) candidates: drawn symbols + the mascot. */
  const rigAssets = $derived(
    assets.filter(
      (a) =>
        (a.kind === "symbol" || a.kind === "mascot") &&
        a.production === "raster" &&
        assetStatus(records[a.key]).generated,
    ),
  );
  /** Parallax (Layers) candidates: drawn backgrounds incl. scene plates. */
  const layerAssets = $derived(
    assets.filter(
      (a) =>
        a.kind === "background" &&
        a.production === "raster" &&
        assetStatus(records[a.key]).generated,
    ),
  );

  const selectedKey = $derived(appState.animateAssetKey);
  const selected = $derived(
    [...rigAssets, ...layerAssets].find((a) => a.key === selectedKey) ?? null,
  );

  // For a symbol/mascot, the same asset can animate two ways: a Spine rig or a spritesheet
  // motion loop (rigging optional). The switch defaults back to the rig each time you pick a
  // new asset — the rig is the primary technique; the loop is one click away.
  let technique = $state<"rig" | "loop">("rig");
  $effect(() => {
    void selectedKey;
    technique = "rig";
  });

  // Auto-select the first available asset when nothing (or something stale) is picked.
  $effect(() => {
    if (loading) return;
    if (selected) return;
    const first = rigAssets[0] ?? layerAssets[0] ?? null;
    if (first && appState.animateAssetKey !== first.key) selectAnimateAsset(first.key);
  });

  function shortName(a: AssetDescriptor): string {
    return a.key.replace(/^symbol_/, "").replace(/^bg_/, "");
  }
  function stateOf(key: string): "drawn" | "ready" {
    return assetStatus(records[key]).ready ? "ready" : "drawn";
  }

  // Rail thumbnails: active variation's processed webp, else raw (Ledger's pattern).
  let thumbs = $state<Record<string, string>>({});
  const requested = new Set<string>();
  $effect(() => {
    for (const a of [...rigAssets, ...layerAssets]) {
      const rec = records[a.key];
      const act = rec?.variations?.find((v) => v.id === rec?.activeVariation);
      if (!act) continue;
      const tag = `${a.key}:${act.id}`;
      if (requested.has(tag)) continue;
      requested.add(tag);
      const hasWebp = act.stages?.some((s) => s.name === "webp");
      const p = hasWebp
        ? commands.getVariationStageImage(gameId, a.key, act.id, "webp")
        : commands.getVariationImage(gameId, a.key, act.id);
      p.then((r) => {
        if (r.status === "ok") thumbs = { ...thumbs, [a.key]: r.data };
      });
    }
  });
</script>

<div class="animate-section density-compact">
  <aside class="rail">
    <div class="rail-head">
      <span class="u-label">Animate</span>
    </div>

    {#if rigAssets.length}
      <span class="group-label u-label">Rigged · Spine</span>
      {#each rigAssets as a (a.key)}
        <button class="row" class:on={a.key === selectedKey} onclick={() => selectAnimateAsset(a.key)}>
          <span class="thumb">
            {#if thumbs[a.key]}<img src={thumbs[a.key]} alt="" />{/if}
          </span>
          <span class="rname mono">{shortName(a)}</span>
          <span class="st st-{stateOf(a.key)}"></span>
        </button>
      {/each}
    {/if}

    {#if layerAssets.length}
      <span class="group-label u-label">Parallax · Layers</span>
      {#each layerAssets as a (a.key)}
        <button class="row" class:on={a.key === selectedKey} onclick={() => selectAnimateAsset(a.key)}>
          <span class="thumb wide">
            {#if thumbs[a.key]}<img src={thumbs[a.key]} alt="" />{/if}
          </span>
          <span class="rname mono">{shortName(a)}</span>
          <span class="st st-{stateOf(a.key)}"></span>
        </button>
      {/each}
    {/if}

    {#if !loading && !rigAssets.length && !layerAssets.length}
      <p class="muted tiny rail-empty">
        Nothing to animate yet — generate a symbol or background in Produce first.
      </p>
    {/if}
  </aside>

  <div class="host">
    {#if error}
      <p class="err pad">{error}</p>
    {:else if selected}
      {#key selected.key}
        {#if selected.kind === "background"}
          <div class="editor"><LayersView {gameId} assetKey={selected.key} /></div>
        {:else}
          <div class="technique-bar">
            <button class="tq" class:on={technique === "rig"} onclick={() => (technique = "rig")}>Spine rig</button>
            <button class="tq" class:on={technique === "loop"} onclick={() => (technique = "loop")}>Motion loop</button>
            <span class="tq-hint muted">
              {technique === "rig"
                ? "cut → rig → animate a skeleton"
                : "spritesheet loop — rigging optional, ships as a spriteSheet"}
            </span>
          </div>
          <div class="editor">
            {#if technique === "loop"}
              <div class="loop-host"><MotionLoop {gameId} assetKey={selected.key} /></div>
            {:else}
              <Studio {gameId} assetKey={selected.key} />
            {/if}
          </div>
        {/if}
      {/key}
    {:else if !loading}
      <div class="empty rise">
        <h2 class="display empty-title">Nothing to animate yet</h2>
        <p class="muted empty-line">
          Animation starts from finished art: generate a symbol or a background in
          Produce, then it appears here — symbols get a Spine rig, backgrounds get
          parallax depth layers.
        </p>
        <button class="gold" onclick={() => goSection("produce")}>Go to Produce</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .animate-section {
    height: 100%;
    display: grid;
    grid-template-columns: 248px 1fr;
    min-height: 0;
  }
  .thumb {
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: var(--radius-sm);
    border: 1px solid var(--line);
    overflow: hidden;
    background:
      repeating-conic-gradient(var(--checker-a) 0% 25%, var(--checker-b) 0% 50%) 0 0 / 10px 10px;
  }
  .thumb.wide {
    width: 44px;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .rail {
    border-right: 1px solid var(--line);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    padding: var(--space-4) 0 var(--space-6);
    background: var(--ink);
  }
  .rail-head {
    padding: var(--space-2) var(--space-5) var(--space-4);
  }
  .group-label {
    padding: var(--space-4) var(--space-5) var(--space-2);
    color: var(--ash-deep);
    font-size: 0.62rem;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: var(--space-2) var(--space-5);
    color: var(--bone-dim);
    font-weight: var(--weight-normal);
  }
  .row .rname {
    flex: 1;
    min-width: 0;
  }
  .row:hover:not(:disabled) {
    background: var(--wash-faint);
    color: var(--bone);
  }
  .row.on {
    background: var(--wash);
    color: var(--bone);
    box-shadow: inset 2px 0 0 var(--gold);
  }
  .rname {
    font-size: var(--text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-round);
    flex: none;
    background: var(--ink-5);
  }
  .st-drawn {
    background: var(--lapis);
  }
  .st-ready {
    background: var(--gold);
  }
  .rail-empty {
    padding: var(--space-4) var(--space-5);
    line-height: 1.5;
  }

  .host {
    position: relative;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .editor {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  /* Technique switch (symbols/mascots): Spine rig vs spritesheet motion loop. */
  .technique-bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--line);
    background: var(--ink);
  }
  .tq {
    padding: var(--space-1) var(--space-3);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--bone-dim);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .tq.on {
    background: var(--gold-glow);
    border-color: var(--gold-deep);
    color: var(--gold);
  }
  .tq-hint {
    margin-left: auto;
    font-size: 0.7rem;
  }
  .loop-host {
    max-width: 440px;
    margin: 0 auto;
    padding: var(--space-6) var(--space-5);
  }
  .pad {
    padding: var(--space-6);
  }
  .err {
    color: var(--oxblood);
  }
  .empty {
    max-width: 420px;
    margin: 18vh auto 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: var(--space-4);
    padding: 0 var(--space-6);
  }
  .empty-title {
    font-size: 1.25rem;
  }
  .empty-line {
    font-size: var(--text-body);
    line-height: 1.55;
    margin: 0 0 var(--space-3);
  }
</style>
