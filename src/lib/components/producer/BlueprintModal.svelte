<script lang="ts">
  /**
   * The Blueprint modal — a game's whole definition: identity, mechanics, symbols, scenes, and the
   * AI Port (draft with any agent). Force-opened for a new game; reopened for edits. Extracted from
   * the Producer so the shell is self-contained + renderable in isolation.
   */
  import type { GameConfig } from "$lib/ipc";
  import ConfigForm from "$lib/components/ConfigForm.svelte";
  import BlueprintPorter from "$lib/components/BlueprintPorter.svelte";

  let {
    config = $bindable(),
    savedId,
    saving = false,
    assetsCount = 0,
    onsave,
    oncancel,
    onclose,
  }: {
    config: GameConfig;
    savedId: string | null;
    saving?: boolean;
    assetsCount?: number;
    onsave: () => void;
    oncancel: () => void;
    onclose: () => void;
  } = $props();

  type BpTab = "identity" | "mechanics" | "symbols" | "scenes" | "assistant";
  const TABS: { key: BpTab; label: string }[] = [
    { key: "identity", label: "Identity" },
    { key: "mechanics", label: "Mechanics" },
    { key: "symbols", label: "Symbols" },
    { key: "scenes", label: "Scenes" },
  ];
  // A new game defaults to Identity; the AI Port is the primary route for many, so it's pinned right.
  let tab = $state<BpTab>("identity");

  const isNew = $derived(savedId === null);
  const canSave = $derived(!!config.gameId?.trim() && !saving);

  function scrimClick(e: MouseEvent) {
    // Click-away closes only a SAVED game (a new game must be created or cancelled explicitly).
    if (e.target === e.currentTarget && !isNew) onclose();
  }
</script>

<div class="scrim" role="presentation" onclick={scrimClick} onkeydown={(e) => e.key === "Escape" && !isNew && onclose()}>
  <div class="sheet rise" role="dialog" aria-modal="true" aria-label="Blueprint">
    <header class="head">
      <div class="head-id">
        <span class="u-label">Blueprint</span>
        <h2 class="display title">{config.name?.trim() || "New game"}</h2>
      </div>
      <div class="head-right">
        <span class="derived" title="assets this Blueprint derives">
          <b>{assetsCount}</b> assets
        </span>
        {#if !isNew}
          <button class="x ghost" aria-label="Close" onclick={onclose}>✕</button>
        {/if}
      </div>
    </header>

    <nav class="tabs" aria-label="Blueprint section">
      {#each TABS as t (t.key)}
        <button class="tab" class:on={tab === t.key} onclick={() => (tab = t.key)}>{t.label}</button>
      {/each}
      <span class="tab-spacer"></span>
      <button class="tab" class:on={tab === "assistant"} onclick={() => (tab = "assistant")}>AI Port</button>
    </nav>

    <div class="body">
      {#if tab === "assistant"}
        <BlueprintPorter bind:config />
      {:else}
        <ConfigForm bind:config idLocked={!isNew} section={tab} />
      {/if}
    </div>

    <footer class="foot">
      <p class="foot-hint">
        {#if isNew}
          Only the essentials are required — scenes, tuning &amp; audio can wait. Everything's editable later.
        {:else}
          Editing <span class="mono">{savedId}</span> — the game id is locked.
        {/if}
      </p>
      <div class="foot-actions">
        <button class="ghost" onclick={isNew ? oncancel : onclose}>{isNew ? "Cancel" : "Close"}</button>
        <button class="gold" onclick={onsave} disabled={!canSave}>
          {saving ? "Saving…" : isNew ? "Create game" : "Save blueprint"}
        </button>
      </div>
    </footer>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--z-modal);
    background: var(--scrim);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3vh 2rem;
    animation: fade 0.18s var(--ease) both;
  }
  .sheet {
    width: min(940px, 95vw);
    height: min(84vh, 880px);
    display: flex;
    flex-direction: column;
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-3);
    overflow: hidden;
  }

  /* Head */
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-5) var(--space-6) var(--space-4);
  }
  .title {
    font-size: 1.4rem;
    margin: 0.1rem 0 0;
    line-height: 1.1;
  }
  .head-right {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    flex: none;
  }
  .derived {
    font-size: var(--text-xs);
    color: var(--ash);
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0.2rem 0.6rem;
    white-space: nowrap;
  }
  .derived b {
    color: var(--bone);
    font-weight: 600;
  }
  .x {
    padding: 0.25rem 0.5rem;
  }

  /* Tabs */
  .tabs {
    display: flex;
    align-items: stretch;
    gap: var(--space-5);
    padding: 0 var(--space-6);
    border-bottom: 1px solid var(--line);
  }
  .tab {
    border: none;
    background: transparent;
    border-radius: 0;
    border-bottom: 2px solid transparent;
    padding: var(--space-3) 0.15rem var(--space-4);
    color: var(--ash);
    font-size: var(--text-md);
    transition: color var(--dur-fast) var(--ease);
  }
  .tab:hover:not(.on) {
    color: var(--bone-dim);
  }
  .tab.on {
    color: var(--bone);
    border-bottom-color: var(--gold);
  }
  .tab-spacer {
    flex: 1;
  }

  /* Body */
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-6);
    display: flex;
    flex-direction: column;
  }

  /* Foot */
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-4) var(--space-6);
    border-top: 1px solid var(--line);
    background: var(--ink);
  }
  .foot-hint {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--ash-deep);
    line-height: 1.4;
  }
  .foot-actions {
    display: flex;
    gap: var(--space-3);
    flex: none;
  }
</style>
