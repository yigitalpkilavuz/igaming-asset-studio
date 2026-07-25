<script lang="ts">
  import { commands, unwrap, type ExportReport } from "$lib/ipc";
  import { openPath } from "@tauri-apps/plugin-opener";
  import { copyText } from "$lib/copy";

  let { gameId, onclose }: { gameId: string; onclose: () => void } = $props();

  let report = $state<ExportReport | null>(null);
  let error = $state("");
  let loading = $state(true);
  let copied = $state(false);

  async function run() {
    loading = true;
    error = "";
    try {
      report = await unwrap(commands.exportDist(gameId));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    run();
  });

  async function copyManifest() {
    if (!report) return;
    copied = await copyText(report.manifestSnippet);
    setTimeout(() => (copied = false), 1500);
  }
  async function openFolder() {
    if (!report) return;
    try {
      await openPath(report.distPath);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && onclose()}
  onkeydown={(e) => e.key === "Escape" && onclose()}
>
  <div class="sheet card rise" role="dialog" aria-modal="true" aria-label="Export">
    <div class="head">
      <div>
        <span class="u-label">Delivery</span>
        <h2 class="display title">Export to Stake format</h2>
      </div>
      <button class="close ghost" onclick={onclose} aria-label="Close">✕</button>
    </div>
    <hr class="hairline" />

    {#if loading}
      <p class="muted pad">Assembling the dist tree…</p>
    {:else if error}
      <p class="warn pad">{error}</p>
      <div class="foot"><button onclick={run}>Try again</button></div>
    {:else if report}
      <div class="tallies">
        <div class="tally">
          <span class="num display">{report.written.length}</span>
          <span class="u-label">exported</span>
        </div>
        <div class="tally">
          <span class="num display dim">{report.missing.length}</span>
          <span class="u-label">not made yet</span>
        </div>
        <div class="tally">
          <span class="num display dim">{report.waived.length}</span>
          <span class="u-label">in code</span>
        </div>
      </div>
      <p class="lead">
        {report.written.length} ready {report.written.length === 1 ? "asset is" : "assets are"} written to dist — drop them into the game now. Assets you haven't made yet are simply skipped; re-export anytime to add more.
      </p>

      <div class="pathrow">
        <span class="path mono">{report.distPath}</span>
        <button class="ghost" onclick={openFolder}>Open folder</button>
      </div>

      {#if report.missing.length}
        <details class="missing">
          <summary class="u-label">{report.missing.length} not exported (not generated/processed yet)</summary>
          <div class="chips">
            {#each report.missing as k (k)}<span class="chip mono">{k}</span>{/each}
          </div>
        </details>
      {/if}

      <div class="mhead">
        <span class="u-label">assets.ts snippet</span>
        <button class="ghost" onclick={copyManifest}>{copied ? "Copied ✓" : "Copy"}</button>
      </div>
      <pre class="manifest"><code>{report.manifestSnippet}</code></pre>

      <p class="hand mono">cp -r {report.distPath}/* web-sdk/apps/{gameId}/static/assets/</p>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: var(--scrim);
    backdrop-filter: blur(3px);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3vh 2rem;
    animation: fade 0.25s var(--ease) both;
  }
  .sheet {
    width: min(760px, 96vw);
    max-height: 92vh;
    overflow: auto;
    padding: 1.5rem 1.75rem 1.75rem;
    animation: sheet-in 0.4s var(--ease-out) both;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 1rem;
  }
  .title {
    font-size: 1.7rem;
    margin-top: 0.25rem;
  }
  .close {
    padding: 0.3rem 0.5rem;
  }
  .pad {
    padding: 2rem 0;
  }
  .tallies {
    display: flex;
    gap: 2.5rem;
    padding: 1.25rem 0 1rem;
  }
  .tally {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .num {
    font-size: 2.2rem;
    line-height: 1;
    color: var(--gold);
  }
  .num.dim {
    color: var(--ash);
  }
  .lead {
    font-size: 0.86rem;
    line-height: 1.55;
    color: var(--bone-dim);
    margin: 0 0 1rem;
  }
  .pathrow {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 0 0 1.25rem;
  }
  .path {
    flex: 1;
    font-size: 0.72rem;
    color: var(--ash-deep);
    word-break: break-all;
  }
  .pathrow button {
    flex: none;
  }
  .missing {
    margin-bottom: 1.25rem;
  }
  summary {
    cursor: pointer;
    color: var(--bone-dim);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    margin-top: 0.7rem;
  }
  .chip {
    font-size: 0.62rem;
    padding: 0.12rem 0.5rem;
    border: 1px solid var(--line-2);
    border-radius: 999px;
    color: var(--ash);
  }
  .mhead {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.55rem;
  }
  .manifest {
    background: var(--ink);
    border: 1px solid var(--line-2);
    border-radius: var(--radius);
    padding: 0.85rem 1rem;
    overflow: auto;
    max-height: 32vh;
    font-family: var(--font-mono);
    font-size: 0.74rem;
    line-height: 1.6;
    color: var(--bone-dim);
    margin: 0 0 1rem;
  }
  .hand {
    font-size: 0.72rem;
    color: var(--gold-deep);
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.6rem 0.8rem;
    word-break: break-all;
    margin: 0;
  }
  .warn {
    color: var(--oxblood);
  }
</style>
