<script lang="ts">
  /** Bottom action bar that appears when assets are multi-selected in the ledger. */
  import type { ProviderInfo } from "$lib/ipc";

  let {
    count,
    busy,
    progress = "",
    symbolCount = 0,
    providers = [],
    providerId = $bindable(""),
    onproviderchange,
    ongenerate,
    ongenerateset,
    onprocess,
    onpublish,
    onclear,
  }: {
    count: number;
    busy: boolean;
    progress?: string;
    /** How many of the selected assets are symbols (≥2 enables set generation). */
    symbolCount?: number;
    /** Providers for batch generation (select shown when non-empty). */
    providers?: ProviderInfo[];
    providerId?: string;
    onproviderchange?: () => void;
    ongenerate: () => void;
    /** Generate the selected symbols as ONE consistent sheet, then split + upscale. */
    ongenerateset: () => void;
    onprocess: () => void;
    onpublish: () => void;
    onclear: () => void;
  } = $props();
</script>

<div class="batchbar rise">
  <span class="count mono">{count} selected</span>
  {#if progress}<span class="prog">{progress}</span>{/if}
  <span class="spacer"></span>
  {#if providers.length}
    <select
      class="provider"
      bind:value={providerId}
      onchange={onproviderchange}
      disabled={busy}
      title="provider used by Generate / Generate as set"
    >
      {#each providers as p (p.id)}
        <option value={p.id} disabled={!p.configured}>
          {p.displayName}{p.configured ? "" : " — not set up"}
        </option>
      {/each}
    </select>
  {/if}
  <button class="gold" onclick={ongenerate} disabled={busy}>Generate</button>
  {#if symbolCount >= 2}
    <button
      onclick={ongenerateset}
      disabled={busy}
      title="draw the {symbolCount} selected symbols together in ONE image for a perfectly consistent style, then auto-split and upscale each into its own symbol"
    >
      Generate as set
    </button>
  {/if}
  <button onclick={onprocess} disabled={busy}>Process</button>
  <button onclick={onpublish} disabled={busy}>Publish</button>
  <button class="ghost" onclick={onclear} disabled={busy}>Clear</button>
</div>

<style>
  .batchbar {
    /* Centered via auto margins, NOT translateX — the .rise animation fills to
       `transform: none`, which overrides transform-based centering and parks the
       bar's left edge at mid-window (off-screen overflow). */
    position: absolute;
    left: 0;
    right: 0;
    margin-inline: auto;
    width: fit-content;
    bottom: 1rem;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 0.9rem;
    background: var(--ink);
    border: 1px solid var(--gold-deep);
    border-radius: var(--radius-lg);
    box-shadow: 0 8px 30px rgba(0, 0, 0, 0.5);
    z-index: var(--z-bar);
    max-width: min(92%, 860px);
  }
  .count {
    font-size: 0.72rem;
    color: var(--gold-bright);
    white-space: nowrap;
  }
  .prog {
    font-size: 0.7rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
    min-width: 0.5rem;
  }
  .provider {
    max-width: 10rem;
    font-size: 0.72rem;
    padding: 0.3rem 0.45rem;
  }
</style>
