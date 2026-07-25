<script lang="ts">
  /**
   * The game's style anchor: a generated style-calibration plate (NOT a game asset) that
   * every generation automatically attaches as its first reference. Manual per-asset
   * style references stack on top of it, capped at 4 total.
   */
  import { commands, unwrap, type ProviderInfo } from "$lib/ipc";
  import { runJob } from "$lib/stores/jobs.svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  let { gameId, onclose }: { gameId: string; onclose: (changed: boolean) => void } = $props();

  let url = $state<string | null>(null);
  let providers = $state<ProviderInfo[]>([]);
  let providerId = $state("");
  let busy = $state(false);
  let error = $state("");
  let changed = $state(false);
  /** The auto-assembled prompt (style master + calibration plate), for edit/reset. */
  let autoPrompt = $state("");
  let prompt = $state("");
  const promptEdited = $derived(prompt.trim() !== "" && prompt.trim() !== autoPrompt.trim());

  $effect(() => {
    (async () => {
      url = await unwrap(commands.getStyleAnchor(gameId)).catch(() => null);
      autoPrompt = await unwrap(commands.styleAnchorPrompt(gameId)).catch(() => "");
      if (!prompt) prompt = autoPrompt;
      const list = await unwrap(commands.listImageProviders()).catch(() => []);
      providers = list.filter((p) => p.configured && p.id !== "drawthings");
      if (!providerId && providers.length) {
        // Follow the bench's remembered provider when it's usable here.
        const saved = localStorage.getItem("wf-provider");
        providerId = providers.find((p) => p.id === saved)?.id ?? providers[0].id;
      }
    })();
  });

  async function generate() {
    if (!providerId) return;
    busy = true;
    error = "";
    // Send the override only when it differs from auto (empty = auto on the backend).
    const override = promptEdited ? prompt.trim() : "";
    const pid = providerId;
    // Background job: closing the modal doesn't cancel; the bench chip updates on finish.
    const res = await runJob({
      gameId,
      kind: "anchor",
      label: "Style anchor",
      exec: async () => {
        url = await unwrap(commands.generateStyleAnchor(gameId, pid, override));
        changed = true;
        return null;
      },
    });
    if (res.error) error = res.error;
    busy = false;
  }

  async function importFile() {
    const sel = await open({
      multiple: false,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    const path = Array.isArray(sel) ? sel[0] : sel;
    if (!path) return;
    busy = true;
    error = "";
    try {
      url = await unwrap(commands.importStyleAnchor(gameId, path));
      changed = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    busy = true;
    error = "";
    try {
      await unwrap(commands.removeStyleAnchor(gameId));
      url = null;
      changed = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && onclose(changed)}
  onkeydown={(e) => e.key === "Escape" && onclose(changed)}
>
  <div class="sheet card rise" role="dialog" aria-modal="true" aria-label="Style anchor">
    <div class="head">
      <div>
        <span class="u-label">Style anchor</span>
        <h2 class="display title">One image that locks the look</h2>
      </div>
      <button class="close ghost" onclick={() => onclose(changed)} aria-label="Close">✕</button>
    </div>
    <hr class="hairline" />

    <p class="note">
      A style calibration plate generated from your Blueprint's style master — not a game
      asset, just pure style: medium, linework, palette, lighting. When it exists, <strong
      >every generation attaches it automatically</strong> as the first reference (cloud
      providers only). Per-asset style references stack on top, up to 4 total.
    </p>

    <div class="plate" class:empty={!url}>
      {#if url}
        <img src={url} alt="style anchor" />
      {:else}
        <span class="muted small">No anchor yet — generations rely on the style prompt alone.</span>
      {/if}
    </div>

    <label class="field">
      <span class="muted tiny">
        prompt {promptEdited ? "(custom)" : "(auto — from the style master)"}
        {#if promptEdited}
          · <button class="link" onclick={() => (prompt = autoPrompt)}>reset to auto</button>
        {/if}
      </span>
      <textarea rows="5" bind:value={prompt} spellcheck="false"></textarea>
    </label>

    {#if error}<p class="err small">{error}</p>{/if}

    <div class="row">
      {#if providers.length}
        <select bind:value={providerId} disabled={busy}>
          {#each providers as p (p.id)}
            <option value={p.id}>{p.displayName}</option>
          {/each}
        </select>
        <button class="gold" onclick={generate} disabled={busy}>
          {busy ? "Working…" : url ? "Reroll anchor" : "Generate anchor"}
        </button>
      {:else}
        <p class="muted small">No cloud provider configured — add a key in Settings.</p>
      {/if}
      <span class="spacer"></span>
      <button class="ghost" onclick={importFile} disabled={busy}>Import…</button>
      {#if url}
        <button class="ghost danger" onclick={remove} disabled={busy}>Remove</button>
      {/if}
    </div>
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
  }
  .sheet {
    width: min(560px, 94vw);
    max-height: 90vh;
    overflow: auto;
    padding: 1.5rem 1.75rem 1.5rem;
    background: var(--ink);
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
  }
  .title {
    font-size: 1.3rem;
    margin-top: 0.25rem;
  }
  .close {
    padding: 0.3rem 0.5rem;
  }
  .note {
    font-size: 0.8rem;
    color: var(--ash);
    line-height: 1.5;
    margin: 0;
  }
  .plate {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    aspect-ratio: 1;
    max-height: 340px;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    align-self: center;
    width: min(340px, 100%);
  }
  .plate img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .spacer {
    flex: 1;
  }
  .err {
    color: var(--oxblood);
    margin: 0;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .field textarea {
    font-size: 0.74rem;
    line-height: 1.5;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--lapis);
    font-size: inherit;
    cursor: pointer;
  }
</style>
