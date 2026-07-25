<script lang="ts">
  /**
   * The keyframe timeline: clip tabs, playback bar, scrub ruler, and one track row per
   * timeline with draggable key diamonds. Alt-click a row inserts a key at that time with
   * the interpolated value. All edits mutate the clip in place and report via onedit.
   */
  import type { Clip, Key, StudioDoc, Timeline as TL, TimelineTarget } from "$lib/ipc";

  let {
    doc,
    clip,
    time,
    playing,
    loop,
    onion,
    selection,
    onselectclip,
    onaddclip,
    ondeleteclip,
    onscrub,
    onplaytoggle,
    onlooptoggle,
    ononiontoggle,
    onselectkey,
    onedit,
  }: {
    doc: StudioDoc;
    clip: Clip | null;
    time: number;
    playing: boolean;
    loop: boolean;
    onion: boolean;
    selection: { t: number; k: number } | null;
    onselectclip: (id: string) => void;
    onaddclip: (name: string) => void;
    ondeleteclip: (id: string) => void;
    onscrub: (t: number) => void;
    onplaytoggle: () => void;
    onlooptoggle: () => void;
    ononiontoggle: () => void;
    onselectkey: (sel: { t: number; k: number } | null) => void;
    /** Any structural change to the clip (keys moved/added/removed, duration…). */
    onedit: () => void;
  } = $props();

  const PRESETS = ["idle", "anticipation", "win", "win_big", "bonus", "expand"];
  let addOpen = $state(false);
  let customName = $state("");
  let trackAddOpen = $state(false);
  let newBone = $state("");
  let newChannel = $state<"rotate" | "translate" | "scale" | "alpha">("rotate");

  let rowsEl = $state<HTMLDivElement | null>(null);

  const fps = $derived(doc.settings?.fps ?? 30);
  const duration = $derived(clip?.duration ?? 2);

  const snap = (t: number) => Math.round(t * fps) / fps;
  const clampT = (t: number) => Math.min(Math.max(t, 0), duration);

  function targetLabel(t: TimelineTarget): string {
    if ("boneRotate" in t) return `${t.boneRotate} · rotate`;
    if ("boneTranslate" in t) return `${t.boneTranslate} · move`;
    if ("boneScale" in t) return `${t.boneScale} · scale`;
    return `${(t as { slotAlpha: string }).slotAlpha} · alpha`;
  }

  function components(t: TimelineTarget): number {
    return "boneTranslate" in t || "boneScale" in t ? 2 : 1;
  }

  function defaultValue(t: TimelineTarget): number[] {
    if ("boneScale" in t) return [1, 1];
    if ("boneTranslate" in t) return [0, 0];
    if ("slotAlpha" in t) return [1];
    return [0];
  }

  /** Linear interpolation of a timeline's value at time t (good enough for inserts). */
  function valueAt(tl: TL, t: number): number[] {
    const keys = tl.keys;
    if (!keys.length) return defaultValue(tl.target);
    if (t <= (keys[0].time ?? 0)) return [...keys[0].v.map((v) => v ?? 0)];
    const last = keys[keys.length - 1];
    if (t >= (last.time ?? 0)) return [...last.v.map((v) => v ?? 0)];
    for (let i = 0; i + 1 < keys.length; i++) {
      const a = keys[i];
      const b = keys[i + 1];
      const ta = a.time ?? 0;
      const tb = b.time ?? 0;
      if (t >= ta && t <= tb) {
        const f = tb > ta ? (t - ta) / (tb - ta) : 0;
        return a.v.map((av, ci) => (av ?? 0) + ((b.v[ci] ?? 0) - (av ?? 0)) * f);
      }
    }
    return defaultValue(tl.target);
  }

  // ── Ruler / scrubbing ────────────────────────────────────────────────────────
  let scrubbing = false;
  function rulerTime(e: PointerEvent, el: HTMLElement): number {
    const r = el.getBoundingClientRect();
    return clampT(((e.clientX - r.left) / Math.max(r.width, 1)) * duration);
  }
  function rulerDown(e: PointerEvent) {
    scrubbing = true;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    onscrub(rulerTime(e, e.currentTarget as HTMLElement));
  }
  function rulerMove(e: PointerEvent) {
    if (scrubbing) onscrub(rulerTime(e, e.currentTarget as HTMLElement));
  }

  // ── Key dragging ─────────────────────────────────────────────────────────────
  let dragKey: { t: number; k: number } | null = null;

  function keyDown(e: PointerEvent, ti: number, ki: number) {
    e.stopPropagation();
    onselectkey({ t: ti, k: ki });
    dragKey = { t: ti, k: ki };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }

  function keyMove(e: PointerEvent) {
    if (!dragKey || !clip || !rowsEl) return;
    const row = rowsEl.querySelector(`[data-row="${dragKey.t}"] .lane`) as HTMLElement | null;
    if (!row) return;
    const t = snap(rulerTime(e, row));
    const tl = clip.timelines[dragKey.t];
    tl.keys[dragKey.k].time = t;
    onedit();
  }

  function keyUp() {
    if (!dragKey || !clip) {
      dragKey = null;
      return;
    }
    // Re-sort keys; keep the dragged key selected at its new index.
    const tl = clip.timelines[dragKey.t];
    const dragged = tl.keys[dragKey.k];
    tl.keys.sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
    onselectkey({ t: dragKey.t, k: tl.keys.indexOf(dragged) });
    dragKey = null;
    onedit();
  }

  function laneClick(e: MouseEvent, ti: number) {
    if (!clip) return;
    const el = (e.currentTarget as HTMLElement);
    const r = el.getBoundingClientRect();
    const t = snap(clampT(((e.clientX - r.left) / Math.max(r.width, 1)) * duration));
    if (e.altKey) {
      const tl = clip.timelines[ti];
      const v = valueAt(tl, t);
      const key: Key = { time: t, v, curve: "linear" };
      tl.keys.push(key);
      tl.keys.sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
      onselectkey({ t: ti, k: tl.keys.indexOf(key) });
      onedit();
    } else {
      onscrub(t);
    }
  }

  // ── Track management ─────────────────────────────────────────────────────────
  function addTrack() {
    if (!clip || !newBone) return;
    const target: TimelineTarget =
      newChannel === "rotate"
        ? { boneRotate: newBone }
        : newChannel === "translate"
          ? { boneTranslate: newBone }
          : newChannel === "scale"
            ? { boneScale: newBone }
            : { slotAlpha: newBone };
    if (clip.timelines.some((tl) => JSON.stringify(tl.target) === JSON.stringify(target))) {
      trackAddOpen = false;
      return;
    }
    clip.timelines.push({ target, keys: [{ time: 0, v: defaultValue(target), curve: "linear" }] });
    trackAddOpen = false;
    onedit();
  }

  function removeTrack(ti: number) {
    if (!clip) return;
    clip.timelines.splice(ti, 1);
    onselectkey(null);
    onedit();
  }

  const slotNames = $derived(doc.slots.map((s) => s.name));
  const boneNames = $derived(doc.bones.filter((b) => b.parent).map((b) => b.name));
</script>

<div class="timeline">
  <div class="clips-row">
    {#each doc.clips as c (c.id)}
      <button
        class="ctab mono"
        class:on={clip?.id === c.id}
        onclick={() => onselectclip(c.id)}
      >
        {c.name}
      </button>
    {/each}
    <div class="addwrap">
      <button class="ghost ctab" onclick={() => (addOpen = !addOpen)}>+</button>
      {#if addOpen}
        <div class="addmenu card">
          {#each PRESETS.filter((p) => !doc.clips.some((c) => c.name === p)) as p (p)}
            <button
              class="ghost"
              onclick={() => {
                onaddclip(p);
                addOpen = false;
              }}
            >
              {p}
            </button>
          {/each}
          <form
            onsubmit={(e) => {
              e.preventDefault();
              if (customName.trim()) onaddclip(customName.trim());
              customName = "";
              addOpen = false;
            }}
          >
            <input bind:value={customName} placeholder="custom name" />
          </form>
        </div>
      {/if}
    </div>
    {#if clip && doc.clips.length > 1}
      <button class="ghost del" title="delete clip" onclick={() => ondeleteclip(clip!.id)}>×</button>
    {/if}

    <span class="spacer"></span>
    {#if clip}
      <label class="opt">
        <input type="checkbox" checked={clip.looping} onchange={() => { clip!.looping = !clip!.looping; onedit(); }} />
        <span class="tiny">loop clip</span>
      </label>
      <label class="opt tiny muted">
        dur
        <input
          class="dur mono"
          type="number"
          min="0.2"
          step="0.1"
          value={clip.duration}
          onchange={(e) => {
            clip!.duration = Math.max(+e.currentTarget.value || 1, 0.2);
            onedit();
          }}
        />s
      </label>
    {/if}
  </div>

  <div class="playbar">
    <button class="play" onclick={onplaytoggle}>{playing ? "❚❚" : "▶"}</button>
    <span class="mono t">{time.toFixed(2)} / {duration.toFixed(2)}s</span>
    <label class="opt"><input type="checkbox" checked={loop} onchange={onlooptoggle} /><span class="tiny">loop</span></label>
    <label class="opt" title="onion skin: ghost the neighboring frames — red = past, green = future"><input type="checkbox" checked={onion} onchange={ononiontoggle} /><span class="tiny">onion</span></label>
    <span class="muted tiny mono">{fps} fps grid · alt-click lane = add key</span>
  </div>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="ruler" onpointerdown={rulerDown} onpointermove={rulerMove} onpointerup={() => (scrubbing = false)}>
    {#each Array.from({ length: Math.floor(duration) + 1 }, (_, i) => i) as s (s)}
      <span class="tick mono" style:left={`${(s / duration) * 100}%`}>{s}s</span>
    {/each}
    <div class="playhead" style:left={`${(time / duration) * 100}%`}></div>
  </div>

  <div class="rows" bind:this={rowsEl}>
    {#if clip}
      {#each clip.timelines as tl, ti (targetLabel(tl.target))}
        <div class="row" data-row={ti}>
          <div class="label">
            <span class="mono lname">{targetLabel(tl.target)}</span>
            <button class="ghost x" title="remove track" onclick={() => removeTrack(ti)}>×</button>
          </div>
          <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
          <div class="lane" onclick={(e) => laneClick(e, ti)}>
            <div class="lane-playhead" style:left={`${(time / duration) * 100}%`}></div>
            {#each tl.keys as k, ki (ki)}
              <button
                class="diamond"
                class:sel={selection?.t === ti && selection?.k === ki}
                class:stepped={k.curve === "stepped"}
                style:left={`${(((k.time ?? 0) / duration) * 100).toFixed(3)}%`}
                title={`t=${(k.time ?? 0).toFixed(2)} v=${k.v.map((x) => (x ?? 0).toFixed(2)).join(",")}`}
                onpointerdown={(e) => keyDown(e, ti, ki)}
                onpointermove={keyMove}
                onpointerup={keyUp}
              ></button>
            {/each}
          </div>
        </div>
      {/each}

      <div class="row addtrack">
        {#if trackAddOpen}
          <select bind:value={newChannel}>
            <option value="rotate">rotate</option>
            <option value="translate">translate</option>
            <option value="scale">scale</option>
            <option value="alpha">alpha (slot)</option>
          </select>
          <select bind:value={newBone}>
            <option value="" disabled>target…</option>
            {#each newChannel === "alpha" ? slotNames : boneNames as n (n)}
              <option value={n}>{n}</option>
            {/each}
          </select>
          <button onclick={addTrack} disabled={!newBone}>Add</button>
          <button class="ghost" onclick={() => (trackAddOpen = false)}>Cancel</button>
        {:else}
          <button class="ghost" onclick={() => (trackAddOpen = true)}>+ Track</button>
        {/if}
      </div>
    {:else}
      <p class="muted small empty">No clip — add one above.</p>
    {/if}
  </div>
</div>

<style>
  .timeline {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }
  .clips-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.45rem 0.9rem;
    border-bottom: 1px solid var(--line);
  }
  .ctab {
    font-size: 0.72rem;
    padding: 0.22rem 0.7rem;
    border-radius: 999px;
    border: 1px solid var(--line);
    background: transparent;
    color: var(--ash);
  }
  .ctab.on {
    color: var(--gold-bright);
    border-color: var(--gold-deep);
  }
  .addwrap {
    position: relative;
  }
  .addmenu {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.5rem;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    min-width: 130px;
  }
  .addmenu input {
    font-size: 0.72rem;
    width: 120px;
  }
  .del:hover {
    color: var(--oxblood);
  }
  .spacer {
    flex: 1;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    color: var(--bone-dim);
  }
  .dur {
    width: 3.6rem;
    font-size: 0.72rem;
  }
  .playbar {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    padding: 0.4rem 0.9rem;
    border-bottom: 1px solid var(--line);
  }
  .play {
    width: 2.2rem;
  }
  .t {
    font-size: 0.75rem;
    color: var(--bone-dim);
    min-width: 7.5rem;
  }
  .ruler {
    position: relative;
    height: 20px;
    margin-left: 170px;
    border-bottom: 1px solid var(--line);
    cursor: ew-resize;
    touch-action: none;
  }
  .tick {
    position: absolute;
    top: 2px;
    font-size: 0.58rem;
    color: var(--ash-deep);
    transform: translateX(2px);
  }
  .playhead,
  .lane-playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--gold);
    pointer-events: none;
  }
  .rows {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }
  .row {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid rgba(35, 35, 39, 0.6);
  }
  .label {
    width: 170px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 0.6rem 0 0.9rem;
  }
  .lname {
    font-size: 0.66rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .x {
    padding: 0 0.2rem;
    font-size: 0.7rem;
    opacity: 0;
  }
  .row:hover .x {
    opacity: 1;
  }
  .x:hover {
    color: var(--oxblood);
  }
  .lane {
    position: relative;
    flex: 1;
    height: 26px;
    cursor: crosshair;
  }
  .diamond {
    position: absolute;
    top: 50%;
    width: 9px;
    height: 9px;
    padding: 0;
    border: 1px solid var(--ash);
    background: var(--ink);
    transform: translate(-50%, -50%) rotate(45deg);
    cursor: grab;
    touch-action: none;
  }
  .diamond.stepped {
    border-radius: 0;
    background: var(--ash-deep);
  }
  .diamond.sel {
    border-color: var(--gold-bright);
    background: var(--gold);
  }
  .addtrack {
    padding: 0.4rem 0.9rem;
    gap: 0.5rem;
    align-items: center;
    border-bottom: none;
  }
  .addtrack select {
    font-size: 0.72rem;
  }
  .empty {
    padding: 0.8rem 0.9rem;
  }
</style>
