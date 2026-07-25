<script lang="ts">
  // Contact sheet — every symbol composited at TRUE cell size into the actual reel
  // grid, over a flat dark field. The only honest way to judge relative scale;
  // labels and safe-box outlines are UI overlays, never baked into the image.
  import { commands, type ContactSheet, type GameConfig } from "$lib/ipc";

  let {
    gameId,
    config,
    onclose,
  }: { gameId: string; config: GameConfig; onclose: () => void } = $props();

  let sheet = $state<ContactSheet | null>(null);
  let error = $state("");
  let showLabels = $state(true);
  let showBoxes = $state(false);

  const safeW = $derived((config.symbolSizing?.safeW ?? 0.92) * 100);
  const safeH = $derived((config.symbolSizing?.safeH ?? 0.88) * 100);

  $effect(() => {
    commands.symbolContactSheet(gameId).then((r) => {
      if (r.status === "ok") sheet = r.data;
      else error = r.error;
    });
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onkeydown} />

<div class="backdrop" onclick={onclose} role="presentation">
  <!-- svelte-ignore a11y_interactive_supports_focus, a11y_click_events_have_key_events -->
  <div class="modal card" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Contact sheet">
    <header>
      <h3>Contact sheet</h3>
      <div class="toggles">
        <label><input type="checkbox" bind:checked={showLabels} /> Labels</label>
        <label><input type="checkbox" bind:checked={showBoxes} /> Safe boxes</label>
      </div>
      <button class="close" onclick={onclose} title="Close">✕</button>
    </header>

    {#if error}
      <p class="error">{error}</p>
    {:else if !sheet}
      <p class="muted">Compositing…</p>
    {:else}
      <div class="sheet-scroll">
        <div class="sheet">
          <img src={sheet.png} alt="Symbol contact sheet" />
          {#each sheet.cells as c (c.key)}
            {@const l = (c.x / (sheet.cols * sheet.cellW)) * 100}
            {@const t = (c.y / (sheet.rows * sheet.cellH)) * 100}
            {@const w = (sheet.cellW / (sheet.cols * sheet.cellW)) * 100}
            {@const h = (sheet.cellH / (sheet.rows * sheet.cellH)) * 100}
            {#if showBoxes}
              <div
                class="safe-box"
                style="left:{l + (w * (100 - safeW)) / 200}%; top:{t + (h * (100 - safeH)) / 200}%; width:{(w * safeW) / 100}%; height:{(h * safeH) / 100}%"
              ></div>
            {/if}
            {#if showLabels}
              <div class="label" class:warn={c.flag === "underweight" || c.flag === "overweight"} class:missing={c.flag === "missing"} style="left:{l}%; top:{t}%; width:{w}%">
                <span class="key">{c.key}</span>
                {#if c.flag === "missing"}
                  <span class="ink">no image</span>
                {:else if (c.inkPct ?? 0) > 0}
                  <span class="ink">{((c.inkPct ?? 0) * 100).toFixed(1)}%{c.flag && c.flag !== "ok" ? ` · ${c.flag}` : ""}</span>
                {/if}
              </div>
            {/if}
          {/each}
        </div>
      </div>
      <p class="muted foot">
        True cell size {sheet.cellW}×{sheet.cellH} · {sheet.cols} cols — judge weight here, not on the 1024² previews.
      </p>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--z-modal);
    background: color-mix(in srgb, var(--void) 70%, transparent);
    display: grid;
    place-items: center;
    padding: var(--space-5);
  }
  .modal {
    width: min(1100px, 94vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
    flex: 1;
  }
  .toggles {
    display: flex;
    gap: var(--space-4);
  }
  .toggles label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: 0.78rem;
    color: var(--bone-dim);
    cursor: pointer;
  }
  .close {
    background: none;
    border: none;
    color: var(--bone-dim);
    cursor: pointer;
    font-size: 0.9rem;
    padding: var(--space-1);
  }
  .close:hover {
    color: var(--bone);
  }
  .sheet-scroll {
    overflow: auto;
    min-height: 0;
  }
  .sheet {
    position: relative;
    line-height: 0;
  }
  .sheet img {
    width: 100%;
    height: auto;
    display: block;
    border-radius: var(--radius-1, 4px);
  }
  .safe-box {
    position: absolute;
    border: 1px dashed color-mix(in srgb, var(--amber) 55%, transparent);
    pointer-events: none;
  }
  .label {
    position: absolute;
    padding: 2px 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    pointer-events: none;
    line-height: 1.2;
  }
  .label .key {
    font-family: var(--font-mono);
    font-size: 0.66rem;
    color: var(--bone);
    text-shadow: 0 1px 2px rgb(0 0 0 / 0.8);
  }
  .label .ink {
    font-family: var(--font-mono);
    font-size: 0.62rem;
    color: var(--bone-dim);
    text-shadow: 0 1px 2px rgb(0 0 0 / 0.8);
  }
  .label.warn .ink {
    color: var(--amber);
  }
  .label.missing .ink {
    color: var(--red, #e5484d);
  }
  .foot {
    margin: 0;
    font-size: 0.72rem;
  }
  .error {
    color: var(--red, #e5484d);
    font-size: 0.8rem;
  }
  .muted {
    color: var(--bone-dim);
    font-size: 0.8rem;
  }
</style>
