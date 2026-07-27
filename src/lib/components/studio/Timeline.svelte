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
    speed,
    selection,
    onselectclip,
    onaddclip,
    ondeleteclip,
    onrenameclip,
    onscrub,
    onplaytoggle,
    onlooptoggle,
    ononiontoggle,
    onspeed,
    onselectkey,
    onedit,
  }: {
    doc: StudioDoc;
    clip: Clip | null;
    time: number;
    playing: boolean;
    loop: boolean;
    onion: boolean;
    speed: number;
    selection: { t: number; k: number } | null;
    onselectclip: (id: string) => void;
    onaddclip: (name: string) => void;
    ondeleteclip: (id: string) => void;
    onrenameclip: (id: string, name: string) => void;
    onscrub: (t: number) => void;
    onplaytoggle: () => void;
    onlooptoggle: () => void;
    ononiontoggle: () => void;
    onspeed: (s: number) => void;
    onselectkey: (sel: { t: number; k: number } | null) => void;
    /** Any structural change to the clip (keys moved/added/removed, duration…). */
    onedit: () => void;
  } = $props();

  const PRESETS = ["idle", "anticipation", "win", "win_big", "bonus", "expand"];
  let addOpen = $state(false);
  let customName = $state("");
  let trackAddOpen = $state(false);
  let newBone = $state("");
  let newChannel = $state<
    "rotate" | "translate" | "scale" | "alpha" | "color" | "attachment"
  >("rotate");
  let renaming = $state<string | null>(null);
  let renameVal = $state("");

  function commitRename(id: string) {
    if (renameVal.trim()) onrenameclip(id, renameVal.trim());
    renaming = null;
  }

  let rowsEl = $state<HTMLDivElement | null>(null);

  const fps = $derived(doc.settings?.fps ?? 30);
  const duration = $derived(clip?.duration ?? 2);

  const snap = (t: number) => Math.round(t * fps) / fps;
  const clampT = (t: number) => Math.min(Math.max(t, 0), duration);

  function targetLabel(t: TimelineTarget): string {
    if ("boneRotate" in t) return `${t.boneRotate} · rotate`;
    if ("boneTranslate" in t) return `${t.boneTranslate} · move`;
    if ("boneScale" in t) return `${t.boneScale} · scale`;
    if ("slotAlpha" in t) return `${t.slotAlpha} · alpha`;
    if ("slotColor" in t) return `${t.slotColor} · color`;
    if ("slotDeform" in t) return `${t.slotDeform} · deform`;
    return `${(t as { slotAttachment: string }).slotAttachment} · swap`;
  }

  /** The bone/slot a track targets (the single key's value). */
  function chName(t: TimelineTarget): string {
    return (Object.values(t)[0] as string) ?? "";
  }
  /** Per-channel glyph + colour class for the track row. */
  function chKind(t: TimelineTarget): { icon: string; cls: string; label: string } {
    if ("boneRotate" in t) return { icon: "↻", cls: "rot", label: "rotate" };
    if ("boneTranslate" in t) return { icon: "✛", cls: "move", label: "translate" };
    if ("boneScale" in t) return { icon: "⤢", cls: "scale", label: "scale" };
    if ("slotAlpha" in t) return { icon: "◐", cls: "alpha", label: "alpha" };
    if ("slotColor" in t) return { icon: "●", cls: "color", label: "colour" };
    if ("slotDeform" in t) return { icon: "∿", cls: "deform", label: "deform" };
    return { icon: "⇄", cls: "swap", label: "swap" };
  }
  /** What an attachment-swap track flips to (first alternate, or plain show/hide). */
  function swapMeta(t: TimelineTarget): string {
    const name = "slotAttachment" in t ? t.slotAttachment : "";
    return doc.slots.find((s) => s.name === name)?.alternates?.[0] ?? "show / hide";
  }

  function components(t: TimelineTarget): number {
    if ("slotColor" in t) return 3;
    if ("slotAttachment" in t) return 1;
    if ("slotDeform" in t) return 0; // dynamic (2×verts) — generated, not scalar-edited
    return "boneTranslate" in t || "boneScale" in t ? 2 : 1;
  }

  function defaultValue(t: TimelineTarget): number[] {
    if ("boneScale" in t) return [1, 1];
    if ("boneTranslate" in t) return [0, 0];
    if ("slotColor" in t) return [1, 1, 1]; // white = no tint
    if ("slotAlpha" in t) return [1];
    if ("slotAttachment" in t) return [0]; // base attachment
    if ("slotDeform" in t) return []; // generated only
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
            : newChannel === "alpha"
              ? { slotAlpha: newBone }
              : newChannel === "attachment"
                ? { slotAttachment: newBone }
                : { slotColor: newBone };
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
      {#if renaming === c.id}
        <form onsubmit={(e) => { e.preventDefault(); commitRename(c.id); }}>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="renameinput mono"
            bind:value={renameVal}
            autofocus
            onblur={() => commitRename(c.id)}
          />
        </form>
      {:else}
        <button
          class="ctab mono"
          class:on={clip?.id === c.id}
          onclick={() => onselectclip(c.id)}
          ondblclick={() => { renaming = c.id; renameVal = c.name; }}
          title="double-click to rename"
        >
          {c.name}
          <span class="cglyph" class:oneshot={c.looping === false} title={c.looping === false ? "one-shot beat" : "looping idle"}>
            {c.looping === false ? "▸" : "∞"}
          </span>
        </button>
      {/if}
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
    <button class="play" onclick={onplaytoggle} title={playing ? "pause" : "play"}>{playing ? "❚❚" : "▶"}</button>
    <span class="mono t num">{time.toFixed(2)} / {duration.toFixed(2)}s</span>
    <button class="tgl" class:on={loop} onclick={onlooptoggle} title="loop playback">∞ loop</button>
    <button
      class="tgl"
      class:on={onion}
      onclick={ononiontoggle}
      title="onion skin: ghost the neighbouring frames — red = past, green = future"
    >◐ onion</button>
    <div class="seg" title="playback speed">
      {#each [0.25, 0.5, 1, 1.5, 2] as s (s)}
        <button class:on={speed === s} onclick={() => onspeed(s)}>{s}×</button>
      {/each}
    </div>
    <span class="muted tiny mono">{fps} fps · alt-click lane = add key</span>
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
            <span class="ticon {chKind(tl.target).cls}" title={chKind(tl.target).label}>{chKind(tl.target).icon}</span>
            <span class="tname mono">{chName(tl.target)}</span>
            {#if "slotDeform" in tl.target}
              <span class="tmeta gen">generated · {((tl.keys[0]?.v.length ?? 0) / 2) | 0} verts</span>
            {:else if "slotAttachment" in tl.target}
              <span class="tmeta att mono">{swapMeta(tl.target)}</span>
            {/if}
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
                title={"slotDeform" in tl.target
                  ? `t=${(k.time ?? 0).toFixed(2)} · deform (${(k.v.length / 2) | 0} verts)`
                  : "slotAttachment" in tl.target
                    ? `t=${(k.time ?? 0).toFixed(2)} · #${Math.round(k.v[0] ?? 0)}`
                    : `t=${(k.time ?? 0).toFixed(2)} v=${k.v.map((x) => (x ?? 0).toFixed(2)).join(",")}`}
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
            <option value="color">color (slot)</option>
            <option value="attachment">swap (slot)</option>
          </select>
          <select bind:value={newBone}>
            <option value="" disabled>target…</option>
            {#each newChannel === "alpha" || newChannel === "color" || newChannel === "attachment" ? slotNames : boneNames as n (n)}
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
  .cglyph {
    color: var(--ash-deep);
    font-size: 0.72rem;
    margin-left: 0.15rem;
  }
  .ctab.on .cglyph {
    color: var(--lapis);
  }
  .ctab.on .cglyph.oneshot {
    color: var(--gold);
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
  .renameinput {
    font-size: 0.72rem;
    width: 5.5rem;
  }
  .tgl {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.68rem;
    color: var(--ash);
    background: transparent;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm, 6px);
    padding: 0.15rem 0.5rem;
  }
  .tgl.on {
    color: var(--bone);
    border-color: var(--line-2);
    background: var(--wash);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm, 6px);
    overflow: hidden;
  }
  .seg button {
    border: 0;
    background: transparent;
    color: var(--ash);
    font-size: 0.66rem;
    padding: 0.15rem 0.45rem;
    font-variant-numeric: tabular-nums;
  }
  .seg button + button {
    border-left: 1px solid var(--line);
  }
  .seg button.on {
    color: var(--bone);
    background: var(--wash);
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
    gap: 0.4rem;
    padding: 0 0.45rem 0 0.6rem;
    min-width: 0;
  }
  .ticon {
    width: 17px;
    height: 17px;
    flex: none;
    display: grid;
    place-items: center;
    font-size: 0.66rem;
    border-radius: 4px;
    background: var(--ink-4);
    border: 1px solid var(--line);
    color: var(--ash);
  }
  .ticon.rot,
  .ticon.move,
  .ticon.scale {
    color: var(--lapis);
  }
  .ticon.color,
  .ticon.alpha {
    color: var(--gold);
  }
  .ticon.deform {
    color: var(--sage);
  }
  .ticon.swap {
    color: var(--oxblood);
  }
  .tname {
    font-size: 0.72rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tmeta {
    font-size: 0.56rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    white-space: nowrap;
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 5.5rem;
  }
  .tmeta.gen {
    color: var(--sage);
    background: color-mix(in srgb, var(--sage) 14%, transparent);
  }
  .tmeta.att {
    color: var(--oxblood);
  }
  .x {
    margin-left: auto;
    padding: 0 0.2rem;
    font-size: 0.7rem;
    opacity: 0;
    flex: none;
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
