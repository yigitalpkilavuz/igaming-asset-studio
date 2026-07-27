<script lang="ts">
  /**
   * Full-screen Animation Studio for one asset: Cut (segmentation) → Rig (bones) →
   * Animate (timeline). The doc is the single source of truth; each mode edits it and
   * every preview plays the actually-exported skeleton on the real Spine runtime.
   */
  import { commands, unwrap, type StudioDoc } from "$lib/ipc";
  import { goProducer } from "$lib/stores/app.svelte";
  import AnimateMode from "./AnimateMode.svelte";
  import CutMode from "./CutMode.svelte";
  import MeshMode from "./MeshMode.svelte";
  import RigMode from "./RigMode.svelte";

  let { gameId, assetKey }: { gameId: string; assetKey: string } = $props();

  type Mode = "cut" | "rig" | "mesh" | "animate";
  const MODES: { key: Mode; label: string }[] = [
    { key: "cut", label: "Cut" },
    { key: "rig", label: "Rig" },
    { key: "mesh", label: "Mesh" },
    { key: "animate", label: "Animate" },
  ];

  let doc = $state<StudioDoc | null>(null);
  let mode = $state<Mode>("animate");
  let error = $state("");
  let exporting = $state(false);
  let exportMsg = $state("");
  /** Animate-mode instance — the top bar's Bake sheet delegates to it. */
  let anim = $state<{ bakeSheet: () => Promise<string>; canBake: () => boolean } | null>(null);
  let baking = $state(false);
  /** Remount token for Animate mode: bumped when Cut/Rig structurally change the doc. */
  let animRev = $state(0);

  /** The bench has a newer active take than the studio's snapshot (docs with real
   *  work don't auto-follow — updating marks selections/cuts stale). */
  let staleSource = $state(false);
  let updatingSource = $state(false);

  // Load or create the studio doc.
  $effect(() => {
    (async () => {
      try {
        error = "";
        doc = await unwrap(commands.studioOpen(gameId, assetKey));
        // Fresh single-part docs start in Cut mode; segmented ones in Animate.
        mode = doc.parts.length > 1 ? "animate" : "cut";
        // Pristine docs already auto-followed the newest take inside studioOpen —
        // a mismatch here means real work exists on older art.
        const rec = await unwrap(commands.getAssetRecord(gameId, assetKey)).catch(() => null);
        staleSource =
          !!rec?.activeVariation && rec.activeVariation !== doc.source.variationId;
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    })();
  });

  async function updateSource() {
    updatingSource = true;
    try {
      doc = await unwrap(commands.studioReimportSource(gameId, assetKey));
      staleSource = false;
      animRev++; // remount modes on the new pixels
      mode = "cut"; // selections need re-checking against the new art
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      updatingSource = false;
    }
  }

  async function saveDoc(next: StudioDoc): Promise<StudioDoc> {
    const saved = await unwrap(commands.studioSave(gameId, assetKey, next));
    doc = saved;
    return saved;
  }

  async function onCut(updated: StudioDoc) {
    doc = updated;
    animRev++;
  }

  /** Switch mode; entering Animate remounts it so structural Cut/Rig edits are picked up.
   *  (Must NOT be an $effect that bumps animRev on mode — `animRev++` reads its own
   *  dependency and self-loops until Svelte kills the reactive tree.) */
  function setMode(next: Mode) {
    if (next === "animate" && mode !== "animate") animRev++;
    mode = next;
  }

  async function exportSpine() {
    if (!doc) return;
    exporting = true;
    exportMsg = "";
    try {
      const report = await unwrap(commands.studioExport(gameId, assetKey, $state.snapshot(doc)));
      exportMsg =
        `Exported ${report.files.length} files` +
        (report.warnings.length ? ` · ${report.warnings.join("; ")}` : "");
    } catch (e) {
      exportMsg = `Export failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      exporting = false;
    }
  }

  async function bakeSheet() {
    if (!anim) return;
    baking = true;
    exportMsg = "";
    try {
      exportMsg = await anim.bakeSheet();
    } catch (e) {
      exportMsg = `Bake failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      baking = false;
    }
  }

  function back() {
    goProducer(gameId, assetKey);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      back();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="studio density-compact">
  <header class="top">
    <nav class="modes" aria-label="Studio mode">
      {#each MODES as m (m.key)}
        <button class="mtab" class:on={mode === m.key} onclick={() => setMode(m.key)}>
          {m.label}
        </button>
      {/each}
    </nav>
    <div class="actions">
      {#if mode === "animate"}
        <button
          class="ghost"
          onclick={bakeSheet}
          disabled={baking || !anim?.canBake()}
          title={anim?.canBake()
            ? "PNG spritesheet fallback for non-Spine consumers"
            : "needs a clip — create one in the timeline first"}
        >
          {baking ? "Baking…" : "Bake frames"}
        </button>
      {/if}
      <button onclick={exportSpine} disabled={exporting || !doc}>
        {exporting ? "Exporting…" : "Export Spine"}
      </button>
    </div>
  </header>

  {#if error}
    <div class="strip err">{error}</div>
  {/if}
  {#if staleSource}
    <div class="strip warn">
      The asset has a newer take than this studio's source — selections and cuts were
      made on the old art.
      <button class="ghost strip-btn" onclick={updateSource} disabled={updatingSource}>
        {updatingSource ? "Updating…" : "Update source"}
      </button>
    </div>
  {/if}
  {#if exportMsg}
    <div class="strip ok">{exportMsg}</div>
  {/if}

  <div class="body">
    {#if doc}
      {#if mode === "cut"}
        {#key doc.source.sha256}
          <CutMode {gameId} {assetKey} {doc} save={saveDoc} oncut={onCut} ongorig={() => setMode("rig")} />
        {/key}
      {:else if mode === "rig"}
        {#key `${doc.parts.length}:${doc.source.sha256}`}
          <RigMode {gameId} {assetKey} {doc} save={saveDoc} />
        {/key}
      {:else if mode === "mesh"}
        {#key `${doc.parts.length}:${doc.source.sha256}`}
          <MeshMode {gameId} {assetKey} {doc} save={saveDoc} />
        {/key}
      {:else}
        {#key animRev}
          <AnimateMode bind:this={anim} {gameId} {assetKey} {doc} save={saveDoc} />
        {/key}
      {/if}
    {/if}
  </div>
</div>

<style>
  .studio {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--ink);
  }
  /* Slim toolbar: identity lives in the Animate rail — this row is modes + actions. */
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-5);
    padding: 0 var(--space-5);
    min-height: 42px;
    border-bottom: 1px solid var(--line);
  }
  .modes {
    display: flex;
    align-self: stretch;
    gap: var(--space-6);
  }
  .mtab {
    display: flex;
    align-items: center;
    border: none;
    border-bottom: 2px solid transparent;
    border-top: 2px solid transparent;
    border-radius: 0;
    background: transparent;
    padding: 0 var(--space-1);
    color: var(--ash);
    font-size: 0.8rem;
  }
  .mtab:hover:not(:disabled) {
    color: var(--bone-dim);
    background: transparent;
  }
  .mtab.on {
    color: var(--bone);
    border-bottom-color: var(--gold);
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
  .strip.warn {
    color: var(--gold);
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }
  .strip-btn {
    font-size: 0.72rem;
    padding: 0.15rem 0.6rem;
    border: 1px solid var(--gold-deep);
    color: var(--gold);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }
</style>
