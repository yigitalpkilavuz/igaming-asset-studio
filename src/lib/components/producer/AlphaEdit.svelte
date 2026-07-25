<script lang="ts">
  /**
   * Manual alpha erase on a take: paint areas (brush / lasso) that become fully
   * transparent — the fallback for interior cut-outs and backdrop remnants automatic
   * keying missed. Applies as a NEW active variation, so it's always undoable.
   */
  import { commands, unwrap, type AssetRecord } from "$lib/ipc";
  import EditCanvas from "../studio/EditCanvas.svelte";

  let {
    gameId,
    assetKey,
    variationId,
    imageUrl,
    onclose,
    onapplied,
  }: {
    gameId: string;
    assetKey: string;
    variationId: string;
    imageUrl: string;
    onclose: () => void;
    onapplied: (record: AssetRecord) => void;
  } = $props();

  let dims = $state<{ w: number; h: number } | null>(null);
  let tool = $state<"brush" | "erase" | "lasso" | "lassoErase">("brush");
  let brushSize = $state(40);
  let maskUrl = $state<string | null>(null);
  let applying = $state(false);
  let error = $state("");

  $effect(() => {
    const img = new Image();
    img.onload = () => (dims = { w: img.naturalWidth, h: img.naturalHeight });
    img.src = imageUrl;
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT") return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "b") tool = "brush";
    else if (k === "e") tool = "erase";
    else if (k === "l") tool = e.shiftKey ? "lassoErase" : "lasso";
  }

  async function apply() {
    if (!maskUrl) return;
    applying = true;
    error = "";
    try {
      const b64 = maskUrl.split(",")[1] ?? "";
      const rec = await unwrap(commands.alphaErase(gameId, assetKey, variationId, b64));
      onapplied(rec);
      onclose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      applying = false;
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div class="overlay" onclick={onclose}>
  <div class="card sheet" onclick={(e) => e.stopPropagation()}>
    <div class="head">
      <span class="u-label">Edit alpha — paint what should become transparent</span>
      <button class="ghost" onclick={onclose}>✕</button>
    </div>
    <div class="canvas-holder">
      <div class="tool-rail" role="toolbar" aria-label="erase tools">
        <button class="trail-btn mono" class:on={tool === "brush"} title="Paint to erase (B)" onclick={() => (tool = "brush")}>B</button>
        <button class="trail-btn mono" class:on={tool === "erase"} title="Un-paint (E)" onclick={() => (tool = "erase")}>E</button>
        <span class="trail-sep"></span>
        <button class="trail-btn mono" class:on={tool === "lasso"} title="Lasso area to erase (L) — right-click undoes a point" onclick={() => (tool = "lasso")}>L+</button>
        <button class="trail-btn mono" class:on={tool === "lassoErase"} title="Lasso to un-paint (⇧L)" onclick={() => (tool = "lassoErase")}>L−</button>
        {#if tool === "brush" || tool === "erase"}
          <span class="trail-sep"></span>
          <div class="trail-size" title="brush size">
            <input type="range" min="4" max="160" bind:value={brushSize} />
            <span class="mono tiny">{brushSize}</span>
          </div>
        {/if}
      </div>
      {#if dims}
        <EditCanvas
          {imageUrl}
          width={dims.w}
          height={dims.h}
          overlays={[]}
          prompts={[]}
          {tool}
          {brushSize}
          onpoint={() => {}}
          onmaskedit={(url) => (maskUrl = url)}
        />
      {:else}
        <p class="muted small">loading…</p>
      {/if}
    </div>
    {#if error}<p class="err tiny">{error}</p>{/if}
    <div class="foot">
      <p class="muted tiny">
        Painted areas are erased to full transparency and saved as a new take (the
        original stays in the filmstrip). Esc closes.
      </p>
      <div class="foot-btns">
        <button class="ghost" onclick={onclose} disabled={applying}>Close</button>
        <button class="gold" onclick={apply} disabled={applying || !maskUrl}>
          {applying ? "Applying…" : "Erase painted areas"}
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    display: grid;
    place-items: center;
    z-index: var(--z-modal);
  }
  .foot-btns {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex: none;
  }
  .sheet {
    width: min(1100px, 94vw);
    height: min(82vh, 900px);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.9rem 1.1rem;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .canvas-holder {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: hidden; /* the canvas must never spill over the modal's own UI */
    border: 1px solid var(--line);
    background:
      repeating-conic-gradient(var(--checker-a) 0% 25%, var(--checker-b) 0% 50%) 0 0 / 20px 20px;
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
  .trail-btn:hover {
    color: var(--bone);
    background: var(--wash);
  }
  .trail-btn.on {
    color: var(--gold);
    border-color: var(--gold-deep);
    background: var(--gold-glow);
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
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .err {
    color: var(--oxblood);
    margin: 0;
  }
</style>
