<script lang="ts">
  /**
   * Full-screen parallax Layers view for one background asset. Flow: Propose (AI depth
   * bands) → adjust selections (SAM click / brush / lasso via the shared EditCanvas) →
   * Cut layers → Preview (gaps show magenta) → Fill hidden → Export layers.
   * The approved concept's pixels ARE the layers — fills only touch hidden bands.
   */
  import { listen } from "@tauri-apps/api/event";
  import {
    commands,
    unwrap,
    type InpaintState,
    type Layer,
    type LayersDoc,
    type SamPrompt,
  } from "$lib/ipc";
  import { goProducer } from "$lib/stores/app.svelte";
  import EditCanvas, { type Overlay } from "../studio/EditCanvas.svelte";
  import LayerList from "./LayerList.svelte";

  let { gameId, assetKey }: { gameId: string; assetKey: string } = $props();

  const PALETTE = ["#6fa7dd", "#7fc584", "#c9a65a", "#b87d7d", "#9b7db8", "#7db8ae"];

  let doc = $state<LayersDoc | null>(null);
  let error = $state("");
  let statusMsg = $state("");

  let sourceUrl = $state<string | null>(null);
  let maskUrls = $state<Record<string, string>>({});
  let selectedId = $state<string | null>(null);
  let prompts = $state<SamPrompt[]>([]);
  let candidate = $state<{ url: string } | null>(null);
  let applying = $state(false);

  let tool = $state<"point" | "brush" | "erase" | "lasso" | "lassoErase" | "outline">("point");
  let brushSize = $state(40);
  let manualOpen = $state(false);
  let segmenting = $state(false);
  let segTimer: ReturnType<typeof setTimeout> | null = null;

  // Outline editing: the traced rings of the selected layer's mask.
  let outlineRings = $state<[number, number][][] | null>(null);
  let outlineTol = $state(3);
  let tracing = $state(false);

  let samReady = $state<boolean | null>(null);
  let downloading = $state(false);
  let dlProgress = $state(0);
  /** Depth model (Depth Anything V2) — powers Propose; SAM only corrects. */
  let depthReady = $state<boolean | null>(null);
  let depthDownloading = $state(false);

  let proposing = $state(false);
  let proposeCount = $state(4);
  let cutting = $state(false);
  let filling = $state(false);
  let exporting = $state(false);
  const busy = $derived(proposing || cutting || filling || exporting);

  let fillStates = $state<Record<string, InpaintState>>({});

  const maskReady = $derived(
    Object.fromEntries((doc?.layers ?? []).map((l) => [l.id, !!l.maskHash])) as Record<
      string,
      boolean
    >,
  );
  const selIdx = $derived(doc?.layers.findIndex((l) => l.id === selectedId) ?? -1);
  const cutCount = $derived((doc?.layers ?? []).filter((l) => l.bbox).length);
  const selectionCount = $derived(
    (doc?.layers ?? []).filter((l, i) => i > 0 && l.maskHash).length,
  );
  const selectionsNeeded = $derived((doc?.layers.length ?? 1) - 1);

  const nextStep = $derived.by(() => {
    if (!doc) return "";
    if (selectionCount === 0) return "Propose layers with AI — or click each layer's scenery to select it";
    if (selectionCount < selectionsNeeded) return `Select the remaining ${selectionsNeeded - selectionCount} layer${selectionsNeeded - selectionCount === 1 ? "" : "s"}, then Cut`;
    if (cutCount === 0) return "Selections ready — Cut layers";
    if (Object.values(fillStates).some((s) => s === "pending" || s === "stale"))
      return "Fill hidden — the bands nearer layers cover";
    return "Layers cut and filled — Export layers (stacking/motion live in the game code)";
  });

  const overlays = $derived<Overlay[]>(
    (doc?.layers ?? [])
      .map((l, i) => {
        if (i === 0) return null; // the catch-all has no mask to tint
        const isSel = l.id === selectedId;
        const url = isSel && candidate ? candidate.url : maskUrls[l.id];
        return url ? { id: l.id, url, color: PALETTE[i % PALETTE.length], active: isSel } : null;
      })
      .filter((o): o is Overlay => o !== null),
  );


  // ── Load ────────────────────────────────────────────────────────────────────
  $effect(() => {
    (async () => {
      try {
        doc = await unwrap(commands.layersOpen(gameId, assetKey));
        sourceUrl = await unwrap(commands.layersGetImage(gameId, assetKey, "source.png"));
        const status = await unwrap(commands.studioSamStatus()).catch(() => null);
        samReady = status?.state === "ready";
        const dstatus = await unwrap(commands.layersDepthStatus()).catch(() => null);
        depthReady = dstatus?.state === "ready";
        for (const l of doc.layers) {
          if (l.maskHash) loadMask(l.id);
        }
        const first = doc.layers.find((_, i) => i > 0);
        if (first) selectLayer(first.id);
        await refreshFill();
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    })();
    const unSam = listen<{ received: number; total: number }>("studio://sam-progress", (e) => {
      dlProgress = e.payload.total > 0 ? e.payload.received / e.payload.total : 0;
    });
    const unDepth = listen<{ received: number; total: number }>(
      "layers://depth-progress",
      (e) => {
        dlProgress = e.payload.total > 0 ? e.payload.received / e.payload.total : 0;
      },
    );
    const unFill = listen<{ layerId: string; state: string; message: string | null }>(
      "layers://fill-progress",
      (e) => {
        const { layerId, state, message } = e.payload;
        statusMsg =
          state === "start"
            ? `Filling ${layerId}… (10–30 s per layer)`
            : state === "error"
              ? `${layerId} failed: ${message ?? "unknown error"}`
              : `${layerId}: ${state}`;
      },
    );
    return () => {
      unSam.then((f) => f());
      unDepth.then((f) => f());
      unFill.then((f) => f());
      if (segTimer) clearTimeout(segTimer);
    };
  });

  async function loadMask(layerId: string) {
    const url = await unwrap(
      commands.layersGetImage(gameId, assetKey, `parts/${layerId}/mask.png`),
    ).catch(() => null);
    if (url) maskUrls = { ...maskUrls, [layerId]: url };
  }

  async function refreshFill() {
    if (!doc || cutCount === 0) return;
    const st = await unwrap(commands.layersFillStatus(gameId, assetKey)).catch(() => null);
    if (st) fillStates = Object.fromEntries(st.map((s) => [s.layerId, s.state]));
  }


  async function persist() {
    if (!doc) return;
    try {
      doc = await unwrap(commands.layersSave(gameId, assetKey, $state.snapshot(doc) as LayersDoc));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  // ── Selection (SAM + manual) ─────────────────────────────────────────────────
  function selectLayer(id: string) {
    selectedId = id;
    candidate = null;
    const layer = doc?.layers.find((l) => l.id === id);
    prompts = layer?.prompts ?? [];
    outlineRings = null;
    // A layer that already has a selection opens straight into its editable outline —
    // the AI's cut IS the lasso; points are only for seeding a selection from nothing.
    const idx = doc?.layers.findIndex((l) => l.id === id) ?? -1;
    if (idx > 0 && layer?.maskHash) {
      traceOutline();
    } else if (tool === "outline") {
      tool = "point";
    }
  }

  function onpoint(x: number, y: number, positive: boolean) {
    if (!selectedId || selIdx === 0) return;
    prompts = [...prompts, { x, y, label: positive ? "positive" : "negative" }];
    scheduleSegment();
  }

  /** Delete a prompt dot: the clicked one, or the last (right-click empty = undo). */
  function onPointRemove(index: number) {
    if (!selectedId || !prompts.length) return;
    prompts = index >= 0 ? prompts.filter((_, i) => i !== index) : prompts.slice(0, -1);
    if (prompts.some((p) => p.label === "positive")) scheduleSegment();
    else candidate = null;
  }

  function scheduleSegment() {
    if (segTimer) clearTimeout(segTimer);
    segTimer = setTimeout(runSegment, 180);
  }

  async function runSegment() {
    if (!selectedId || !samReady || !prompts.some((p) => p.label === "positive")) return;
    segmenting = true;
    error = "";
    try {
      const r = await unwrap(commands.layersSegment(gameId, assetKey, $state.snapshot(prompts)));
      candidate = { url: r.maskDataUrl };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      segmenting = false;
    }
  }

  // Cloud "paint-out" cut: semantic selection for bands where clicks struggle.
  let cloudBusy = $state(false);
  async function cloudCut() {
    if (!selectedId) return;
    cloudBusy = true;
    error = "";
    statusMsg = "Cloud paint-out: repainting everything except this layer… (~20 s, uses OpenAI)";
    try {
      const r = await unwrap(commands.layersCloudCut(gameId, assetKey, selectedId));
      candidate = { url: r.maskDataUrl };
      statusMsg = "Check the tint, then Apply selection.";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      statusMsg = "";
    } finally {
      cloudBusy = false;
    }
  }

  function resetPoints() {
    prompts = [];
    candidate = null;
  }

  // ── Outline editing (adjust the traced boundary instead of re-stroking) ─────
  async function traceOutline() {
    if (!selectedId) return;
    tracing = true;
    error = "";
    try {
      const raw = await unwrap(
        commands.layersTraceOutline(gameId, assetKey, selectedId, outlineTol),
      );
      // specta emits floats as number|null — normalize.
      outlineRings = raw.map((ring) => ring.map((p) => [p[0] ?? 0, p[1] ?? 0] as [number, number]));
      tool = "outline";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      tracing = false;
    }
  }

  /** Rasterize the edited rings (even-odd: holes stay holes) into a candidate mask. */
  function onOutlineChange(rings: [number, number][][]) {
    if (!doc) return;
    outlineRings = rings;
    const c = document.createElement("canvas");
    c.width = doc.source.width;
    c.height = doc.source.height;
    const ctx = c.getContext("2d")!;
    ctx.fillStyle = "#000000";
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    for (const ring of rings) {
      if (ring.length < 3) continue;
      ctx.moveTo(ring[0][0], ring[0][1]);
      for (const p of ring.slice(1)) ctx.lineTo(p[0], p[1]);
      ctx.closePath();
    }
    ctx.fill("evenodd");
    candidate = { url: c.toDataURL("image/png") };
  }

  function exitOutline() {
    outlineRings = null;
    tool = "point";
  }

  async function applyMask() {
    if (!selectedId) return;
    const url = candidate?.url ?? maskUrls[selectedId];
    if (!url) return;
    applying = true;
    error = "";
    try {
      doc = await unwrap(
        commands.layersSetMask(gameId, assetKey, selectedId, url, $state.snapshot(prompts)),
      );
      maskUrls = { ...maskUrls, [selectedId]: url };
      candidate = null;
      // Back to the editable outline of what was just applied.
      await traceOutline();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      applying = false;
    }
  }

  // ── Depth propose ────────────────────────────────────────────────────────────
  // Depth Anything V2 estimates the scene's depth and slices it into bands — the layer
  // structure comes from actual depth, not object segmentation. SAM stays for corrections.
  async function propose() {
    if (!doc) return;
    if (
      selectionCount > 0 &&
      !confirm("Proposing replaces the current layers and their selections. Continue?")
    ) {
      return;
    }
    proposing = true;
    error = "";
    try {
      statusMsg = "Estimating scene depth…";
      doc = await unwrap(commands.layersProposeDepth(gameId, assetKey, proposeCount));
      maskUrls = {};
      fillStates = {};
      for (const l of doc.layers) {
        if (l.maskHash) loadMask(l.id);
      }
      statusMsg = "Depth bands proposed — check each layer's outline, then Cut layers.";
      const first = doc.layers.find((_, i) => i > 0);
      if (first) selectLayer(first.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      statusMsg = "";
    } finally {
      proposing = false;
    }
  }

  async function downloadSam() {
    downloading = true;
    error = "";
    try {
      await unwrap(commands.studioSamDownload(true));
      samReady = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      downloading = false;
    }
  }

  async function downloadDepth() {
    depthDownloading = true;
    error = "";
    try {
      await unwrap(commands.layersDepthDownload());
      depthReady = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      depthDownloading = false;
    }
  }

  // ── Cut / fill / export ─────────────────────────────────────────────────────
  async function cutLayers() {
    cutting = true;
    error = "";
    try {
      doc = await unwrap(commands.layersCut(gameId, assetKey));
      statusMsg = "Layers cut — check the Preview (show gaps), then Fill hidden.";
      await refreshFill();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      cutting = false;
    }
  }

  async function fillLayers(only: string | null, force: boolean) {
    filling = true;
    error = "";
    try {
      doc = await unwrap(commands.layersFill(gameId, assetKey, only, force));
      statusMsg = "Fill complete — hidden areas are painted in behind nearer layers.";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      filling = false;
      await refreshFill();
    }
  }

  async function exportLayers() {
    exporting = true;
    error = "";
    try {
      const rep = await unwrap(commands.layersExport(gameId, assetKey));
      statusMsg =
        `Exported ${rep.files.length} layers (${rep.width}×${rep.height})` +
        (rep.stale.length ? ` · exported from raw cuts (no fresh fill): ${rep.stale.join(", ")}` : "") +
        " — game Export now ships this set.";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      exporting = false;
    }
  }

  // ── Layer list actions ──────────────────────────────────────────────────────
  function slugify(s: string): string {
    return s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "");
  }

  function addLayer(name: string) {
    if (!doc) return;
    const slug = slugify(name) || `layer_${doc.layers.length}`;
    let id = slug;
    let n = 2;
    while (doc.layers.some((l) => l.id === id)) id = `${slug}_${n++}`;
    const speed = Math.min((doc.layers.at(-1)?.speed ?? 0.5) + 0.1, 1.25);
    doc.layers.push({
      id,
      name: name.trim() || id,
      prompts: [],
      maskHash: null,
      bbox: null,
      filledHash: null,
      filledBbox: null,
      speed,
    } as Layer);
    persist();
    selectLayer(id);
  }

  function deleteLayer(id: string) {
    if (!doc) return;
    doc.layers = doc.layers.filter((l) => l.id !== id);
    if (selectedId === id) selectedId = null;
    persist();
  }

  function moveLayer(id: string, dir: -1 | 1) {
    if (!doc) return;
    const i = doc.layers.findIndex((l) => l.id === id);
    const j = i + dir;
    if (i <= 0 || j <= 0 || j >= doc.layers.length) return;
    const arr = [...doc.layers];
    [arr[i], arr[j]] = [arr[j], arr[i]];
    doc.layers = arr;
    persist();
  }

  function renameLayer(id: string, name: string) {
    const l = doc?.layers.find((l) => l.id === id);
    if (l && name.trim()) {
      l.name = name.trim();
      persist();
    }
  }


  function back() {
    goProducer(gameId, assetKey);
  }

  function onkeydown(e: KeyboardEvent) {
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") return;
    if (e.key === "Escape") {
      e.preventDefault();
      back();
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "v") tool = "point";
    else if (k === "b") tool = "brush";
    else if (k === "e") tool = "erase";
    else if (k === "l") tool = e.shiftKey ? "lassoErase" : "lasso";
    else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="layers-view density-compact">
  <header class="top">
    <span class="u-label">Cut into layers</span>
    <div class="actions">
      <button
        onclick={exportLayers}
        disabled={busy || cutCount < 2}
        title={cutCount < 2 ? "needs cut layers first" : "write the layer set the game Export ships"}
      >
        {exporting ? "Exporting…" : "Export layers"}
      </button>
    </div>
  </header>

  {#if error}
    <div class="strip err">{error}</div>
  {:else if downloading}
    <div class="strip"><progress value={dlProgress}></progress></div>
  {:else if statusMsg}
    <div class="strip ok">{statusMsg}</div>
  {:else}
    <div class="strip cue">▸ {nextStep}</div>
  {/if}

  {#if doc}
    <div class="body">
      <aside class="left">
        <div class="propose">
          {#if depthReady === false}
            <button class="gold" onclick={downloadDepth} disabled={depthDownloading}>
              {depthDownloading ? "Downloading depth model…" : "Get depth model (~190 MB)"}
            </button>
            <p class="muted tiny dl-note">
              Estimates real scene depth and slices it into bands — the accurate way to
              propose layers.
            </p>
          {:else}
            <div class="prow">
              <button class="gold" onclick={propose} disabled={busy || depthReady !== true}>
                {proposing ? "Proposing…" : "✦ Propose by depth"}
              </button>
              <input
                class="mono count"
                type="number"
                min="2"
                max="8"
                bind:value={proposeCount}
                title="how many depth layers"
              />
            </div>
          {/if}
          {#if samReady === false}
            <button class="ghost sam-dl" onclick={downloadSam} disabled={downloading}>
              {downloading ? "Downloading…" : "Get selection model (375 MB) — for click corrections"}
            </button>
          {/if}
        </div>
        <LayerList
          layers={doc.layers}
          {selectedId}
          {maskReady}
          {fillStates}
          {busy}
          onselect={selectLayer}
          onname={renameLayer}
          onmove={moveLayer}
          ondelete={deleteLayer}
          onadd={addLayer}
          onrefill={(id) => fillLayers(id, true)}
        />
        <div class="left-foot">
          <button
            onclick={cutLayers}
            disabled={busy || selectionCount < selectionsNeeded}
            title={selectionCount < selectionsNeeded
              ? "every layer above the backdrop needs a selection first"
              : "split the scene along the selections"}
          >
            {cutting ? "Cutting…" : cutCount > 0 ? "Re-cut layers" : "Cut layers"}
          </button>
          <button
            onclick={() => fillLayers(null, false)}
            disabled={busy || cutCount === 0}
            title={cutCount === 0 ? "needs cut layers first" : "AI-fill what nearer layers hide"}
          >
            {filling ? "Filling…" : "Fill hidden (AI)"}
          </button>
        </div>
      </aside>

      <section class="center">
          <div class="canvas-holder">
            <div class="tool-rail" role="toolbar" aria-label="selection tools">
              <button class="trail-btn" class:on={tool === "point"} title="AI points (V) — click to select; click a dot to delete it; right-click = undo last; ⇧-click removes an area" onclick={() => (tool = "point")}>◎</button>
              <span class="trail-sep"></span>
              <button class="trail-btn mono" class:on={tool === "brush"} title="Brush (B)" onclick={() => (tool = "brush")}>B</button>
              <button class="trail-btn mono" class:on={tool === "erase"} title="Erase (E)" onclick={() => (tool = "erase")}>E</button>
              <span class="trail-sep"></span>
              <button class="trail-btn mono" class:on={tool === "lasso"} title="Lasso add (L)" onclick={() => (tool = "lasso")}>L+</button>
              <button class="trail-btn mono" class:on={tool === "lassoErase"} title="Lasso remove (⇧L)" onclick={() => (tool = "lassoErase")}>L−</button>
              <span class="trail-sep"></span>
              <button
                class="trail-btn"
                class:on={tool === "outline"}
                disabled={!selectedId || !maskReady[selectedId ?? ""]}
                title={tool === "outline" ? "leave outline editing" : "edit the traced outline — drag vertices, click an edge to add, ⌥-click to remove"}
                onclick={() => (tool === "outline" ? exitOutline() : traceOutline())}
              >✎</button>
              {#if tool === "brush" || tool === "erase"}
                <span class="trail-sep"></span>
                <div class="trail-size" title="brush size">
                  <input type="range" min="4" max="160" bind:value={brushSize} />
                  <span class="mono tiny">{brushSize}</span>
                </div>
              {/if}
            </div>

            {#if selectedId}
              <div class="action-bar">
                <span class="ab-name mono">{selectedId}</span>
                {#if selIdx === 0}
                  <span class="muted tiny">backdrop — owns everything the nearer layers don't claim</span>
                {:else if tool === "outline"}
                  <label class="ab-tol" title="lower = more vertices, finer detail; re-traces from the current selection">
                    <span class="muted tiny">detail {outlineTol}px</span>
                    <input type="range" min="1" max="12" bind:value={outlineTol} onchange={traceOutline} />
                  </label>
                  <button class="ghost ab-btn" onclick={exitOutline}>↩ AI points</button>
                {:else}
                  <button
                    class="ghost ab-btn"
                    onclick={cloudCut}
                    disabled={cloudBusy || applying}
                    title="an image model repaints everything except this layer as flat magenta; keying it gives the selection — paid, ~20 s, semantic"
                  >
                    {cloudBusy ? "☁ Painting…" : "☁ Cloud cut"}
                  </button>
                  {#if candidate || prompts.length}
                    <span class="ab-sep"></span>
                    <button class="ghost ab-btn" onclick={resetPoints}>Reset</button>
                    <button class="gold ab-btn" onclick={applyMask} disabled={applying || !candidate}>
                      {applying ? "Applying…" : "Apply"}
                    </button>
                  {/if}
                {/if}
              </div>
            {/if}
            <EditCanvas
              imageUrl={sourceUrl}
              width={doc.source.width}
              height={doc.source.height}
              {overlays}
              prompts={selectedId && selIdx > 0 && tool !== "outline" ? prompts : []}
              {tool}
              {brushSize}
              outline={tool === "outline" ? outlineRings : null}
              {onpoint}
              onpointremove={onPointRemove}
              onmaskedit={(url) => (candidate = { url })}
              onoutlinechange={onOutlineChange}
            />
            {#if segmenting}<span class="seg-badge mono">selecting…</span>{/if}
          </div>
      </section>

    </div>
  {/if}
</div>

<style>
  .layers-view {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--ink);
  }
  /* Slim toolbar: identity lives in the Animate rail — this row is tabs + actions. */
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-5);
    padding: 0 var(--space-5);
    min-height: 42px;
    border-bottom: 1px solid var(--line);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }
  .strip {
    padding: 0.4rem 1.2rem;
    font-size: 0.75rem;
    border-bottom: 1px solid var(--line);
  }
  .strip.err {
    color: var(--oxblood);
  }
  .strip.ok {
    color: var(--sage);
  }
  .strip.cue {
    color: var(--bone-dim);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 250px;
  }
  /* Layer inventory docks on the RIGHT — tools left on the canvas, list right,
     mirroring the Cut screen. */
  .left {
    order: 2;
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .center {
    order: 1;
  }
  .propose {
    padding: 0.8rem 0.9rem 0;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  .dl-note {
    margin: 0;
    line-height: 1.4;
  }
  .sam-dl {
    font-size: 0.66rem;
    text-align: left;
    padding: 0.2rem 0;
  }
  .prow {
    display: flex;
    gap: 0.4rem;
  }
  .prow .gold {
    flex: 1;
  }
  .count {
    width: 3.2rem;
    text-align: center;
  }
  .left-foot {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.8rem 0.9rem;
    border-top: 1px solid var(--line);
  }
  .center {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }
  .canvas-holder {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    overflow: hidden;
  }
  .tool-rail {
    position: absolute;
    top: var(--space-4);
    left: var(--space-4);
    z-index: var(--z-rail);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    box-shadow: var(--elev-2);
  }
  .trail-btn {
    width: 2.1rem;
    height: 2.1rem;
    display: grid;
    place-items: center;
    padding: 0;
    font-size: var(--text-md);
    color: var(--ash-deep);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
  }
  .trail-btn:hover:not(:disabled) {
    color: var(--bone);
    background: var(--wash);
  }
  .trail-btn.on {
    color: var(--gold);
    border-color: var(--gold-deep);
    background: var(--gold-glow);
  }
  .trail-btn:disabled {
    color: var(--ink-5);
  }
  .trail-sep {
    width: 1.4rem;
    height: 1px;
    background: var(--line);
  }
  .trail-size {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-1);
  }
  .trail-size input {
    width: 2.4rem;
  }
  .action-bar {
    position: absolute;
    bottom: var(--space-4);
    left: 50%;
    transform: translateX(-50%);
    z-index: var(--z-rail);
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-4);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--elev-2);
    max-width: calc(100% - 2 * var(--space-6));
  }
  .ab-name {
    font-size: var(--text-xs);
    color: var(--ash);
    max-width: 10rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ab-btn {
    font-size: var(--text-sm);
    padding: var(--space-2) var(--space-4);
  }
  .ab-sep {
    width: 1px;
    align-self: stretch;
    background: var(--line);
  }
  .ab-tol {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .ab-tol input {
    width: 6rem;
  }
  .seg-badge {
    position: absolute;
    top: 0.6rem;
    left: 0.8rem;
    font-size: 0.62rem;
    color: var(--lapis);
    background: rgba(12, 13, 16, 0.7);
    padding: 0.15rem 0.5rem;
    border-radius: var(--radius-sm);
  }
</style>
