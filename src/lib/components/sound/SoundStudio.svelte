<script lang="ts">
  /**
   * Sound studio — the dedicated audio page (masthead section). A master-detail cue manager for
   * a game's music + SFX: add / remove / edit cues (rename, loop, gain, duration, kind), and
   * generate + audition + normalize each via the embedded AudioBench. Cue edits persist to
   * config.audio.cues; the derived asset list re-derives on every change.
   */
  import { onMount } from "svelte";
  import {
    commands,
    unwrap,
    type AssetDescriptor,
    type AssetRecord,
    type AudioCueDef,
    type AudioKind,
    type GameConfig,
  } from "$lib/ipc";
  import { assetStatus } from "$lib/assetStatus";
  import { DEFAULT_AUDIO_CUES, newCue } from "$lib/audioCues";
  import AudioBench from "../producer/AudioBench.svelte";

  let { gameId }: { gameId: string } = $props();

  let config = $state<GameConfig | null>(null);
  let descriptors = $state<AssetDescriptor[]>([]);
  let records = $state<Record<string, AssetRecord>>({});
  let selectedKey = $state<string | null>(null);
  let loading = $state(true);
  let error = $state("");
  let busy = $state("");

  const cues = $derived(config?.audio?.cues ?? []);
  const hasAudio = $derived(!!config?.hasAudio);
  const musicCues = $derived(cues.filter((c) => c.kind === "music"));
  const sfxCues = $derived(cues.filter((c) => c.kind === "sfx"));
  const selectedCue = $derived(cues.find((c) => c.key === selectedKey) ?? null);
  const selectedDescriptor = $derived(descriptors.find((d) => d.key === selectedKey) ?? null);

  async function reload() {
    const project = await unwrap(commands.getProject(gameId));
    config = project.config;
    descriptors = (await commands.deriveAssets(project.config)).filter(
      (d) => d.kind === "music" || d.kind === "sfx",
    );
    const recs = await unwrap(commands.listAssetRecords(gameId));
    records = Object.fromEntries(recs.map((r) => [r.key, r]));
    if (!selectedKey || !cues.some((c) => c.key === selectedKey)) {
      selectedKey = cues[0]?.key ?? null;
    }
  }

  onMount(() => {
    void (async () => {
      try {
        await reload();
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        loading = false;
      }
    })();
  });

  /** Persist a new config and re-derive. */
  async function save(next: GameConfig, note = "") {
    busy = note || "Saving…";
    try {
      const project = await unwrap(commands.saveProjectConfig(next));
      config = project.config;
      descriptors = (await commands.deriveAssets(project.config)).filter(
        (d) => d.kind === "music" || d.kind === "sfx",
      );
      busy = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      busy = "";
    }
  }

  const audioBlock = (c: GameConfig): NonNullable<GameConfig["audio"]> =>
    c.audio ?? { cues: [], defaultProvider: "", stylePrompt: "" };

  async function enableAudio() {
    if (!config) return;
    await save(
      { ...config, hasAudio: true, audio: { ...audioBlock(config), cues: DEFAULT_AUDIO_CUES.map((c) => ({ ...c })) } },
      "Enabling audio…",
    );
    selectedKey = DEFAULT_AUDIO_CUES[0].key;
  }

  async function addCue(kind: AudioKind) {
    if (!config) return;
    const c = newCue(kind, cues);
    await save({ ...config, audio: { ...audioBlock(config), cues: [...cues, c] } }, "Adding cue…");
    selectedKey = c.key;
  }

  async function removeCue(key: string) {
    if (!config) return;
    await save({ ...config, audio: { ...audioBlock(config), cues: cues.filter((c) => c.key !== key) } }, "Removing…");
    if (selectedKey === key) selectedKey = cues.find((c) => c.key !== key)?.key ?? null;
  }

  function patchCue(key: string, patch: Partial<AudioCueDef>) {
    if (!config) return;
    void save({
      ...config,
      audio: { ...audioBlock(config), cues: cues.map((c) => (c.key === key ? { ...c, ...patch } : c)) },
    });
  }

  function patchAudio(patch: Partial<NonNullable<GameConfig["audio"]>>) {
    if (!config) return;
    void save({ ...config, audio: { ...audioBlock(config), ...patch } });
  }

  // Ready state per cue: green = active + normalized, amber = generated, grey = nothing.
  function dot(key: string): "ready" | "gen" | "empty" {
    const st = assetStatus(records[key]);
    return st.processed ? "ready" : st.generated ? "gen" : "empty";
  }
</script>

<div class="sound density-compact">
  <div class="topbar">
    <div class="lead">
      <span class="u-label">Sound</span>
      <h2 class="title">{config?.name || gameId}</h2>
    </div>
    {#if hasAudio && config?.audio}
      <div class="master">
        <input
          class="style"
          placeholder="audio style master — e.g. dark baroque orchestral, harpsichord, ominous"
          value={config.audio.stylePrompt}
          onchange={(e) => patchAudio({ stylePrompt: e.currentTarget.value })}
        />
      </div>
    {/if}
    {#if busy}<span class="busy tiny amber">{busy}</span>{/if}
  </div>

  {#if loading}
    <p class="msg">Loading…</p>
  {:else if error}
    <p class="msg err">{error}</p>
  {:else if !hasAudio}
    <div class="enable">
      <p class="u-label">No audio yet</p>
      <p class="muted">Turn on audio to seed the core music + SFX cues, then generate them here.</p>
      <button class="gold" onclick={enableAudio}>Enable audio</button>
    </div>
  {:else}
    <div class="work">
      <aside class="rail">
        {#each [{ label: "Music", kind: "music", list: musicCues }, { label: "Sound effects", kind: "sfx", list: sfxCues }] as grp (grp.kind)}
          <div class="grp">
            <div class="grp-head">
              <span class="u-label">{grp.label}</span>
              <button class="add" title="add a cue" onclick={() => addCue(grp.kind as AudioKind)}>＋</button>
            </div>
            {#each grp.list as c (c.key)}
              <button class="cue" class:on={c.key === selectedKey} onclick={() => (selectedKey = c.key)}>
                <span class="d {dot(c.key)}"></span>
                <span class="nm">{c.name || c.key}</span>
                {#if c.looped}<span class="loop" title="loops">∞</span>{/if}
              </button>
            {/each}
            {#if !grp.list.length}<p class="none tiny">none — ＋ to add</p>{/if}
          </div>
        {/each}
      </aside>

      <section class="detail">
        {#if selectedCue}
          {@const c = selectedCue}
          <div class="cue-settings">
            <input
              class="cue-name"
              value={c.name}
              placeholder="cue name"
              onchange={(e) => patchCue(c.key, { name: e.currentTarget.value })}
            />
            <span class="mono key">{c.key}</span>
            <span class="sep"></span>
            <select
              class="mini"
              value={c.kind}
              onchange={(e) => patchCue(c.key, { kind: e.currentTarget.value as AudioKind })}
            >
              <option value="music">music</option>
              <option value="sfx">sfx</option>
            </select>
            <label class="mini-check">
              <input
                type="checkbox"
                checked={c.looped}
                onchange={(e) => patchCue(c.key, { looped: e.currentTarget.checked })}
              /> loop
            </label>
            <label class="mini-num">gain
              <input
                type="number" min="0" max="1" step="0.05"
                value={c.gain}
                onchange={(e) => patchCue(c.key, { gain: parseFloat(e.currentTarget.value) || 1 })}
              />
            </label>
            <label class="mini-num">secs
              <input
                type="number" min="0.1" max="180" step="0.1"
                value={c.targetSecs}
                onchange={(e) => patchCue(c.key, { targetSecs: parseFloat(e.currentTarget.value) || 3 })}
              />
            </label>
            <button class="ghost danger sm" onclick={() => removeCue(c.key)}>Remove</button>
          </div>

          {#if selectedDescriptor && config}
            {#key selectedDescriptor.key}
              <AudioBench
                {gameId}
                asset={selectedDescriptor}
                {config}
                onsaved={(r) => (records = { ...records, [r.key]: r })}
              />
            {/key}
          {:else}
            <p class="muted small pad">Save to derive this cue's asset…</p>
          {/if}
        {:else}
          <div class="enable">
            <p class="u-label">No cue selected</p>
            <p class="muted">Pick a cue on the left, or ＋ to add music or an effect.</p>
          </div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .sound {
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
  .style {
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
    gap: var(--space-4);
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
  .cue {
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
  .cue:hover {
    background: var(--wash);
  }
  .cue.on {
    background: var(--wash);
    color: var(--bone);
    box-shadow: inset 2px 0 0 var(--gold);
  }
  .d {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    flex: none;
    background: var(--ink-5);
  }
  .d.ready {
    background: var(--sage);
  }
  .d.gen {
    background: var(--gold);
  }
  .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .loop {
    color: var(--ash);
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
  .cue-settings {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .cue-name {
    font-size: 0.85rem;
    font-weight: 600;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.3rem 0.5rem;
    min-width: 180px;
  }
  .key {
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .sep {
    flex: 1;
  }
  .mini,
  .mini-num input {
    font-size: 0.74rem;
    background: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: 0.25rem 0.4rem;
  }
  .mini-num {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .mini-num input {
    width: 3.6rem;
  }
  .mini-check {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: var(--text-xs);
    color: var(--bone-dim);
  }
  .sm {
    font-size: 0.72rem;
    padding: 0.22rem 0.6rem;
  }
  .pad {
    padding: var(--space-5);
  }
  .tiny {
    font-size: var(--text-xs);
  }
</style>
