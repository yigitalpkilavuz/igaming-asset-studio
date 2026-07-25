<script lang="ts">
  import { commands, unwrap, type AppSettings, type ProviderInfo } from "$lib/ipc";
  import { closeSettings, bumpData } from "$lib/stores/app.svelte";
  import { themeState, setTheme } from "$lib/stores/theme.svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  let present = $state(false);
  let key = $state("");
  let geminiPresent = $state(false);
  let geminiKey = $state("");
  let spritecookPresent = $state(false);
  let spritecookKey = $state("");
  let gamelabPresent = $state(false);
  let gamelabKey = $state("");
  let saving = $state(false);
  let msg = $state("");

  let settings = $state<AppSettings | null>(null);
  let providers = $state<ProviderInfo[]>([]);
  let resolvedRoot = $state("");

  async function refresh() {
    present = await commands.openaiKeyPresent();
    geminiPresent = await commands.geminiKeyPresent();
    spritecookPresent = await commands.spritecookKeyPresent();
    gamelabPresent = await commands.gamelabKeyPresent();
    providers = await unwrap(commands.listImageProviders());
    resolvedRoot = await unwrap(commands.projectsRootPath());
  }

  $effect(() => {
    (async () => {
      settings = await unwrap(commands.getSettings());
      await refresh();
    })();
  });

  async function pickFolder() {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string" && settings) {
      settings.projectsRoot = dir;
      await saveSettings();
      bumpData();
    }
  }

  async function useDefaultFolder() {
    if (!settings) return;
    settings.projectsRoot = "";
    await saveSettings();
    bumpData();
  }

  async function saveKey() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setOpenaiKey(key));
      key = "";
      await refresh();
      msg = present ? "Key stored." : "Cleared.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  async function saveGeminiKey() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setGeminiKey(geminiKey));
      geminiKey = "";
      await refresh();
      msg = geminiPresent ? "Key stored." : "Cleared.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  async function saveSpritecookKey() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setSpritecookKey(spritecookKey));
      spritecookKey = "";
      await refresh();
      msg = spritecookPresent ? "Key stored." : "Cleared.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  async function saveGamelabKey() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setGamelabKey(gamelabKey));
      gamelabKey = "";
      await refresh();
      msg = gamelabPresent ? "Key stored." : "Cleared.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  async function saveSettings() {
    if (!settings) return;
    saving = true;
    msg = "";
    try {
      await unwrap(commands.saveSettings($state.snapshot(settings)));
      await refresh();
      msg = "Saved.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  // Provider row → the settings section that configures it.
  const FIX_TARGET: Record<string, string> = {
    openai_image: "set-openai",
    gemini_image: "set-gemini",
    spritecook: "set-spritecook",
    gamelab: "set-gamelab",
    drawthings: "set-drawthings",
  };
  let flashId = $state("");
  function fixProvider(providerId: string) {
    const id = FIX_TARGET[providerId];
    if (!id) return;
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "center" });
    flashId = id;
    setTimeout(() => (flashId = ""), 1600);
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => e.target === e.currentTarget && closeSettings()}
  onkeydown={(e) => e.key === "Escape" && closeSettings()}
>
  <div class="sheet card rise" role="dialog" aria-modal="true" aria-label="Settings">
    <div class="head">
      <div>
        <span class="u-label">Workshop</span>
        <h2 class="display title">Settings</h2>
      </div>
      <button class="close ghost" onclick={closeSettings} aria-label="Close">✕</button>
    </div>
    <hr class="hairline" />

    <section class="sec">
      <div class="sec-head"><span class="u-label">Image providers</span></div>
      <div class="provs">
        {#each providers as p (p.id)}
          <div class="prov">
            <span class="dot" class:on={p.configured}></span>
            <span class="pname">{p.displayName}</span>
            <span class="pmeta mono">{p.nativeAlpha ? "alpha" : "flat"}{p.supportsSeed ? " · seed" : ""}</span>
            {#if p.configured}
              <span class="pstat mono on">ready</span>
            {:else}
              <button class="ghost fixbtn" onclick={() => fixProvider(p.id)}>fix ↓</button>
            {/if}
          </div>
        {/each}
      </div>
    </section>

    <hr class="hairline" />
    <div class="cols">
      <div class="col">
        <span class="col-label u-label">Cloud keys &amp; models</span>

        <section class="sec" id="set-openai" class:flash={flashId === "set-openai"}>
          <div class="sec-head">
            <span class="u-label">OpenAI key</span>
            <span class="stat mono" class:on={present}>{present ? "configured" : "not set"}</span>
          </div>
          <p class="note">Used for the concept assistant &amp; image generation. Stored in the macOS Keychain — never in project files.</p>
          <div class="row">
            <input
              type="password"
              bind:value={key}
              placeholder={present ? "•••• enter a new key to replace" : "sk-…"}
              spellcheck="false"
            />
            <button onclick={saveKey} disabled={saving}>Save</button>
          </div>
          {#if present}
            <button class="ghost danger tiny" onclick={() => { key = ""; saveKey(); }} disabled={saving}>
              Clear key
            </button>
          {/if}
          {#if settings}
            <div class="model-row">
              <span class="mini-label">Image model</span>
              <input class="mono" bind:value={settings.openaiImageModel} placeholder="gpt-image-2" spellcheck="false" />
              <button onclick={saveSettings} disabled={saving}>Save</button>
            </div>
            <p class="note small">Newest: <code>gpt-image-2</code> (2K, agentic). Use <code>gpt-image-1</code> for cheaper/faster.</p>
            <div class="model-row">
              <span class="mini-label">Vision model</span>
              <input class="mono" bind:value={settings.openaiVisionModel} placeholder="gpt-4o" spellcheck="false" />
              <button onclick={saveSettings} disabled={saving}>Save</button>
            </div>
            <p class="note small">Labels regions for AI auto-cut &amp; layer proposals. Try <code>gpt-4.1</code> or <code>o4-mini</code> for tougher art.</p>
          {/if}
        </section>

        <hr class="hairline" />
        <section class="sec" id="set-gemini" class:flash={flashId === "set-gemini"}>
          <div class="sec-head">
            <span class="u-label">Gemini key (Nano Banana)</span>
            <span class="stat mono" class:on={geminiPresent}>{geminiPresent ? "configured" : "not set"}</span>
          </div>
          <p class="note">Google AI Studio key for Nano Banana image generation. Stored in the macOS Keychain.</p>
          <div class="row">
            <input
              type="password"
              bind:value={geminiKey}
              placeholder={geminiPresent ? "•••• enter a new key to replace" : "AIza…"}
              spellcheck="false"
            />
            <button onclick={saveGeminiKey} disabled={saving}>Save</button>
          </div>
          {#if geminiPresent}
            <button class="ghost danger tiny" onclick={() => { geminiKey = ""; saveGeminiKey(); }} disabled={saving}>
              Clear key
            </button>
          {/if}
          {#if settings}
            <div class="model-row">
              <span class="mini-label">Image model</span>
              <input class="mono" bind:value={settings.geminiImageModel} placeholder="gemini-2.5-flash-image" spellcheck="false" />
              <button onclick={saveSettings} disabled={saving}>Save</button>
            </div>
            <p class="note small">Default: <code>gemini-3.1-flash-image-preview</code> (Nano Banana 2). Use <code>gemini-3-pro-image-preview</code> for Nano Banana Pro (higher fidelity, slower).</p>
          {/if}
        </section>

        <hr class="hairline" />
        <section class="sec" id="set-spritecook" class:flash={flashId === "set-spritecook"}>
          <div class="sec-head">
            <span class="u-label">SpriteCook key</span>
            <span class="stat mono" class:on={spritecookPresent}>{spritecookPresent ? "configured" : "not set"}</span>
          </div>
          <p class="note">spritecook.ai key (<code>sc_live_…</code>) for still generation and AI spritesheets. Stored in the macOS Keychain.</p>
          <div class="row">
            <input
              type="password"
              bind:value={spritecookKey}
              placeholder={spritecookPresent ? "•••• enter a new key to replace" : "sc_live_…"}
              spellcheck="false"
            />
            <button onclick={saveSpritecookKey} disabled={saving}>Save</button>
          </div>
          {#if spritecookPresent}
            <button class="ghost danger tiny" onclick={() => { spritecookKey = ""; saveSpritecookKey(); }} disabled={saving}>
              Clear key
            </button>
          {/if}
          {#if settings}
            <div class="model-row">
              <span class="mini-label">Model</span>
              <input class="mono" bind:value={settings.spritecookModel} placeholder="gemini-3.1-flash-image" spellcheck="false" />
              <button onclick={saveSettings} disabled={saving}>Save</button>
            </div>
          {/if}
        </section>

        <hr class="hairline" />
        <section class="sec" id="set-gamelab" class:flash={flashId === "set-gamelab"}>
          <div class="sec-head">
            <span class="u-label">Gamelab Studio key</span>
            <span class="stat mono" class:on={gamelabPresent}>{gamelabPresent ? "configured" : "not set"}</span>
          </div>
          <p class="note">gamelabstudio.co API key (dashboard → MCP Integration) — game-asset generation with a native transparency pipeline. 1 credit per artwork. Stored in the macOS Keychain.</p>
          <div class="row">
            <input
              type="password"
              bind:value={gamelabKey}
              placeholder={gamelabPresent ? "•••• enter a new key to replace" : "API key"}
              spellcheck="false"
            />
            <button onclick={saveGamelabKey} disabled={saving}>Save</button>
          </div>
          {#if gamelabPresent}
            <button class="ghost danger tiny" onclick={() => { gamelabKey = ""; saveGamelabKey(); }} disabled={saving}>
              Clear key
            </button>
          {/if}
        </section>
      </div>

      <div class="col">
        <span class="col-label u-label">Local &amp; folders</span>

        <section class="sec">
          <div class="sec-head"><span class="u-label">Appearance</span></div>
          <div class="seg">
            <button
              class:on={themeState.theme === "dark"}
              onclick={() => setTheme("dark")}
            >
              Dark
            </button>
            <button
              class:on={themeState.theme === "light"}
              onclick={() => setTheme("light")}
            >
              Light
            </button>
          </div>
        </section>

        <hr class="hairline" />
        {#if settings}
          <section class="sec">
            <div class="sec-head"><span class="u-label">Projects folder</span></div>
            <p class="note">Where games are stored. Point it inside your repo or an SSD. Changing it doesn't move existing games.</p>
            <p class="resolved mono">{resolvedRoot}</p>
            <div class="row">
              <input class="mono" bind:value={settings.projectsRoot} placeholder="default (app data dir)" spellcheck="false" />
              <button onclick={pickFolder}>Browse…</button>
            </div>
            <div class="folder-actions">
              <button onclick={() => saveSettings().then(bumpData)} disabled={saving}>Save folder</button>
              <button class="ghost" onclick={useDefaultFolder} disabled={saving}>Use default</button>
            </div>
          </section>

          <hr class="hairline" />
          <section class="sec" id="set-drawthings" class:flash={flashId === "set-drawthings"}>
            <div class="sec-head"><span class="u-label">Draw Things endpoint</span></div>
            <p class="note">Local A1111-compatible server for on-device generation.</p>
            <div class="row">
              <input class="mono" bind:value={settings.drawThingsUrl} placeholder="http://127.0.0.1:7860" spellcheck="false" />
              <button onclick={saveSettings} disabled={saving}>Save</button>
            </div>
          </section>
        {/if}
      </div>
    </div>

    {#if msg}<p class="msg mono">{msg}</p>{/if}
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
    width: min(860px, 94vw);
    max-height: 90vh;
    overflow: auto;
    padding: 1.5rem 1.75rem 1.75rem;
    animation: sheet-in 0.4s var(--ease-out) both;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 2rem;
  }
  @media (max-width: 720px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
  .col {
    min-width: 0;
  }
  .col-label {
    display: block;
    margin-top: 1rem;
    color: var(--gold-deep);
  }
  .sec.flash {
    animation: sec-flash 1.5s var(--ease) both;
    border-radius: var(--radius);
  }
  @keyframes sec-flash {
    0%,
    60% {
      background: rgba(217, 169, 68, 0.08);
      box-shadow: 0 0 0 1px var(--gold-deep);
    }
    100% {
      background: transparent;
      box-shadow: none;
    }
  }
  .fixbtn {
    font-size: 0.62rem;
    padding: 0.1rem 0.5rem;
    color: var(--gold);
  }
  .seg {
    display: flex;
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    overflow: hidden;
    width: fit-content;
  }
  .seg button {
    border: none;
    border-radius: 0;
    background: transparent;
    font-size: 0.74rem;
    padding: 0.35rem 1rem;
    color: var(--ash);
  }
  .seg button.on {
    background: var(--wash);
    color: var(--bone);
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
  .sec {
    padding: 1.15rem 0 0.25rem;
  }
  .sec-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 0.5rem;
  }
  .stat {
    font-size: 0.62rem;
    color: var(--ash);
  }
  .stat.on {
    color: var(--sage);
  }
  .note {
    font-size: 0.82rem;
    color: var(--ash);
    margin: 0 0 0.7rem;
    line-height: 1.5;
  }
  .row {
    display: flex;
    gap: 0.5rem;
  }
  .row input {
    flex: 1;
  }
  .resolved {
    font-size: 0.68rem;
    color: var(--ash-deep);
    word-break: break-all;
    margin: 0 0 0.6rem;
  }
  .folder-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
  .model-row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.7rem;
  }
  .mini-label {
    font-size: 0.7rem;
    color: var(--ash);
  }
  .note.small {
    font-size: 0.72rem;
    margin-top: 0.4rem;
  }
  .note code {
    font-family: var(--font-mono);
    color: var(--gold-deep);
  }
  .tiny {
    margin-top: 0.55rem;
    font-size: 0.6rem;
    padding: 0.35rem 0.6rem;
  }
  .provs {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .prov {
    display: grid;
    grid-template-columns: auto 1fr auto auto;
    gap: 0.6rem;
    align-items: center;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ash-deep);
  }
  .dot.on {
    background: var(--sage);
    box-shadow: 0 0 6px rgba(127, 197, 132, 0.5);
  }
  .pname {
    color: var(--bone);
    font-size: 0.9rem;
  }
  .pmeta {
    font-size: 0.62rem;
    color: var(--ash-deep);
  }
  .pstat {
    font-size: 0.62rem;
    color: var(--ash-deep);
  }
  .pstat.on {
    color: var(--sage);
  }
  .msg {
    margin: 1rem 0 0;
    font-size: 0.72rem;
    color: var(--bone-dim);
  }
</style>
