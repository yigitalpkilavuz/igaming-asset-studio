<script lang="ts">
  /**
   * Audio bench — the authoring surface for a music/SFX cue (the audio analogue of Bench):
   * pick a gambling-safe provider, generate takes, audition them in a player, normalize + encode
   * the shippable twins, and set the active one. Prompt edits are saved back to the cue.
   */
  import {
    commands,
    unwrap,
    type AssetDescriptor,
    type AssetRecord,
    type AudioProviderInfo,
    type GameConfig,
    type Variation,
  } from "$lib/ipc";

  let {
    gameId,
    asset,
    config,
    onsaved,
  }: {
    gameId: string;
    asset: AssetDescriptor;
    config: GameConfig;
    onsaved?: (record: AssetRecord) => void;
  } = $props();

  const cue = $derived(config.audio?.cues?.find((c) => c.key === asset.key) ?? null);

  let record = $state<AssetRecord | null>(null);
  let providers = $state<AudioProviderInfo[]>([]);
  let providerId = $state("");
  let count = $state(1);
  let busy = $state(false);
  let msg = $state("");
  let prompt = $state("");
  // varId → playable data URL (the normalized wav if processed, else the raw take).
  let urls = $state<Record<string, string>>({});

  const active = $derived(record?.activeVariation ?? null);
  const isProcessed = (v: Variation) => v.stages?.some((s) => s.name === "wav") ?? false;

  async function loadUrl(v: Variation) {
    const which = isProcessed(v) ? "wav" : "raw";
    try {
      urls[v.id] = await unwrap(commands.getVariationAudio(gameId, asset.key, v.id, which));
    } catch {
      /* leave unset — the row just won't have a player */
    }
  }

  async function refresh(rec?: AssetRecord) {
    if (!rec) {
      const recs = await unwrap(commands.listAssetRecords(gameId));
      rec = recs.find((r) => r.key === asset.key) ?? null ?? undefined;
    }
    record = rec ?? null;
    urls = {};
    for (const v of record?.variations ?? []) void loadUrl(v);
  }

  // (Re)load when the selected asset changes.
  $effect(() => {
    void asset.key;
    prompt = cue?.description ?? "";
    void refresh();
  });

  $effect(() => {
    void gameId;
    commands.listAudioProviders().then((r) => {
      if (r.status !== "ok") return;
      providers = r.data;
      const pref = config.audio?.defaultProvider || "";
      providerId =
        (pref && providers.some((p) => p.id === pref && p.configured) && pref) ||
        providers.find((p) => p.configured)?.id ||
        providers[0]?.id ||
        "";
    });
  });

  // Whether the chosen provider can make this cue's kind.
  const providerOk = $derived.by(() => {
    const p = providers.find((x) => x.id === providerId);
    if (!p) return false;
    return asset.kind === "music" ? p.doesMusic : p.doesSfx;
  });

  async function persistPromptIfChanged() {
    if (!cue || prompt.trim() === (cue.description ?? "").trim()) return;
    const next: GameConfig = {
      ...config,
      audio: {
        ...(config.audio ?? { cues: [], defaultProvider: "", stylePrompt: "" }),
        cues: (config.audio?.cues ?? []).map((c) =>
          c.key === asset.key ? { ...c, description: prompt.trim() } : c,
        ),
      },
    };
    await unwrap(commands.saveProjectConfig(next));
  }

  async function generate() {
    if (busy || !providerId) return;
    busy = true;
    msg = "Generating…";
    try {
      await persistPromptIfChanged();
      const rec = await unwrap(
        commands.generateAudioVariation(gameId, asset.key, providerId, Math.min(4, Math.max(1, count))),
      );
      await refresh(rec);
      onsaved?.(rec);
      msg = `Generated — audition below, then Finish to normalize.`;
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function finish(v: Variation) {
    busy = true;
    msg = "Normalizing + encoding (ffmpeg)…";
    try {
      const rec = await unwrap(commands.processAudioVariation(gameId, asset.key, v.id));
      await refresh(rec);
      onsaved?.(rec);
      msg = "Normalized to −16 LUFS · baked into the game audiosprite at export.";
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function use(v: Variation) {
    try {
      const rec = await unwrap(commands.setActiveVariation(gameId, asset.key, v.id));
      await refresh(rec);
      onsaved?.(rec);
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    }
  }

  const variations = $derived([...(record?.variations ?? [])].reverse());
</script>

<div class="audio-bench">
  <header class="head">
    <div class="title">
      <span class="kind" class:music={asset.kind === "music"}>{asset.kind === "music" ? "♪ Music" : "◈ SFX"}</span>
      <h2>{cue?.name || asset.key}</h2>
      {#if cue?.looped}<span class="tag">loop</span>{/if}
      {#if cue}<span class="tag mono">{cue.targetSecs ?? 3}s · gain {cue.gain ?? 1}</span>{/if}
    </div>
  </header>

  <section class="compose">
    <label class="lbl" for="ab-prompt">Prompt</label>
    <textarea
      id="ab-prompt"
      class="prompt"
      rows="2"
      bind:value={prompt}
      placeholder="describe the sound — “a short bright coin chime”"
    ></textarea>
    {#if config.audio?.stylePrompt}
      <p class="hint">Prepended with the game's audio style master: <span class="mono">{config.audio.stylePrompt}</span></p>
    {/if}

    <div class="controls">
      <select class="dd mono" bind:value={providerId} title="audio provider">
        {#each providers as p (p.id)}
          <option value={p.id} disabled={!p.configured}>
            {p.displayName}{p.configured ? "" : " (add key in Settings)"}
          </option>
        {/each}
        {#if !providers.length}<option value="">no audio providers</option>{/if}
      </select>
      <label class="count">×<input type="number" min="1" max="4" bind:value={count} class="mono" /></label>
      <button class="gold" onclick={generate} disabled={busy || !providerId || !providerOk}>
        {busy ? "…" : "Generate"}
      </button>
      {#if !providerOk && providerId}
        <span class="warn tiny">this provider can't make {asset.kind === "music" ? "music" : "SFX"}</span>
      {/if}
    </div>
    {#if msg}<p class="msg tiny" class:err={msg.includes("error") || msg.includes("failed")}>{msg}</p>{/if}
  </section>

  <section class="takes">
    {#if !variations.length}
      <p class="hint empty">No takes yet — generate one to audition it.</p>
    {/if}
    {#each variations as v (v.id)}
      <div class="take" class:active={v.id === active}>
        <div class="take-head">
          <span class="mono vid">{v.id}</span>
          <span class="prov tiny">{v.provider}</span>
          {#if v.id === active}<span class="tag ok">active</span>{/if}
          {#if isProcessed(v)}<span class="tag done">normalized</span>{/if}
        </div>
        {#if urls[v.id]}
          <audio class="player" controls src={urls[v.id]} loop={cue?.looped ?? false}></audio>
        {:else}
          <p class="hint tiny">loading audio…</p>
        {/if}
        <div class="take-actions">
          {#if v.audioReport}
            <span class="report tiny mono">
              {(v.audioReport.durationSecs ?? 0).toFixed(1)}s · {(v.audioReport.lufs ?? 0).toFixed(1)} LUFS · {(v.audioReport.peakDbtp ?? 0).toFixed(1)} dBTP
            </span>
          {/if}
          <span class="spacer"></span>
          {#if !isProcessed(v)}
            <button class="ghost sm" onclick={() => finish(v)} disabled={busy}>Finish (normalize)</button>
          {/if}
          {#if v.id !== active}
            <button class="ghost sm" onclick={() => use(v)} disabled={busy}>Use</button>
          {/if}
        </div>
      </div>
    {/each}
  </section>
</div>

<style>
  .audio-bench {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-5);
    overflow: auto;
    min-height: 0;
  }
  .head .title {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
  }
  .head h2 {
    margin: 0;
    font-size: 1rem;
    color: var(--bone);
  }
  .kind {
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--lapis);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    padding: 1px 7px;
  }
  .kind.music {
    color: var(--gold);
  }
  .tag {
    font-size: var(--text-xs);
    color: var(--ash);
    border: 1px solid var(--line-2);
    border-radius: 999px;
    padding: 0 8px;
  }
  .tag.ok {
    color: var(--gold);
    border-color: var(--gold-deep);
  }
  .tag.done {
    color: var(--sage);
    border-color: var(--sage);
  }
  .lbl {
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ash);
    font-weight: 600;
  }
  .prompt {
    width: 100%;
    font-size: 0.82rem;
    font-family: inherit;
    resize: vertical;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.5rem 0.6rem;
    margin-top: 0.3rem;
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--ash);
    margin: 0.35rem 0 0;
  }
  .hint.empty {
    padding: var(--space-4);
    text-align: center;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-top: var(--space-3);
    flex-wrap: wrap;
  }
  .dd {
    font-size: 0.78rem;
    color: var(--bone);
    background: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    padding: 0.34rem 0.5rem;
  }
  .count {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    color: var(--ash);
    font-size: 0.8rem;
  }
  .count input {
    width: 3rem;
    font-size: 0.78rem;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.25rem 0.35rem;
  }
  .warn {
    color: var(--oxblood);
  }
  .msg {
    color: var(--bone-dim);
    margin: 0;
  }
  .msg.err {
    color: var(--oxblood);
  }
  .takes {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .take {
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
    background: var(--ink-2);
  }
  .take.active {
    border-color: var(--gold-deep);
    box-shadow: inset 0 0 0 1px var(--gold-deep);
  }
  .take-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-bottom: var(--space-2);
  }
  .prov {
    color: var(--ash);
  }
  .player {
    width: 100%;
    height: 34px;
  }
  .take-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .report {
    color: var(--sage);
  }
  .sm {
    font-size: 0.72rem;
    padding: 0.22rem 0.6rem;
  }
  .tiny {
    font-size: var(--text-xs);
  }
</style>
