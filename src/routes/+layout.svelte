<script lang="ts">
  import "../app.css";
  import { Settings } from "@lucide/svelte";
  import { appState, goSection, openSettings, type Section } from "$lib/stores/app.svelte";
  import SettingsModal from "$lib/components/SettingsModal.svelte";
  let { children } = $props();

  const SECTIONS: { key: Section; label: string }[] = [
    { key: "library", label: "Library" },
    { key: "produce", label: "Produce" },
    { key: "animate", label: "Animate" },
    { key: "sound", label: "Sound" },
    { key: "fonts", label: "Fonts" },
    { key: "preview", label: "Preview" },
  ];
  /** Produce/Animate need an open game (Produce also allows the new-game blueprint). */
  const enabled = $derived((s: Section) =>
    s === "library" || !!appState.gameId || (s === "produce" && appState.newGame),
  );
</script>

<div class="shell">
  <header class="masthead">
    <div class="brand">
      <span class="wordmark">Wishfell</span>
      <span class="sub">Asset&nbsp;Pipeline</span>
    </div>

    <nav class="sections" aria-label="App section">
      {#each SECTIONS as s (s.key)}
        <button
          class="sec-tab"
          class:on={appState.section === s.key}
          disabled={!enabled(s.key)}
          title={enabled(s.key) ? "" : "open a game first"}
          onclick={() => goSection(s.key)}
        >
          {s.label}
        </button>
      {/each}
    </nav>

    <div class="right">
      {#if appState.gameId}
        <span class="ctx mono" title="the open game — sticky across sections">{appState.gameId}</span>
      {/if}
      <button class="ghost settings-btn" onclick={openSettings} title="Settings">
        <Settings size={15} strokeWidth={1.75} />
        <span>Settings</span>
      </button>
    </div>
  </header>

  <div class="stage">
    {@render children()}
  </div>
</div>

{#if appState.settingsOpen}
  <SettingsModal />
{/if}

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    position: relative;
  }

  .masthead {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    padding: 0 var(--space-6);
    height: 46px;
    border-bottom: 1px solid var(--line);
    background: var(--ink);
    -webkit-app-region: drag;
    user-select: none;
    flex: none;
  }
  .brand {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  .wordmark {
    font-family: var(--font-sans);
    font-size: 1rem;
    font-weight: var(--weight-semibold);
    letter-spacing: -0.01em;
    color: var(--bone);
  }
  .sub {
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--ash-deep);
  }

  /* ── Section tabs: the app's primary navigation ── */
  .sections {
    display: flex;
    align-items: stretch;
    gap: var(--space-6);
    height: 100%;
    -webkit-app-region: no-drag;
  }
  .sec-tab {
    display: flex;
    align-items: center;
    background: transparent;
    border: none;
    border-radius: 0;
    border-bottom: 2px solid transparent;
    border-top: 2px solid transparent; /* keeps the label vertically centred */
    padding: 0 var(--space-1);
    color: var(--ash);
    font-size: var(--text-md);
    font-weight: var(--weight-medium);
    transition: color var(--dur-fast) var(--ease);
  }
  .sec-tab:hover:not(:disabled) {
    color: var(--bone-dim);
    background: transparent;
  }
  .sec-tab.on {
    color: var(--bone);
    border-bottom-color: var(--gold);
  }
  .sec-tab:disabled {
    color: var(--ink-5);
    cursor: default;
  }

  .right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-4);
  }
  .ctx {
    font-size: var(--text-xs);
    color: var(--ash-deep);
  }
  .settings-btn {
    -webkit-app-region: no-drag;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }

  .stage {
    flex: 1;
    overflow: hidden;
    position: relative;
  }
</style>
