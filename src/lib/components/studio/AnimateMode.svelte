<script lang="ts">
  /**
   * Animate mode: the real-runtime stage with pose gizmos (drag a bone → auto-key at the
   * playhead), the keyframe timeline below, and the key inspector (values + easing) on
   * the right. Keyframe edits ride the skeleton-only fast path — atlas textures are
   * reused, so the preview updates in tens of milliseconds.
   */
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    commands,
    unwrap,
    type Clip,
    type Curve,
    type Key,
    type PreviewBundle,
    type StudioDoc,
    type TimelineTarget,
  } from "$lib/ipc";
  import SpinePreview, { type StageBone } from "./SpinePreview.svelte";
  import Timeline from "./Timeline.svelte";
  import CurveEditor from "./CurveEditor.svelte";
  import {
    poseHit,
    beginDrag,
    applyDrag,
    drawGizmos as drawPoseGizmos,
    type PoseDrag,
  } from "./poseDrag";

  let {
    gameId,
    assetKey,
    doc,
    save,
  }: {
    gameId: string;
    assetKey: string;
    doc: StudioDoc;
    save: (doc: StudioDoc) => Promise<StudioDoc>;
  } = $props();

  let bundle = $state<PreviewBundle | null>(null);
  let preview = $state<ReturnType<typeof SpinePreview> | null>(null);
  let error = $state("");

  let activeClipId = $state<string | null>(null);
  let time = $state(0);
  let playing = $state(false);
  let loop = $state(true);
  let onion = $state(false);
  let speed = $state(1);
  let curveComp = $state(0); // which value component's easing curve the editor shows
  let keyClipboard: { v: number[]; curve: Curve } | null = null;
  $effect(() => {
    void selection; // reset the curve-component selector when the selected key changes
    curveComp = 0;
  });
  let selection = $state<{ t: number; k: number } | null>(null);
  let selectedBone = $state<string | null>(null);

  let overlay = $state<HTMLCanvasElement | null>(null);
  let stageHost = $state<HTMLElement | null>(null);

  const clip = $derived(doc.clips.find((c) => c.id === activeClipId) ?? null);
  const fps = $derived(doc.settings?.fps ?? 30);
  const snap = (t: number) => Math.round(t * fps) / fps;

  const EASE: [number, number, number, number] = [0.42, 0, 0.58, 1];

  onMount(() => {
    activeClipId = doc.clips[0]?.id ?? null;
    (async () => {
      try {
        bundle = await unwrap(
          commands.studioPreviewBundle(gameId, assetKey, $state.snapshot(doc) as StudioDoc),
        );
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    })();
    const unlisten = listen<{ current: number; total: number; label: string }>(
      "studio://ai-progress",
      (e) => (aiProg = { cur: e.payload.current, total: e.payload.total, label: e.payload.label }),
    );
    let raf = 0;
    const tick = () => {
      drawGizmos();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      unlisten.then((f) => f());
    };
  });

  // ── Rebuild paths ────────────────────────────────────────────────────────────
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let skelTimer: ReturnType<typeof setTimeout> | null = null;

  /** Persist + refresh the skeleton through the fast path (textures reused). */
  function commit() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await save($state.snapshot(doc) as StudioDoc);
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }, 600);
    if (skelTimer) clearTimeout(skelTimer);
    skelTimer = setTimeout(async () => {
      try {
        const skeletonJson = await unwrap(
          commands.studioSkeletonOnly($state.snapshot(doc) as StudioDoc),
        );
        if (bundle) bundle = { ...bundle, skeletonJson };
        error = "";
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }, 120);
  }

  // ── Clip management ──────────────────────────────────────────────────────────
  function addClip(name: string) {
    const id = name.toLowerCase().replace(/[^a-z0-9]+/g, "_") || `clip_${doc.clips.length}`;
    if (doc.clips.some((c) => c.id === id)) {
      activeClipId = id;
      return;
    }
    const c: Clip = { id, name, duration: 2, looping: true, timelines: [] };
    doc.clips.push(c);
    activeClipId = id;
    selection = null;
    commit();
  }

  function deleteClip(id: string) {
    if (doc.clips.length <= 1) return;
    if (!confirm(`Delete clip "${id}"?`)) return;
    doc.clips = doc.clips.filter((c) => c.id !== id);
    if (activeClipId === id) activeClipId = doc.clips[0]?.id ?? null;
    selection = null;
    commit();
  }

  // ── Key helpers ──────────────────────────────────────────────────────────────
  function findOrCreateTimeline(target: TimelineTarget) {
    if (!clip) return null;
    const key = JSON.stringify(target);
    let tl = clip.timelines.find((t) => JSON.stringify(t.target) === key);
    if (!tl) {
      tl = { target, keys: [] };
      clip.timelines.push(tl);
    }
    return tl;
  }

  /** Insert or replace a key at (snapped) time t. Default ease-in-out out-curve. */
  function setKey(target: TimelineTarget, t: number, v: number[]) {
    const tl = findOrCreateTimeline(target);
    if (!tl || !clip) return;
    const st = snap(Math.min(Math.max(t, 0), clip.duration ?? 2));
    const comps = v.length;
    const existing = tl.keys.find((k) => Math.abs((k.time ?? 0) - st) < 1e-4);
    if (existing) {
      existing.v = v;
    } else {
      const curve: Curve = { bezier: Array(comps).fill(EASE).flat() };
      const key: Key = { time: st, v, curve };
      tl.keys.push(key);
      tl.keys.sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
      selection = { t: clip.timelines.indexOf(tl), k: tl.keys.indexOf(key) };
    }
    commit();
  }

  const selectedKey = $derived.by(() => {
    if (!clip || !selection) return null;
    const tl = clip.timelines[selection.t];
    const k = tl?.keys[selection.k];
    return tl && k ? { tl, k } : null;
  });

  function deleteSelectedKey() {
    if (!clip || !selection || !selectedKey) return;
    selectedKey.tl.keys.splice(selection.k, 1);
    if (!selectedKey.tl.keys.length) {
      clip.timelines.splice(selection.t, 1);
    }
    selection = null;
    commit();
  }

  function curveKind(c: Curve | undefined): "linear" | "stepped" | "bezier" {
    if (c === "stepped") return "stepped";
    if (c && typeof c === "object" && "bezier" in c) return "bezier";
    return "linear";
  }

  function setCurveKind(kind: string) {
    if (!selectedKey) return;
    const comps = selectedKey.k.v.length;
    selectedKey.k.curve =
      kind === "stepped"
        ? "stepped"
        : kind === "bezier"
          ? { bezier: Array(comps).fill(EASE).flat() }
          : "linear";
    commit();
  }

  function bezierHandles(comp: number): [number, number, number, number] {
    const c = selectedKey?.k.curve;
    const o = comp * 4;
    if (c && typeof c === "object" && "bezier" in c && c.bezier.length >= o + 4) {
      return [c.bezier[o] ?? 0, c.bezier[o + 1] ?? 0, c.bezier[o + 2] ?? 1, c.bezier[o + 3] ?? 1];
    }
    return EASE;
  }

  /** Write one component's easing handles, leaving the other components' curves intact. */
  function setBezierHandles(comp: number, h: [number, number, number, number]) {
    if (!selectedKey) return;
    const comps = selectedKey.k.v.length;
    const c = selectedKey.k.curve;
    const arr =
      c && typeof c === "object" && "bezier" in c && c.bezier.length === comps * 4
        ? c.bezier.map((x) => x ?? 0)
        : (Array(comps).fill(EASE).flat() as number[]);
    arr[comp * 4] = h[0];
    arr[comp * 4 + 1] = h[1];
    arr[comp * 4 + 2] = h[2];
    arr[comp * 4 + 3] = h[3];
    selectedKey.k.curve = { bezier: arr };
    commit();
  }

  function keyValueLabel(target: TimelineTarget): string[] {
    if ("boneRotate" in target) return ["deg"];
    if ("boneTranslate" in target) return ["x px", "y px"];
    if ("boneScale" in target) return ["scale x", "scale y"];
    if ("slotColor" in target) return ["r", "g", "b"];
    if ("slotAttachment" in target) return ["attachment"];
    if ("slotDeform" in target) return []; // generated — no scalar fields
    return ["alpha"];
  }

  /** The attachment options for a swap timeline: `[part_id, ...alternates]`. */
  function attachmentNames(target: TimelineTarget): string[] {
    if (!("slotAttachment" in target)) return [];
    const slot = doc.slots.find((s) => s.name === target.slotAttachment);
    return slot ? [slot.partId, ...(slot.alternates ?? [])] : [];
  }

  /** Selected slot-color key ↔ a `#rrggbb` swatch (values are 0..1). */
  function keyHex(v: (number | null)[]): string {
    const b = (x: number | null) => Math.max(0, Math.min(255, Math.round((x ?? 0) * 255)));
    return `#${[0, 1, 2].map((i) => b(v[i]).toString(16).padStart(2, "0")).join("")}`;
  }
  function setKeyColor(hex: string) {
    if (!selectedKey) return;
    const n = (s: string) => parseInt(s, 16) / 255;
    selectedKey.k.v[0] = n(hex.slice(1, 3));
    selectedKey.k.v[1] = n(hex.slice(3, 5));
    selectedKey.k.v[2] = n(hex.slice(5, 7));
    commit();
  }

  // ── Stage gizmos (shared math in poseDrag.ts) ────────────────────────────────
  let drag: PoseDrag | null = null;

  function stageBones(): StageBone[] {
    return preview?.getStageBones().filter((b) => b.parent) ?? [];
  }

  function drawGizmos() {
    if (!overlay || !stageHost) return;
    // Gizmos only while paused; empty list just clears the canvas.
    drawPoseGizmos(overlay, stageHost, playing ? [] : stageBones(), selectedBone, "#edc76a");
  }

  function overlayDown(e: PointerEvent) {
    if (playing || !clip) return;
    const rect = overlay!.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const hit = poseHit(stageBones(), selectedBone, mx, my, e.metaKey || e.ctrlKey);
    if (hit.action === "clear") {
      selectedBone = null;
      return;
    }
    if (hit.action === "select") {
      selectedBone = hit.name;
      return;
    }
    if (hit.select) selectedBone = hit.select;
    const setup = preview?.getSetup(hit.bone.name);
    if (!setup) return;
    overlay!.setPointerCapture(e.pointerId);
    drag = beginDrag(hit.bone, setup, hit.kind, mx, my);
  }

  function overlayMove(e: PointerEvent) {
    if (!drag || !preview) return;
    const rect = overlay!.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const b = stageBones().find((x) => x.name === drag!.bone);
    if (!b) return;
    applyDrag(drag, b, mx, my, (bone, patch) => preview!.setPose(bone, patch));
  }

  function overlayUp() {
    if (!drag || !preview) return;
    const b = preview.getStageBones().find((x) => x.name === drag!.bone);
    if (drag.moved && b) {
      // Commit the pose as a key at the playhead. Doc translate v = [x, -ySpine] (y-down).
      if (drag.kind === "rotate") {
        setKey({ boneRotate: drag.bone }, time, [round2(b.animRotation)]);
      } else {
        setKey({ boneTranslate: drag.bone }, time, [round2(b.animX), round2(-b.animY)]);
      }
    }
    preview.clearPose();
    drag = null;
  }

  const round2 = (v: number) => Math.round(v * 100) / 100;

  /** Key the selected bone's full current pose (rotate + translate) at the playhead. */
  function keyPose() {
    const b = stageBones().find((x) => x.name === selectedBone);
    if (!b) return;
    setKey({ boneRotate: b.name }, time, [round2(b.animRotation)]);
    setKey({ boneTranslate: b.name }, time, [round2(b.animX), round2(-b.animY)]);
  }

  function renameClip(id: string, name: string) {
    const c = doc.clips.find((x) => x.id === id);
    if (!c || doc.clips.some((x) => x.id !== id && x.name === name)) return; // names stay unique
    c.name = name;
    doc.clips = [...doc.clips];
    commit();
  }

  const onionTimes = $derived.by(() => {
    if (!onion || playing || !clip) return [];
    const before = Math.round(doc.settings?.onionBefore ?? 2);
    const after = Math.round(doc.settings?.onionAfter ?? 2);
    const dur = clip.duration ?? 2;
    const out: number[] = [];
    for (let i = before; i >= 1; i--) out.push(time - i / fps);
    for (let i = 1; i <= after; i++) out.push(time + i / fps);
    return out.filter((t) => t >= 0 && t <= dur);
  });

  // Surface that a 2.5D turn / mesh-deform is baked into this symbol (discoverability).
  const hasTurn = $derived(doc.clips.some((c) => c.timelines.some((t) => "slotDeform" in t.target)));

  // ── AI motion (Phase 6) ──────────────────────────────────────────────────────
  // Seeded ONCE from the motion brief written at the Cut step — the same plan the
  // parts were cut for; edits here stay local to the clip being drafted.
  // svelte-ignore state_referenced_locally
  let aiBrief = $state(doc.motionBrief ?? "");
  let aiName = $state("");
  let aiBusy = $state(false);
  let aiMsg = $state("");
  /** Live best-of-N progress from the backend (null while idle / single Draft). */
  let aiProg = $state<{ cur: number; total: number; label: string } | null>(null);
  let aiStash = $state<{ id: string; json: string; existed: boolean } | null>(null);
  let ibFrom = $state(0);
  let ibTo = $state(0);
  let ibCount = $state(2);

  // Prefill the in-between interval from the selected key → its next key.
  $effect(() => {
    if (!selectedKey || !selection) return;
    const keys = selectedKey.tl.keys;
    const next = keys[selection.k + 1];
    ibFrom = selectedKey.k.time ?? 0;
    ibTo = next ? (next.time ?? 0) : (clip?.duration ?? 2);
  });

  function stashClip(id: string) {
    const existing = doc.clips.find((c) => c.id === id);
    aiStash = existing
      ? { id, json: JSON.stringify($state.snapshot(existing)), existed: true }
      : { id, json: "", existed: false };
  }

  function undoAi() {
    if (!aiStash) return;
    const idx = doc.clips.findIndex((c) => c.id === aiStash!.id);
    if (aiStash.existed) {
      const restored = JSON.parse(aiStash.json) as Clip;
      if (idx >= 0) doc.clips[idx] = restored;
      else doc.clips.push(restored);
      activeClipId = restored.id;
    } else if (idx >= 0) {
      doc.clips.splice(idx, 1);
      activeClipId = doc.clips[0]?.id ?? null;
    }
    aiStash = null;
    aiMsg = "Reverted.";
    selection = null;
    commit();
  }

  function applyDrafted(result: Clip) {
    stashClip(result.id);
    const idx = doc.clips.findIndex((c) => c.id === result.id);
    if (idx >= 0) doc.clips[idx] = result;
    else doc.clips.push(result);
    activeClipId = result.id;
    selection = null;
    time = 0;
    playing = true;
    commit();
  }
  async function draftClip() {
    const name = (aiName.trim() || clip?.name || "idle").trim();
    aiBusy = true;
    aiProg = null; // single call → indeterminate spinner
    aiMsg = `Drafting “${name}”…`;
    try {
      const result = await unwrap(commands.studioAiDraftClip(gameId, assetKey, name, aiBrief.trim()));
      applyDrafted(result);
      aiMsg = `Drafted “${result.name}” — ${result.timelines.length} tracks. Refine or Revert.`;
    } catch (e) {
      aiMsg = e instanceof Error ? e.message : String(e);
    } finally {
      aiBusy = false;
      aiProg = null;
    }
  }
  async function polishClip() {
    const name = (aiName.trim() || clip?.name || "idle").trim();
    aiBusy = true;
    aiProg = null; // filled by studio://ai-progress step events
    aiMsg = `Polishing “${name}”…`;
    try {
      const result = await unwrap(commands.studioAiPolishClip(gameId, assetKey, name, aiBrief.trim(), 3));
      applyDrafted(result);
      aiMsg = `Polished “${result.name}” — best of 3, ${result.timelines.length} tracks. Refine or Revert.`;
    } catch (e) {
      aiMsg = e instanceof Error ? e.message : String(e);
    } finally {
      aiBusy = false;
      aiProg = null;
    }
  }

  // ── Spritesheet bake — invoked from the Studio top bar (it's an exporter, not an
  //    AI tool). Returns a status message for the caller's strip; throws on failure.
  export async function bakeSheet(): Promise<string> {
    if (!clip || !preview) throw new Error("open a clip first");
    const { frames } = await preview.bakeFrames(clip.name, fps, 512);
    if (!frames.length) throw new Error("no frames rendered");
    const path = await unwrap(
      commands.studioSaveSpritesheet(gameId, assetKey, clip.name, fps, frames),
    );
    return `Baked ${frames.length} frames → ${path.split("/").slice(-3).join("/")}`;
  }
  export function canBake(): boolean {
    return !!clip;
  }

  async function runInbetween() {
    if (!clip) return;
    aiBusy = true;
    aiMsg = `In-betweening ${ibFrom.toFixed(2)}–${ibTo.toFixed(2)}s…`;
    try {
      const result = await unwrap(
        commands.studioAiInbetween(
          gameId,
          assetKey,
          $state.snapshot(clip) as Clip,
          ibFrom,
          ibTo,
          ibCount,
        ),
      );
      stashClip(clip.id);
      const idx = doc.clips.findIndex((c) => c.id === clip!.id);
      if (idx >= 0) doc.clips[idx] = result;
      selection = null;
      aiMsg = "Breakdowns added. Refine or Revert.";
      commit();
    } catch (e) {
      aiMsg = e instanceof Error ? e.message : String(e);
    } finally {
      aiBusy = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT" || el.tagName === "SELECT" || el.tagName === "TEXTAREA") return;
    if (e.key === " ") {
      e.preventDefault();
      playing = !playing;
    } else if ((e.key === "Backspace" || e.key === "Delete") && selection) {
      e.preventDefault();
      deleteSelectedKey();
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c" && selection && clip) {
      e.preventDefault();
      const k = clip.timelines[selection.t]?.keys[selection.k];
      if (k) keyClipboard = { v: k.v.map((x) => x ?? 0), curve: k.curve ?? "linear" };
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v" && keyClipboard && selection && clip) {
      // Paste the copied key onto the selected track at the playhead (matching arity).
      e.preventDefault();
      const tl = clip.timelines[selection.t];
      if (tl && keyValueLabel(tl.target).length === keyClipboard.v.length) {
        const t = snap(time);
        tl.keys = tl.keys.filter((kk) => Math.abs((kk.time ?? 0) - t) > 1e-6);
        const key: Key = { time: t, v: [...keyClipboard.v], curve: keyClipboard.curve };
        tl.keys.push(key);
        tl.keys.sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
        selection = { t: selection.t, k: tl.keys.indexOf(key) };
        commit();
      }
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="animate">
  <div class="upper">
    <section class="stage-wrap" bind:this={stageHost}>
      <SpinePreview
        bind:this={preview}
        {bundle}
        clip={clip?.name ?? null}
        {playing}
        loop={loop && (clip?.looping ?? true)}
        time={playing ? null : time}
        {onionTimes}
        {speed}
        onstatus={(s) => (error = s.error ?? "")}
        ontick={(t) => (time = t)}
      />
      <canvas
        class="gizmos"
        bind:this={overlay}
        onpointerdown={overlayDown}
        onpointermove={overlayMove}
        onpointerup={overlayUp}
      ></canvas>
      {#if error}<div class="stage-err">{error}</div>{/if}
      {#if hasTurn}<span class="turn-chip" title="a 2.5D depth turn / mesh deform is baked in">2.5D turn · baked</span>{/if}
      {#if !playing}
        <span class="stage-hint muted tiny">
          click bone = select · drag tip = rotate · ⌘-drag = move · keys land at the playhead
        </span>
      {/if}
    </section>

    <aside class="inspector">
      {#if selectedKey}
        <span class="u-label">Key</span>
        <p class="mono ktarget">{JSON.stringify(selectedKey.tl.target).replace(/[{}"]/g, "")}</p>
        <label class="field">
          <span class="muted tiny">time</span>
          <input
            type="number"
            step={1 / fps}
            min="0"
            value={selectedKey.k.time ?? 0}
            onchange={(e) => {
              selectedKey!.k.time = snap(+e.currentTarget.value || 0);
              selectedKey!.tl.keys.sort((a, b) => (a.time ?? 0) - (b.time ?? 0));
              commit();
            }}
          />
        </label>
        {#if "slotColor" in selectedKey.tl.target}
          <label class="field">
            <span class="muted tiny">tint (white = none)</span>
            <input
              class="swatch"
              type="color"
              value={keyHex(selectedKey.k.v)}
              oninput={(e) => setKeyColor(e.currentTarget.value)}
            />
          </label>
        {/if}
        {#if "slotAttachment" in selectedKey.tl.target}
          <label class="field">
            <span class="muted tiny">show attachment</span>
            <select
              value={String(Math.round(selectedKey.k.v[0] ?? 0))}
              onchange={(e) => {
                selectedKey!.k.v[0] = +e.currentTarget.value;
                commit();
              }}
            >
              <option value="-1">— hidden —</option>
              {#each attachmentNames(selectedKey.tl.target) as nm, i (nm)}
                <option value={String(i)}>{nm}{i === 0 ? " (base)" : ""}</option>
              {/each}
            </select>
          </label>
        {:else if "slotDeform" in selectedKey.tl.target}
          <p class="muted tiny">
            generated — {(selectedKey.k.v.length / 2) | 0} vertices (2.5D turn / ripple)
          </p>
        {:else}
          {#each keyValueLabel(selectedKey.tl.target) as lbl, ci (lbl)}
            <label class="field">
              <span class="muted tiny">{lbl}</span>
              <input
                type="number"
                step={"slotColor" in selectedKey.tl.target ? 0.05 : 0.1}
                value={selectedKey.k.v[ci] ?? 0}
                onchange={(e) => {
                  selectedKey!.k.v[ci] = +e.currentTarget.value || 0;
                  commit();
                }}
              />
            </label>
          {/each}
        {/if}
        {#if !("slotAttachment" in selectedKey.tl.target) && !("slotDeform" in selectedKey.tl.target)}
          <label class="field">
            <span class="muted tiny">ease out</span>
            <select value={curveKind(selectedKey.k.curve)} onchange={(e) => setCurveKind(e.currentTarget.value)}>
              <option value="linear">linear</option>
              <option value="bezier">bezier</option>
              <option value="stepped">stepped</option>
            </select>
          </label>
          {#if curveKind(selectedKey.k.curve) === "bezier"}
            {#if keyValueLabel(selectedKey.tl.target).length > 1}
              <div class="curvecomps">
                {#each keyValueLabel(selectedKey.tl.target) as lbl, ci (lbl)}
                  <button class="ccomp" class:on={curveComp === ci} onclick={() => (curveComp = ci)}>
                    {lbl}
                  </button>
                {/each}
              </div>
            {/if}
            <CurveEditor handles={bezierHandles(curveComp)} onchange={(h) => setBezierHandles(curveComp, h)} />
          {/if}
        {/if}
        <button class="ghost danger" onclick={deleteSelectedKey}>Delete key</button>
      {:else if selectedBone}
        <span class="u-label">Bone</span>
        <p class="mono ktarget">{selectedBone}</p>
        <p class="muted tiny">
          Drag the tip to rotate, ⌘-drag the ring to move. Releasing sets a key at the
          playhead.
        </p>
        <button onclick={keyPose}>Key pose at {time.toFixed(2)}s</button>
      {:else}
        <span class="u-label">Inspector</span>
        <p class="muted small">Select a key diamond or a bone on the stage.</p>
      {/if}
    </aside>
  </div>

  <div class="ai-bar">
    <div class="ai-top">
      <span class="ai-glyph" title="AI motion director">✦</span>
      <input
        class="ai-name mono"
        bind:value={aiName}
        placeholder={clip?.name ?? "clip"}
        title="target clip name (new or existing)"
      />
      <span class="ai-loop" title={clip?.looping === false ? "one-shot" : "looping"}>
        {clip?.looping === false ? "▸" : "∞"}
      </span>
      <textarea
        class="ai-brief"
        rows="1"
        bind:value={aiBrief}
        placeholder="describe the motion… e.g. “body breathes, the axe glints, then a sharp win pop”"
        onkeydown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !aiBusy && aiBrief.trim()) {
            e.preventDefault();
            draftClip();
          }
        }}
      ></textarea>
    </div>
    <div class="ai-actions">
      <button onclick={draftClip} disabled={aiBusy || !aiBrief.trim()}>Draft clip</button>
      <button
        class="polish"
        title="Draft 3 candidates and let the AI critic pick the best — costs more"
        onclick={polishClip}
        disabled={aiBusy || !aiBrief.trim()}
      >
        ✦ Polish · best of 3
      </button>
      <span class="ai-sep"></span>
      <label class="ai-ib tiny muted">
        refine
        <input class="ai-num mono" type="number" step="0.1" min="0" bind:value={ibFrom} />–<input
          class="ai-num mono"
          type="number"
          step="0.1"
          min="0"
          bind:value={ibTo}
        />s ×<input class="ai-num mono" type="number" min="1" max="5" bind:value={ibCount} />
      </label>
      <button class="ghost" onclick={runInbetween} disabled={aiBusy || !clip || ibTo <= ibFrom}>
        In-between
      </button>
      {#if aiStash && !aiBusy}
        <button class="ghost revert" onclick={undoAi}>Revert AI</button>
      {/if}
      <div class="ai-status">
        {#if aiBusy}
          <span class="spin"></span>
          {#if aiProg}
            <span class="pbar"><i style:width={`${(aiProg.cur / Math.max(1, aiProg.total)) * 100}%`}></i></span>
            <span class="ai-msg tiny amber">{aiProg.label}</span>
          {:else}
            <span class="ai-msg tiny amber">{aiMsg}</span>
          {/if}
        {:else if aiMsg}
          <span class="ai-msg tiny">{aiMsg}</span>
        {/if}
      </div>
    </div>
  </div>

  <div class="lower">
    <Timeline
      {doc}
      {clip}
      {time}
      {playing}
      {loop}
      {onion}
      {speed}
      {selection}
      onselectclip={(id) => {
        activeClipId = id;
        selection = null;
        time = 0;
      }}
      onaddclip={addClip}
      ondeleteclip={deleteClip}
      onrenameclip={renameClip}
      onscrub={(t) => {
        time = t;
        playing = false;
      }}
      onplaytoggle={() => (playing = !playing)}
      onlooptoggle={() => (loop = !loop)}
      ononiontoggle={() => (onion = !onion)}
      onspeed={(s) => (speed = s)}
      onselectkey={(s) => (selection = s)}
      onedit={commit}
    />
  </div>
</div>

<style>
  .animate {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .upper {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .stage-wrap {
    flex: 1;
    position: relative;
    min-width: 0;
    background: radial-gradient(ellipse at center, var(--wash-faint), transparent 70%);
  }
  .turn-chip {
    position: absolute;
    top: 0.6rem;
    left: 0.6rem;
    z-index: 4;
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.62rem;
    color: var(--gold);
    background: var(--gold-glow);
    border: 1px solid var(--gold-deep);
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    pointer-events: none;
  }
  .turn-chip::before {
    content: "";
    width: 5px;
    height: 5px;
    border-radius: 999px;
    background: var(--gold);
  }
  .gizmos {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    touch-action: none;
  }
  .stage-err {
    position: absolute;
    top: 0.6rem;
    left: 0.9rem;
    right: 0.9rem;
    color: var(--oxblood);
    font-size: 0.72rem;
  }
  .stage-hint {
    position: absolute;
    left: 0.9rem;
    bottom: 0.5rem;
    pointer-events: none;
    opacity: 0.7;
  }
  .inspector {
    width: 210px;
    flex: none;
    border-left: 1px solid var(--line);
    padding: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    overflow-y: auto;
  }
  .ktarget {
    margin: 0;
    font-size: 0.7rem;
    color: var(--bone);
    word-break: break-all;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .field input,
  .field select {
    font-size: 0.75rem;
    width: 100%;
  }
  .swatch {
    height: 26px;
    padding: 1px;
    cursor: pointer;
  }
  .curvecomps {
    display: flex;
    gap: 0.25rem;
  }
  .ccomp {
    flex: 1;
    font-size: 0.66rem;
    padding: 0.1rem 0.2rem;
    background: transparent;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--ash);
  }
  .ccomp.on {
    color: var(--gold-bright);
    border-color: var(--gold-deep);
  }
  .danger:hover {
    color: var(--oxblood);
  }
  .ai-bar {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.5rem 0.9rem;
    border-top: 1px solid var(--line);
    flex: none;
  }
  .ai-top {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
  }
  .ai-glyph {
    color: var(--gold);
    align-self: center;
  }
  .ai-name {
    width: 84px;
    font-size: 0.72rem;
    align-self: stretch;
  }
  .ai-loop {
    align-self: center;
    color: var(--ash);
    font-size: 0.85rem;
    margin-left: -0.25rem;
  }
  .ai-brief {
    flex: 1;
    min-width: 0;
    font-size: 0.78rem;
    font-family: inherit;
    resize: none;
    line-height: 1.4;
    min-height: 2.5rem;
  }
  .ai-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .ai-sep {
    width: 1px;
    align-self: stretch;
    background: var(--line);
    margin: 0 0.1rem;
  }
  .polish {
    border-color: var(--gold-deep);
    color: var(--gold);
    white-space: nowrap;
  }
  .polish:hover:not(:disabled) {
    background: var(--gold-glow);
  }
  .ai-ib {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--ash);
    white-space: nowrap;
  }
  .ai-num {
    width: 3rem;
    font-size: 0.72rem;
  }
  .revert:hover {
    color: var(--oxblood);
  }
  .ai-status {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }
  .ai-msg {
    color: var(--bone-dim);
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ai-msg.amber {
    color: var(--gold);
  }
  .pbar {
    width: 120px;
    height: 5px;
    border-radius: 999px;
    background: var(--ink-4);
    overflow: hidden;
    display: inline-block;
    flex: none;
  }
  .pbar > i {
    display: block;
    height: 100%;
    background: var(--gold);
    border-radius: 999px;
    transition: width 0.3s ease;
  }
  .spin {
    width: 12px;
    height: 12px;
    border-radius: 999px;
    border: 2px solid var(--gold-glow);
    border-top-color: var(--gold);
    animation: spin 0.8s linear infinite;
    flex: none;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
    .pbar > i {
      transition: none;
    }
  }
  .lower {
    height: 240px;
    flex: none;
    border-top: 1px solid var(--line);
  }
</style>
