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
  let stabilityPresent = $state(false);
  let stabilityKey = $state("");
  let vertexPresent = $state(false);
  let vertexToken = $state("");
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
    stabilityPresent = await commands.stabilityKeyPresent();
    vertexPresent = await commands.vertexTokenPresent();
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

  async function saveStabilityKey() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setStabilityKey(stabilityKey));
      stabilityKey = "";
      await refresh();
      msg = stabilityPresent ? "Key stored." : "Cleared.";
    } catch (e) {
      msg = String(e);
    } finally {
      saving = false;
    }
  }

  async function saveVertexToken() {
    saving = true;
    msg = "";
    try {
      await unwrap(commands.setVertexToken(vertexToken));
      vertexToken = "";
      await refresh();
      msg = vertexPresent ? "Token stored." : "Cleared.";
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
    <header class="head">
      <div class="head-title">
        <span class="u-label">Workshop</span>
        <h2 class="display title">Settings</h2>
      </div>
      <div class="head-right">
        {#if msg}<span class="toast" class:err={/error|failed/i.test(msg)}>{msg}</span>{/if}
        <button class="close ghost" onclick={closeSettings} aria-label="Close">✕</button>
      </div>
    </header>

    <!-- At-a-glance provider readiness. -->
    <div class="overview">
      {#each providers as p (p.id)}
        <button
          class="chip"
          class:on={p.configured}
          disabled={p.configured}
          title={p.configured ? "ready" : "not configured — jump to settings"}
          onclick={() => fixProvider(p.id)}
        >
          <span class="cdot" class:on={p.configured}></span>
          {p.displayName}
          {#if !p.configured}<span class="fix">set up ↓</span>{/if}
        </button>
      {/each}
    </div>

    <div class="grid">
      <!-- ── Left: generation providers ─────────────────────────────── -->
      <div class="col">
        <div class="group-label">Image generation</div>

        <section class="pcard" id="set-openai" class:flash={flashId === "set-openai"}>
          <div class="ch">
            <span class="cname">OpenAI</span>
            <span class="pill" class:on={present}>{present ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">Concept assistant &amp; image generation. Stored in the macOS Keychain.</p>
          <div class="keyrow">
            <input type="password" bind:value={key} placeholder={present ? "•••• replace key" : "sk-…"} spellcheck="false" />
            <button class="save" onclick={saveKey} disabled={saving}>Save</button>
            {#if present}<button class="clear" onclick={() => { key = ""; saveKey(); }} disabled={saving}>Clear</button>{/if}
          </div>
          {#if settings}
            <div class="fields">
              <label class="f"><span>Image model</span>
                <input class="mono" bind:value={settings.openaiImageModel} placeholder="gpt-image-2" spellcheck="false" onchange={saveSettings} />
              </label>
              <label class="f"><span>Vision model</span>
                <input class="mono" bind:value={settings.openaiVisionModel} placeholder="gpt-4o" spellcheck="false" onchange={saveSettings} />
              </label>
            </div>
          {/if}
        </section>

        <section class="pcard" id="set-gemini" class:flash={flashId === "set-gemini"}>
          <div class="ch">
            <span class="cname">Gemini <span class="cbadge">Nano Banana</span></span>
            <span class="pill" class:on={geminiPresent}>{geminiPresent ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">Google AI Studio key for Nano Banana image generation.</p>
          <div class="keyrow">
            <input type="password" bind:value={geminiKey} placeholder={geminiPresent ? "•••• replace key" : "AIza…"} spellcheck="false" />
            <button class="save" onclick={saveGeminiKey} disabled={saving}>Save</button>
            {#if geminiPresent}<button class="clear" onclick={() => { geminiKey = ""; saveGeminiKey(); }} disabled={saving}>Clear</button>{/if}
          </div>
          {#if settings}
            <div class="fields">
              <label class="f"><span>Image model</span>
                <input class="mono" bind:value={settings.geminiImageModel} placeholder="gemini-3.1-flash-image-preview" spellcheck="false" onchange={saveSettings} />
              </label>
            </div>
          {/if}
        </section>

        <section class="pcard" id="set-spritecook" class:flash={flashId === "set-spritecook"}>
          <div class="ch">
            <span class="cname">SpriteCook</span>
            <span class="pill" class:on={spritecookPresent}>{spritecookPresent ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">Stills &amp; AI spritesheets (<code>sc_live_…</code>).</p>
          <div class="keyrow">
            <input type="password" bind:value={spritecookKey} placeholder={spritecookPresent ? "•••• replace key" : "sc_live_…"} spellcheck="false" />
            <button class="save" onclick={saveSpritecookKey} disabled={saving}>Save</button>
            {#if spritecookPresent}<button class="clear" onclick={() => { spritecookKey = ""; saveSpritecookKey(); }} disabled={saving}>Clear</button>{/if}
          </div>
          {#if settings}
            <div class="fields">
              <label class="f"><span>Model</span>
                <input class="mono" bind:value={settings.spritecookModel} placeholder="gemini-3.1-flash-image" spellcheck="false" onchange={saveSettings} />
              </label>
            </div>
          {/if}
        </section>

        <section class="pcard" id="set-gamelab" class:flash={flashId === "set-gamelab"}>
          <div class="ch">
            <span class="cname">Gamelab Studio</span>
            <span class="pill" class:on={gamelabPresent}>{gamelabPresent ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">Native-alpha game-asset generation (dashboard → MCP Integration). 1 credit / artwork.</p>
          <div class="keyrow">
            <input type="password" bind:value={gamelabKey} placeholder={gamelabPresent ? "•••• replace key" : "API key"} spellcheck="false" />
            <button class="save" onclick={saveGamelabKey} disabled={saving}>Save</button>
            {#if gamelabPresent}<button class="clear" onclick={() => { gamelabKey = ""; saveGamelabKey(); }} disabled={saving}>Clear</button>{/if}
          </div>
        </section>

        {#if settings}
          <section class="pcard" id="set-drawthings" class:flash={flashId === "set-drawthings"}>
            <div class="ch">
              <span class="cname">Draw Things <span class="cbadge">local</span></span>
            </div>
            <p class="cnote">Local A1111-compatible server for on-device generation — no key needed.</p>
            <div class="keyrow">
              <input class="mono" bind:value={settings.drawThingsUrl} placeholder="http://127.0.0.1:7860" spellcheck="false" onchange={saveSettings} />
              <button class="save" onclick={saveSettings} disabled={saving}>Save</button>
            </div>
          </section>
        {/if}
      </div>

      <!-- ── Right: audio + application ───────────────────────────────── -->
      <div class="col">
        <div class="group-label">Audio <span class="gl-note">gambling-safe providers only</span></div>

        <section class="pcard" id="set-audio" class:flash={flashId === "set-audio"}>
          <div class="ch">
            <span class="cname">Stable Audio <span class="cbadge">music + SFX</span></span>
            <span class="pill" class:on={stabilityPresent}>{stabilityPresent ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">A key from platform.stability.ai.</p>
          <div class="keyrow">
            <input type="password" bind:value={stabilityKey} placeholder={stabilityPresent ? "•••• replace key" : "Stability key — sk-…"} spellcheck="false" />
            <button class="save" onclick={saveStabilityKey} disabled={saving}>Save</button>
            {#if stabilityPresent}<button class="clear" onclick={() => { stabilityKey = ""; saveStabilityKey(); }} disabled={saving}>Clear</button>{/if}
          </div>
          {#if settings}
            <div class="fields">
              <label class="f"><span>Model</span>
                <input class="mono" bind:value={settings.stableAudioModel} placeholder="stable-audio-2" spellcheck="false" onchange={saveSettings} />
              </label>
            </div>
          {/if}

          <div class="ch mt">
            <span class="cname">Google Lyria <span class="cbadge">music only</span></span>
            <span class="pill" class:on={vertexPresent}>{vertexPresent ? "Ready" : "Not set"}</span>
          </div>
          <p class="cnote">Vertex AI access token (<code>gcloud auth print-access-token</code>) + a project.</p>
          <div class="keyrow">
            <input type="password" bind:value={vertexToken} placeholder={vertexPresent ? "•••• replace token" : "Vertex token — ya29.…"} spellcheck="false" />
            <button class="save" onclick={saveVertexToken} disabled={saving}>Save</button>
            {#if vertexPresent}<button class="clear" onclick={() => { vertexToken = ""; saveVertexToken(); }} disabled={saving}>Clear</button>{/if}
          </div>
          {#if settings}
            <div class="fields three">
              <label class="f"><span>Model</span>
                <input class="mono" bind:value={settings.lyriaModel} placeholder="lyria-002" spellcheck="false" onchange={saveSettings} />
              </label>
              <label class="f"><span>Project</span>
                <input class="mono" bind:value={settings.vertexProject} placeholder="gcp-project" spellcheck="false" onchange={saveSettings} />
              </label>
              <label class="f"><span>Region</span>
                <input class="mono" bind:value={settings.vertexLocation} placeholder="us-central1" spellcheck="false" onchange={saveSettings} />
              </label>
            </div>
          {/if}
          <p class="cnote foot">ElevenLabs, MusicGen &amp; Udio are excluded — their licenses bar real-money gambling.</p>
        </section>

        <div class="group-label">Application</div>

        <section class="pcard">
          <div class="ch"><span class="cname">Appearance</span></div>
          <div class="seg">
            <button class:on={themeState.theme === "dark"} onclick={() => setTheme("dark")}>Dark</button>
            <button class:on={themeState.theme === "light"} onclick={() => setTheme("light")}>Light</button>
          </div>
        </section>

        {#if settings}
          <section class="pcard">
            <div class="ch"><span class="cname">Projects folder</span></div>
            <p class="cnote">Where games are stored — point it inside your repo or an SSD. Changing it doesn't move existing games.</p>
            <p class="resolved mono">{resolvedRoot}</p>
            <div class="keyrow">
              <input class="mono" bind:value={settings.projectsRoot} placeholder="default (app data dir)" spellcheck="false" />
              <button class="save" onclick={pickFolder}>Browse…</button>
            </div>
            <div class="folder-actions">
              <button class="save" onclick={() => saveSettings().then(bumpData)} disabled={saving}>Save folder</button>
              <button class="clear" onclick={useDefaultFolder} disabled={saving}>Use default</button>
            </div>
          </section>
        {/if}
      </div>
    </div>
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
    animation: fade 0.2s var(--ease) both;
  }
  .sheet {
    width: min(880px, 94vw);
    max-height: 90vh;
    overflow: auto;
    padding: 1.4rem 1.6rem 1.6rem;
    animation: sheet-in 0.32s var(--ease-out) both;
  }

  /* Header + toast */
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 1rem;
  }
  .title {
    font-size: 1.55rem;
    margin-top: 0.15rem;
  }
  .head-right {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .toast {
    font-size: 0.7rem;
    color: var(--sage);
    background: color-mix(in srgb, var(--sage) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--sage) 35%, transparent);
    border-radius: 999px;
    padding: 0.15rem 0.6rem;
    animation: fade 0.2s var(--ease) both;
  }
  .toast.err {
    color: var(--oxblood);
    background: color-mix(in srgb, var(--oxblood) 12%, transparent);
    border-color: color-mix(in srgb, var(--oxblood) 35%, transparent);
  }
  .close {
    padding: 0.3rem 0.55rem;
  }

  /* Provider readiness overview */
  .overview {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin-bottom: 1.1rem;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.72rem;
    padding: 0.28rem 0.65rem;
    border-radius: 999px;
    border: 1px solid var(--line-2);
    background: var(--ink-3);
    color: var(--bone-dim);
  }
  .chip.on {
    color: var(--bone);
    cursor: default;
  }
  .chip:not(.on):hover {
    border-color: var(--gold-deep);
    color: var(--bone);
  }
  .cdot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: var(--ash-deep);
    flex: none;
  }
  .cdot.on {
    background: var(--sage);
    box-shadow: 0 0 6px color-mix(in srgb, var(--sage) 55%, transparent);
  }
  .fix {
    color: var(--gold);
    font-size: 0.64rem;
  }

  /* Two-column card grid */
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 1.4rem;
    align-items: start;
  }
  @media (max-width: 760px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
  .col {
    min-width: 0;
  }
  .group-label {
    font-size: 0.64rem;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    font-weight: 700;
    color: var(--gold-deep);
    margin: 0.2rem 0 0.55rem;
  }
  .group-label:not(:first-child) {
    margin-top: 1.05rem;
  }
  .gl-note {
    color: var(--ash-deep);
    font-weight: 500;
    letter-spacing: 0.02em;
    text-transform: none;
  }

  /* Provider / settings card */
  .pcard {
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius, 10px);
    padding: 0.85rem 0.95rem;
    margin-bottom: 0.7rem;
  }
  .pcard.flash {
    animation: sec-flash 1.5s var(--ease) both;
  }
  @keyframes sec-flash {
    0%,
    55% {
      border-color: var(--gold-deep);
      box-shadow:
        0 0 0 1px var(--gold-deep),
        0 0 18px color-mix(in srgb, var(--gold) 25%, transparent);
    }
    100% {
      border-color: var(--line);
      box-shadow: none;
    }
  }
  .ch {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .ch.mt {
    margin-top: 0.85rem;
    padding-top: 0.85rem;
    border-top: 1px solid var(--line);
  }
  .cname {
    color: var(--bone);
    font-size: 0.88rem;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }
  .cbadge {
    font-size: 0.56rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ash);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.35rem;
  }
  .pill {
    font-size: 0.56rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-weight: 600;
    color: var(--ash);
    border: 1px solid var(--line-2);
    border-radius: 999px;
    padding: 0.1rem 0.5rem;
    flex: none;
  }
  .pill.on {
    color: var(--sage);
    border-color: color-mix(in srgb, var(--sage) 40%, transparent);
    background: color-mix(in srgb, var(--sage) 12%, transparent);
  }
  .cnote {
    font-size: 0.74rem;
    color: var(--ash);
    line-height: 1.45;
    margin: 0.4rem 0 0;
  }
  .cnote.foot {
    margin-top: 0.6rem;
    font-size: 0.68rem;
    color: var(--ash-deep);
  }
  .cnote code {
    font-family: var(--font-mono);
    color: var(--gold-deep);
    font-size: 0.92em;
  }

  /* Key row */
  .keyrow {
    display: flex;
    gap: 0.4rem;
    margin-top: 0.55rem;
  }
  .keyrow input {
    flex: 1;
    min-width: 0;
  }
  .save,
  .clear {
    white-space: nowrap;
    font-size: 0.76rem;
    padding: 0.4rem 0.75rem;
  }
  .clear {
    background: transparent;
    border: 1px solid var(--line-2);
    color: var(--ash);
  }
  .clear:hover:not(:disabled) {
    color: var(--oxblood);
    border-color: color-mix(in srgb, var(--oxblood) 45%, transparent);
  }

  /* Model fields */
  .fields {
    display: grid;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  .fields.three {
    grid-template-columns: 1fr 1fr 1fr;
  }
  .f {
    display: flex;
    flex-direction: column;
    gap: 0.22rem;
    min-width: 0;
  }
  .f span {
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ash);
  }
  .f input {
    width: 100%;
    min-width: 0;
  }

  /* Scoped, consistent input styling */
  .sheet input {
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.4rem 0.55rem;
    font-size: 0.78rem;
  }
  .sheet input:focus {
    outline: none;
    border-color: var(--gold-deep);
  }
  .sheet input::placeholder {
    color: var(--ash-deep);
  }

  /* Appearance segmented */
  .seg {
    display: flex;
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    overflow: hidden;
    width: fit-content;
    margin-top: 0.5rem;
  }
  .seg button {
    border: none;
    border-radius: 0;
    background: transparent;
    font-size: 0.74rem;
    padding: 0.35rem 1.1rem;
    color: var(--ash);
  }
  .seg button.on {
    background: var(--wash);
    color: var(--bone);
  }

  .resolved {
    font-size: 0.66rem;
    color: var(--ash-deep);
    word-break: break-all;
    margin: 0.5rem 0 0;
  }
  .folder-actions {
    display: flex;
    gap: 0.4rem;
    margin-top: 0.5rem;
  }
</style>
