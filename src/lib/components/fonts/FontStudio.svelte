<script lang="ts">
  /**
   * Font studio — the dedicated Fonts page (masthead section). A master-detail editor for a game's
   * bitmap fonts: add / remove / edit each font (typeface, size, gradient fill, outline) with a
   * LIVE preview rasterized by the backend. Fonts persist to config.fonts and bake to BMFont
   * `.xml` + `.webp` at export — no AI, no per-take generation.
   */
  import { onMount } from "svelte";
  import { commands, unwrap, type FontDef, type FontTypeface, type GameConfig } from "$lib/ipc";
  import { DEFAULT_FONTS, newFont } from "$lib/fonts";

  let { gameId }: { gameId: string } = $props();

  let config = $state<GameConfig | null>(null);
  let typefaces = $state<FontTypeface[]>([]);
  let selectedKey = $state<string | null>(null);
  let loading = $state(true);
  let error = $state("");
  let busy = $state("");
  let sample = $state("WIN $1,234.56");
  let previewUrl = $state("");

  const fonts = $derived(config?.fonts ?? []);
  const hasFonts = $derived(!!config?.hasFonts);
  const selectedFont = $derived(fonts.find((f) => f.key === selectedKey) ?? null);

  function msg(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function reload() {
    const project = await unwrap(commands.getProject(gameId));
    config = project.config;
    typefaces = await commands.listFontTypefaces();
    if (!selectedKey || !fonts.some((f) => f.key === selectedKey)) {
      selectedKey = fonts[0]?.key ?? null;
    }
  }

  onMount(() => {
    void (async () => {
      try {
        await reload();
      } catch (e) {
        error = msg(e);
      } finally {
        loading = false;
      }
    })();
  });

  async function save(next: GameConfig, note = "") {
    busy = note || "Saving…";
    try {
      const project = await unwrap(commands.saveProjectConfig(next));
      config = project.config;
      busy = "";
    } catch (e) {
      error = msg(e);
      busy = "";
    }
  }

  async function enableFonts() {
    if (!config) return;
    await save({ ...config, hasFonts: true, fonts: DEFAULT_FONTS.map((f) => ({ ...f })) }, "Enabling fonts…");
    selectedKey = DEFAULT_FONTS[0].key;
  }

  async function addFont() {
    if (!config) return;
    const f = newFont(fonts);
    await save({ ...config, fonts: [...fonts, f] }, "Adding font…");
    selectedKey = f.key;
  }

  async function removeFont(key: string) {
    if (!config) return;
    await save({ ...config, fonts: fonts.filter((f) => f.key !== key) }, "Removing…");
    if (selectedKey === key) selectedKey = fonts.find((f) => f.key !== key)?.key ?? null;
  }

  function patchFont(key: string, patch: Partial<FontDef>) {
    if (!config) return;
    void save({ ...config, fonts: fonts.map((f) => (f.key === key ? { ...f, ...patch } : f)) });
  }

  // Live preview — re-rasterize whenever the selected font or the sample text changes.
  $effect(() => {
    const f = selectedFont;
    const s = sample;
    if (!f) {
      previewUrl = "";
      return;
    }
    let cancelled = false;
    void unwrap(commands.previewFont(f, s))
      .then((url) => {
        if (!cancelled) previewUrl = url;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  });
</script>

<div class="fonts density-compact">
  <div class="topbar">
    <div class="lead">
      <span class="u-label">Fonts</span>
      <h2 class="title">{config?.name || gameId}</h2>
    </div>
    {#if hasFonts}
      <div class="master">
        <input class="sample" placeholder="preview text" bind:value={sample} />
      </div>
    {/if}
    {#if busy}<span class="busy tiny amber">{busy}</span>{/if}
  </div>

  {#if loading}
    <p class="msg">Loading…</p>
  {:else if error}
    <p class="msg err">{error}</p>
  {:else if !hasFonts}
    <div class="enable">
      <p class="u-label">No fonts yet</p>
      <p class="muted">Bitmap fonts render win amounts, multipliers and labels at runtime. Seed a white + gold pair to start.</p>
      <button class="gold" onclick={enableFonts}>Enable fonts</button>
    </div>
  {:else}
    <div class="work">
      <aside class="rail">
        <div class="grp-head">
          <span class="u-label">Fonts</span>
          <button class="add" title="add a font" onclick={addFont}>＋</button>
        </div>
        {#each fonts as f (f.key)}
          <button class="font" class:on={f.key === selectedKey} onclick={() => (selectedKey = f.key)}>
            <span class="swatch" style="background: linear-gradient(180deg, {f.fillTop}, {f.fillBottom}); outline: 1px solid {f.outlineColor};"></span>
            <span class="nm">{f.name || f.key}</span>
          </button>
        {/each}
        {#if !fonts.length}<p class="none tiny">none — ＋ to add</p>{/if}
      </aside>

      <section class="detail">
        {#if selectedFont}
          {@const f = selectedFont}
          <div class="preview">
            {#if previewUrl}<img class="prev-img" src={previewUrl} alt="font preview" />{/if}
          </div>

          <div class="settings">
            <label class="row wide">
              <span>Name</span>
              <input value={f.name} placeholder="font name" onchange={(e) => patchFont(f.key, { name: e.currentTarget.value })} />
              <span class="mono key">{f.key}</span>
            </label>

            <label class="row">
              <span>Typeface</span>
              <select value={f.typeface} onchange={(e) => patchFont(f.key, { typeface: e.currentTarget.value })}>
                {#each typefaces as t (t.id)}
                  <option value={t.id}>{t.name}</option>
                {/each}
              </select>
            </label>

            <label class="row">
              <span>Size (px)</span>
              <input type="number" min="16" max="256" step="4" value={f.sizePx} onchange={(e) => patchFont(f.key, { sizePx: parseInt(e.currentTarget.value) || 96 })} />
            </label>

            <label class="row">
              <span>Fill top</span>
              <input type="color" value={f.fillTop} onchange={(e) => patchFont(f.key, { fillTop: e.currentTarget.value })} />
              <input class="hex" value={f.fillTop} onchange={(e) => patchFont(f.key, { fillTop: e.currentTarget.value })} />
            </label>
            <label class="row">
              <span>Fill bottom</span>
              <input type="color" value={f.fillBottom} onchange={(e) => patchFont(f.key, { fillBottom: e.currentTarget.value })} />
              <input class="hex" value={f.fillBottom} onchange={(e) => patchFont(f.key, { fillBottom: e.currentTarget.value })} />
            </label>

            <label class="row">
              <span>Outline</span>
              <input type="color" value={f.outlineColor} onchange={(e) => patchFont(f.key, { outlineColor: e.currentTarget.value })} />
              <input type="number" min="0" max="24" step="1" value={f.outlinePx} onchange={(e) => patchFont(f.key, { outlinePx: parseInt(e.currentTarget.value) || 0 })} title="outline width px" />
            </label>

            <div class="row wide end">
              <button class="ghost danger sm" onclick={() => removeFont(f.key)}>Remove font</button>
            </div>
          </div>
        {:else}
          <div class="enable">
            <p class="u-label">No font selected</p>
            <p class="muted">Pick a font on the left, or ＋ to add one.</p>
          </div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .fonts {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--ink-2);
  }
  .topbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
    background: var(--ink);
  }
  .lead {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  .title {
    margin: 0;
    font-size: 0.95rem;
    color: var(--bone);
    white-space: nowrap;
  }
  .master {
    flex: 1;
  }
  .sample {
    width: 100%;
    font-size: 0.78rem;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.35rem 0.5rem;
  }
  .busy {
    color: var(--gold);
    white-space: nowrap;
  }
  .msg {
    padding: var(--space-5);
    color: var(--bone-dim);
  }
  .msg.err {
    color: var(--oxblood);
  }
  .enable {
    margin: auto;
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    align-items: center;
    padding: var(--space-6);
    max-width: 420px;
  }
  .work {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 240px 1fr;
  }
  .rail {
    border-right: 1px solid var(--line);
    overflow: auto;
    padding: var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    background: var(--ink);
  }
  .grp-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-2);
  }
  .add {
    background: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    color: var(--bone-dim);
    width: 22px;
    height: 22px;
    line-height: 1;
    padding: 0;
  }
  .add:hover {
    color: var(--gold);
  }
  .font {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    padding: 0.35rem 0.45rem;
    color: var(--bone-dim);
    font-size: 0.8rem;
  }
  .font:hover {
    background: var(--wash);
  }
  .font.on {
    background: var(--wash);
    color: var(--bone);
    box-shadow: inset 2px 0 0 var(--gold);
  }
  .swatch {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    flex: none;
  }
  .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .none {
    color: var(--ash-deep);
    padding: 0 0.45rem;
  }
  .detail {
    min-width: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .preview {
    flex: none;
    height: 160px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-bottom: 1px solid var(--line);
    padding: var(--space-4);
    overflow: hidden;
    /* Neutral checkerboard so both light and dark fills read clearly. */
    background:
      repeating-conic-gradient(var(--ink-3) 0% 25%, var(--ink-2) 0% 50%) 50% / 22px 22px;
  }
  .prev-img {
    max-width: 100%;
    max-height: 128px;
    object-fit: contain;
    image-rendering: auto;
  }
  .settings {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
    max-width: 560px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .row > span:first-child {
    width: 84px;
    flex: none;
  }
  .row.wide {
    max-width: none;
  }
  .row.end {
    justify-content: flex-end;
  }
  .row input:not([type="color"]):not(.hex),
  .row select {
    font-size: 0.78rem;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.3rem 0.5rem;
  }
  .row input[type="number"] {
    width: 5rem;
  }
  .row.wide input {
    flex: 1;
  }
  .hex {
    width: 6.5rem;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.28rem 0.45rem;
  }
  .row input[type="color"] {
    width: 34px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    background: none;
  }
  .key {
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .sm {
    font-size: 0.72rem;
    padding: 0.22rem 0.6rem;
  }
  .tiny {
    font-size: var(--text-xs);
  }
</style>
