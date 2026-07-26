<script lang="ts">
  /**
   * Cut mode: AI part proposals → SAM click-to-segment → manual brush correction →
   * cut parts from the ORIGINAL pixels. Left: parts ledger. Center: EditCanvas.
   * Right: segmentation controls for the selected part.
   */
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import {
    commands,
    unwrap,
    type Part,
    type SamPrompt,
    type StudioDoc,
  } from "$lib/ipc";
  import EditCanvas, { type Overlay } from "./EditCanvas.svelte";
  import PartsPanel from "./PartsPanel.svelte";

  let {
    gameId,
    assetKey,
    doc,
    save,
    oncut,
    ongorig,
  }: {
    gameId: string;
    assetKey: string;
    doc: StudioDoc;
    /** Persist a locally-edited doc; returns the saved doc. */
    save: (doc: StudioDoc) => Promise<StudioDoc>;
    /** Called with the rebuilt doc after a successful cut. */
    oncut: (doc: StudioDoc) => void;
    /** Navigate to the Rig tab (step 4 of the guided flow). */
    ongorig?: () => void;
  } = $props();

  const PALETTE = ["#d9a944", "#6fa7dd", "#7fc584", "#e0716a", "#a98abf", "#6fb3ab", "#c2955f", "#8a94a6"];

  let sourceUrl = $state<string | null>(null);
  let selectedId = $state<string | null>(null);
  let maskUrls = $state<Record<string, string>>({});
  let candidate = $state<{ url: string; area?: number } | null>(null);
  // "manual" = the user's own strokes (NEVER silently discarded — auto-applied on
  // part switch / leave); "proposal" = SAM/cloud suggestion (discarded unless applied).
  let candidateFrom = $state<"" | "manual" | "proposal">("");
  let prompts = $state<SamPrompt[]>([]);
  let tool = $state<"point" | "brush" | "erase" | "lasso" | "lassoErase">("point");
  let brushSize = $state(24);

  let samReady = $state<boolean | null>(null);
  let samModel = $state<string | null>(null);
  let downloading = $state(false);
  let dlProgress = $state(0);
  let segmenting = $state(false);
  let applying = $state(false);
  let cutting = $state(false);
  let autoMsg = $state("");
  let autoRunning = $state(false);
  // True while a snapshot from the last "AI select" is on disk — enables one-click revert.
  let autoUndoAvailable = $state(false);
  // The planned animation lives on the doc (motionBrief): described BEFORE cutting so
  // the AI partitions for the motion, and reused later as the AI clip brief.
  let motionTimer: ReturnType<typeof setTimeout> | null = null;
  function motionChanged() {
    if (motionTimer) clearTimeout(motionTimer);
    motionTimer = setTimeout(() => save($state.snapshot(doc) as StudioDoc), 600);
  }
  let error = $state("");

  // Inpainting (Phase 4).
  type InpaintState = "clear" | "pending" | "fresh" | "stale";
  let inpaintStates = $state<Record<string, InpaintState>>({});
  let inpainting = $state(false);
  let inpaintMsg = $state("");

  // AI FX layers (glows/flares on additive slots).
  let fxOpen = $state(false);
  let fxName = $state("");
  let fxBrief = $state("");
  let fxBusy = $state(false);

  async function generateFx() {
    if (!fxBrief.trim()) return;
    fxBusy = true;
    error = "";
    autoMsg = "Painting the light… (10–30 s)";
    try {
      const updated = await unwrap(
        commands.studioGenerateFx(gameId, assetKey, fxName.trim() || "glow", fxBrief.trim()),
      );
      Object.assign(doc, updated);
      doc.parts = [...doc.parts];
      const newest = doc.parts[doc.parts.length - 1];
      autoMsg = `FX layer "${newest.id}" added (additive, front-most) — reorder it in the ledger, animate its alpha in Animate.`;
      fxOpen = false;
      fxBrief = "";
      await refreshCutThumbs();
      selectPart(newest.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      autoMsg = "";
    } finally {
      fxBusy = false;
    }
  }

  // Cut verification: per-part texture thumbnails + isolate view of the shipped pixels.
  let cutThumbs = $state<Record<string, string>>({});
  let isolate = $state(false);
  let isolateUrl = $state<string | null>(null);

  /** Path of the texture that actually feeds the atlas for a part. */
  function texturePath(p: Part): string {
    return p.texture === "completed" && p.completedHash
      ? `parts/${p.id}/completed.${p.completedHash}.png`
      : `parts/${p.id}/cut.png`;
  }

  async function refreshCutThumbs() {
    for (const p of doc.parts) {
      if (!p.bbox) continue;
      const url = await unwrap(
        commands.studioGetImage(gameId, assetKey, texturePath(p)),
      ).catch(() => null);
      if (url) cutThumbs = { ...cutThumbs, [p.id]: url };
    }
  }

  // Compose the selected part's texture at its bbox onto a full-source-size canvas so the
  // isolate view lines up 1:1 with the normal view (same zoom/pan space).
  $effect(() => {
    const id = selectedId;
    const on = isolate;
    const part = doc.parts.find((p) => p.id === id);
    isolateUrl = null;
    if (!on || !part?.bbox) return;
    const bbox = (part.texture === "completed" && part.completedBbox) || part.bbox;
    const url = cutThumbs[part.id];
    if (!url) return;
    const img = new Image();
    img.onload = () => {
      const c = document.createElement("canvas");
      c.width = doc.source.width;
      c.height = doc.source.height;
      c.getContext("2d")!.drawImage(img, bbox.x, bbox.y);
      isolateUrl = c.toDataURL("image/png");
    };
    img.src = url;
  });

  let segTimer: ReturnType<typeof setTimeout> | null = null;

  // ── Guided flow: 1 Segment → 2 Cut → 3 Fill hidden → 4 Rig ──────────────────
  /** Parts whose mask changed after they were cut (session-local re-cut reminder). */
  let dirtySinceCut = $state(new Set<string>());
  // Filled-texture proof thumbs: keyed `${partId}:${completedHash}` so a redo refreshes.
  let filledThumbs = $state<Record<string, string>>({});
  $effect(() => {
    for (const p of doc.parts) {
      if (!p.completedHash) continue;
      const key = `${p.id}:${p.completedHash}`;
      if (filledThumbs[key]) continue;
      unwrap(commands.studioGetImage(gameId, assetKey, `parts/${p.id}/completed.${p.completedHash}.png`))
        .then((url) => (filledThumbs = { ...filledThumbs, [key]: url }))
        .catch(() => {});
    }
  });

  /** Tool switching + single-key shortcuts (V/B/E/L, ⇧L). */
  function setTool(next: typeof tool) {
    tool = next;
  }
  function onToolKey(e: KeyboardEvent) {
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT") return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "v") setTool("point");
    else if (k === "b") setTool("brush");
    else if (k === "e") setTool("erase");
    else if (k === "l") setTool(e.shiftKey ? "lassoErase" : "lasso");
    else return;
    e.preventDefault();
  }

  const maskedCount = $derived(doc.parts.filter((p) => p.maskHash).length);
  const cutCount = $derived(doc.parts.filter((p) => p.maskHash && p.bbox).length);
  const needsCut = $derived(
    maskedCount > 0 && (cutCount < maskedCount || dirtySinceCut.size > 0),
  );
  const inpaintPending = $derived(
    Object.values(inpaintStates).some((st) => st === "pending" || st === "stale"),
  );
  const selPart = $derived(doc.parts.find((p) => p.id === selectedId) ?? null);
  const isFxLayer = $derived(!!selPart?.bbox && !selPart?.maskHash && !!selectedId);
  /** Context guidance shown when nothing louder is happening. */
  const guideLine = $derived(
    !selectedId
      ? "Pick a part in the list — or run step 1 to let AI find them."
      : isFxLayer
        ? "Light layer — drag it in the list to sit behind or in front; animate its glow in Animate."
        : candidate
          ? "Adjust the selection with more clicks, then Apply."
          : !selPart?.maskHash
            ? `Click the ${selectedId} on the image — AI selects it. ⇧-click removes an area.`
            : "Selection wrong? Click the image to redo it, or use the canvas tools.",
  );
  const statusLine = $derived(inpaintMsg || autoMsg || guideLine);

  onMount(() => {
    (async () => {
      sourceUrl = await unwrap(commands.studioGetImage(gameId, assetKey, "source.png")).catch(
        (e) => {
          error = `Could not load the source image: ${e instanceof Error ? e.message : e}`;
          return null;
        },
      );
      const status = await unwrap(commands.studioSamStatus()).catch(() => null);
      samReady = status?.state === "ready";
      samModel = status?.state === "ready" ? status.model : null;
      // Load persisted masks.
      for (const p of doc.parts) {
        if (p.maskHash) loadMask(p.id);
      }
      const first = doc.parts.find((p) => p.id !== "all") ?? doc.parts[0];
      if (first) selectPart(first.id);
      autoUndoAvailable = await unwrap(
        commands.studioAutocutUndoAvailable(gameId, assetKey),
      ).catch(() => false);
      refreshInpaint();
      refreshCutThumbs();
    })();
    const un = listen<{ received: number; total: number }>("studio://sam-progress", (e) => {
      dlProgress = e.payload.total > 0 ? e.payload.received / e.payload.total : 0;
    });
    const unInpaint = listen<{ partId: string; state: string; message: string | null }>(
      "studio://inpaint-progress",
      (e) => {
        const { partId, state, message } = e.payload;
        inpaintMsg =
          state === "start"
            ? `Filling ${partId}… (10–30 s per part)`
            : state === "error"
              ? `${partId} failed: ${message ?? "unknown error"}`
              : `${partId}: ${state}`;
      },
    );
    return () => {
      un.then((f) => f());
      unInpaint.then((f) => f());
      if (segTimer) clearTimeout(segTimer);
    };
  });

  const anyCut = $derived(doc.parts.some((p) => p.bbox && p.maskHash));

  async function refreshInpaint() {
    if (!anyCut) return;
    const st = await unwrap(commands.studioInpaintStatus(gameId, assetKey)).catch(() => null);
    if (st) inpaintStates = Object.fromEntries(st.map((s) => [s.partId, s.state]));
  }

  async function inpaintAll() {
    inpainting = true;
    error = "";
    inpaintMsg = "Planning hidden areas…";
    try {
      const updated = await unwrap(commands.studioInpaintAll(gameId, assetKey));
      Object.assign(doc, updated);
      inpaintMsg = "Fill complete — hidden areas are painted in behind overlapping parts.";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      inpaintMsg = "";
    } finally {
      inpainting = false;
      await refreshInpaint();
      await refreshCutThumbs();
    }
  }

  async function inpaintSelected(force: boolean) {
    if (!selectedId) return;
    inpainting = true;
    error = "";
    try {
      const updated = await unwrap(commands.studioInpaintPart(gameId, assetKey, selectedId, force));
      Object.assign(doc, updated);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      inpainting = false;
      inpaintMsg = "";
      await refreshInpaint();
      await refreshCutThumbs();
    }
  }

  async function setTexture(useCompleted: boolean) {
    const part = doc.parts.find((p) => p.id === selectedId);
    if (!part) return;
    part.texture = useCompleted ? "completed" : "cut";
    await persist();
    await refreshCutThumbs();
  }

  async function loadMask(partId: string) {
    const url = await unwrap(
      commands.studioGetImage(gameId, assetKey, `parts/${partId}/mask.png`),
    ).catch(() => null);
    if (url) maskUrls = { ...maskUrls, [partId]: url };
  }

  /** Persist a mask to a part (shared by Apply, part-switch auto-apply, unmount flush). */
  async function commitMaskTo(partId: string, url: string, pts: typeof prompts) {
    const updated = await unwrap(
      commands.studioSetMask(gameId, assetKey, partId, url, $state.snapshot(pts)),
    );
    Object.assign(doc, updated);
    maskUrls = { ...maskUrls, [partId]: url };
    if (doc.parts.find((p) => p.id === partId)?.bbox) {
      dirtySinceCut = new Set([...dirtySinceCut, partId]);
    }
    refreshInpaint();
  }

  function selectPart(id: string) {
    // Manual strokes are real work — commit them before moving on. Proposals stay
    // opt-in and are dropped as before.
    if (candidate && candidateFrom === "manual" && selectedId && selectedId !== id) {
      const partId = selectedId;
      const url = candidate.url;
      const pts = prompts;
      commitMaskTo(partId, url, pts).catch((e) => {
        error = e instanceof Error ? e.message : String(e);
      });
    }
    selectedId = id;
    candidate = null;
    candidateFrom = "";
    prompts = doc.parts.find((p) => p.id === id)?.prompts ?? [];
  }

  const visibleParts = $derived(doc.parts.filter((p) => p.id !== "all" || doc.parts.length === 1));

  const overlays = $derived<Overlay[]>(
    visibleParts
      .map((p, i) => {
        const isSel = p.id === selectedId;
        const url = isSel && candidate ? candidate.url : maskUrls[p.id];
        return url
          ? { id: p.id, url, color: PALETTE[i % PALETTE.length], active: isSel }
          : null;
      })
      .filter((o): o is Overlay => o !== null),
  );

  const maskReady = $derived(
    Object.fromEntries(doc.parts.map((p) => [p.id, !!p.maskHash])) as Record<string, boolean>,
  );

  // ── SAM interaction ──────────────────────────────────────────────────────────
  function onpoint(x: number, y: number, positive: boolean) {
    if (!selectedId) return;
    prompts = [...prompts, { x, y, label: positive ? "positive" : "negative" }];
    scheduleSegment();
  }

  /** Delete a prompt dot: the clicked one, or the last (right-click empty = undo). */
  function onPointRemove(index: number) {
    if (!selectedId || !prompts.length) return;
    prompts = index >= 0 ? prompts.filter((_, i) => i !== index) : prompts.slice(0, -1);
    if (prompts.some((p) => p.label === "positive")) scheduleSegment();
    else candidate = null;
  }

  function scheduleSegment() {
    if (segTimer) clearTimeout(segTimer);
    segTimer = setTimeout(runSegment, 180);
  }

  async function runSegment() {
    if (!selectedId || !samReady || !prompts.some((p) => p.label === "positive")) return;
    segmenting = true;
    error = "";
    try {
      const r = await unwrap(commands.studioSegment(gameId, assetKey, $state.snapshot(prompts)));
      candidate = { url: r.maskDataUrl, area: r.area };
      candidateFrom = "proposal";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      segmenting = false;
    }
  }

  // Leaving Cut (tab switch / asset switch) must not lose manual strokes either.
  $effect(() => {
    return () => {
      if (candidate && candidateFrom === "manual" && selectedId) {
        commands.studioSetMask(gameId, assetKey, selectedId, candidate.url, $state.snapshot(prompts));
      }
    };
  });

  function resetPoints() {
    prompts = [];
    candidate = null;
  }

  async function applyMask() {
    if (!selectedId) return;
    const url = candidate?.url ?? maskUrls[selectedId];
    if (!url) return;
    applying = true;
    error = "";
    try {
      await commitMaskTo(selectedId, url, prompts);
      candidate = null;
      candidateFrom = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      applying = false;
    }
  }

  // ── Parts management (local doc edits, persisted via save()) ────────────────
  function newPart(name: string): Part {
    const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "part";
    let id = slug;
    let i = 2;
    while (doc.parts.some((p) => p.id === id)) id = `${slug}_${i++}`;
    return {
      id,
      name,
      prompts: [],
      bbox: null,
      maskHash: null,
      completedHash: null,
      texture: "cut",
    };
  }

  async function addPart(name: string) {
    const p = newPart(name);
    doc.parts = [...doc.parts.filter((x) => x.id !== "all" || x.maskHash), p];
    await persist();
    selectPart(p.id);
  }

  async function deletePart(id: string) {
    doc.parts = doc.parts.filter((p) => p.id !== id);
    if (selectedId === id) selectedId = null;
    await persist();
  }

  async function movePart(id: string, dir: -1 | 1) {
    const i = doc.parts.findIndex((p) => p.id === id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= doc.parts.length) return;
    const next = [...doc.parts];
    [next[i], next[j]] = [next[j], next[i]];
    doc.parts = next;
    // Draw order = parts order everywhere: keep the slots (built at cut time) in lockstep
    // so reordering after a cut takes effect immediately, without recutting.
    const order = new Map(doc.parts.map((p, idx) => [p.id, idx]));
    doc.slots = [...doc.slots].sort(
      (a, b) => (order.get(a.partId) ?? 999) - (order.get(b.partId) ?? 999),
    );
    await persist();
    refreshInpaint(); // occlusion zones depend on what's above → staleness may flip
  }

  async function persist() {
    try {
      const saved = await save($state.snapshot(doc) as StudioDoc);
      Object.assign(doc, saved);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  // ── AI auto-cut (region-first) ───────────────────────────────────────────────
  // SAM segments the whole image into candidate regions (real boundaries, no coordinate
  // guessing), then the vision model only LABELS them into named parts.
  async function autoSegment() {
    if (doc.parts.some((p) => p.maskHash) &&
        !confirm("This replaces all existing parts and their selections. Continue?")) {
      return;
    }
    autoRunning = true;
    error = "";
    try {
      autoMsg = "Discovering regions with SAM, then labeling with AI… (~30 s)";
      // Flush the motion brief to disk first — the backend reads the doc from there.
      if (motionTimer) clearTimeout(motionTimer);
      await save($state.snapshot(doc) as StudioDoc);
      const updated = await unwrap(commands.studioAutoCut(gameId, assetKey, assetKey));
      Object.assign(doc, updated);
      maskUrls = {};
      for (const p of doc.parts) {
        if (p.maskHash) loadMask(p.id);
      }
      autoMsg = "Check each part's tint, then Cut.";
      autoUndoAvailable = true;
      dirtySinceCut = new Set();
      const first = doc.parts[0];
      if (first) selectPart(first.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      autoMsg = "";
    } finally {
      autoRunning = false;
    }
  }

  // Revert to the parts snapshotted right before the last "AI select" — the safety net so
  // a disappointing auto-cut never costs hand-selected masks.
  async function undoAutoCut() {
    autoRunning = true;
    error = "";
    try {
      const updated = await unwrap(commands.studioUndoAutoCut(gameId, assetKey));
      Object.assign(doc, updated);
      doc.parts = [...doc.parts];
      maskUrls = {};
      for (const p of doc.parts) {
        if (p.maskHash) loadMask(p.id);
      }
      cutThumbs = {};
      await refreshCutThumbs();
      autoUndoAvailable = false;
      autoMsg = "Reverted to the parts from before the last AI select.";
      dirtySinceCut = new Set();
      const first = doc.parts.find((p) => p.id !== "all") ?? doc.parts[0];
      if (first) selectPart(first.id);
      oncut(updated); // keep Rig/Animate in sync with the restored doc
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      autoRunning = false;
    }
  }

  // Cloud "paint-out" cut: the model repaints everything except the selected part as
  // flat magenta; keying it yields the mask. Paid + slower than SAM, but semantic.
  let cloudBusy = $state(false);
  async function cloudCut() {
    if (!selectedId) return;
    cloudBusy = true;
    error = "";
    autoMsg = "Cloud paint-out: repainting everything except this part… (~20 s, uses OpenAI)";
    try {
      const r = await unwrap(commands.studioCloudCut(gameId, assetKey, selectedId));
      candidate = { url: r.maskDataUrl, area: r.area };
      candidateFrom = "proposal";
      autoMsg = "Check the tint, then Apply selection.";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      autoMsg = "";
    } finally {
      cloudBusy = false;
    }
  }

  // ── Cloud cut ALL parts: one paint-out call per empty part, in parallel; results
  //    auto-apply (doc writes serialized — they must not race). Parts that already
  //    have a selection are left alone; re-cut those individually.
  let cloudAllBusy = $state(false);
  async function cloudCutAll() {
    const targets = doc.parts.filter((p) => !maskUrls[p.id]);
    if (!targets.length) {
      autoMsg = "Every part already has a selection — cloud-cut a single part to redo one.";
      return;
    }
    cloudAllBusy = true;
    error = "";
    autoMsg = `☁ Cloud paint-out ×${targets.length} — one call per part, in parallel… (uses OpenAI)`;
    const results = await Promise.all(
      targets.map(async (part) => ({ part, r: await commands.studioCloudCut(gameId, assetKey, part.id) })),
    );
    let done = 0;
    const fails: string[] = [];
    for (const { part, r } of results) {
      if (r.status !== "ok") {
        fails.push(part.name || part.id);
        continue;
      }
      try {
        const updated = await unwrap(
          commands.studioSetMask(gameId, assetKey, part.id, r.data.maskDataUrl, []),
        );
        Object.assign(doc, updated);
        maskUrls = { ...maskUrls, [part.id]: r.data.maskDataUrl };
        if (part.bbox) dirtySinceCut = new Set([...dirtySinceCut, part.id]);
        done++;
      } catch {
        fails.push(part.name || part.id);
      }
    }
    refreshInpaint();
    autoMsg = fails.length
      ? `${done} selections filled · failed: ${fails.join(", ")} — cloud-cut those individually.`
      : `${done} selections filled — check each part's tint, then Cut parts.`;
    cloudAllBusy = false;
  }

  async function cutParts() {
    cutting = true;
    error = "";
    try {
      const updated = await unwrap(commands.studioCutParts(gameId, assetKey));
      oncut(updated);
      dirtySinceCut = new Set();
      autoMsg = "";
      await refreshInpaint();
      await refreshCutThumbs();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      cutting = false;
    }
  }

  async function downloadSam(hq = true) {
    downloading = true;
    error = "";
    try {
      const st = await unwrap(commands.studioSamDownload(hq));
      samReady = true;
      samModel = st.state === "ready" ? st.model : null;
      autoMsg = hq ? "HQ model ready — selections will be sharper from now on." : "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      downloading = false;
    }
  }
</script>

<svelte:window onkeydown={onToolKey} />

<div class="cut">
  <aside class="left">
    <div class="panel-top">
      <textarea
        class="motion-input"
        rows="2"
        value={doc.motionBrief ?? ""}
        oninput={(e) => {
          doc.motionBrief = e.currentTarget.value;
          motionChanged();
        }}
        placeholder="planned animation — “the cables sway, the glow pulses” — the cut follows it"
        spellcheck="false"
        disabled={autoRunning}
        title="describe the motion BEFORE cutting: every element that moves on its own becomes its own part; also seeds the AI clip brief in Animate"
      ></textarea>
      <button
        class="gold"
        disabled={autoRunning || cutting || inpainting || downloading}
        onclick={() => (samReady === false ? downloadSam(true) : autoSegment())}
      >
        {samReady === false
          ? downloading
            ? "Downloading model…"
            : "Get AI model (375 MB)"
          : autoRunning
            ? "Selecting…"
            : maskedCount > 0
              ? "✦ Re-select parts"
              : "✦ Select parts (AI)"}
      </button>
      {#if doc.parts.length && doc.parts.some((p) => !maskUrls[p.id])}
        <button
          disabled={cloudAllBusy || cloudBusy || autoRunning || cutting}
          onclick={cloudCutAll}
          title="one cloud paint-out call PER empty part, run in parallel — the model isolates each named part semantically; parts that already have a selection are skipped (~20 s, uses OpenAI per part)"
        >
          {cloudAllBusy
            ? "☁ Cutting all…"
            : `☁ ✦ Cloud cut all (${doc.parts.filter((p) => !maskUrls[p.id]).length})`}
        </button>
      {/if}
      {#if autoUndoAvailable}
        <button
          class="ghost undo-auto"
          disabled={autoRunning || cutting || inpainting}
          onclick={undoAutoCut}
          title="restore the parts you had before the last AI select"
        >
          ↩ Undo AI select
        </button>
      {/if}
    </div>
    <PartsPanel
      parts={visibleParts}
      {selectedId}
      {maskReady}
      thumbs={cutThumbs}
      onselect={selectPart}
      onadd={addPart}
      ondelete={deletePart}
      onmove={movePart}
    />
    {#if selPart?.bbox && selPart?.maskHash}
      {@const ist = inpaintStates[selectedId ?? ""] ?? "clear"}
      <div class="inpaint-box">
        <span class="u-label">Behind this part</span>
        <p class="muted tiny">
          {ist === "clear"
            ? "Nothing overlaps it — no fill needed."
            : ist === "fresh"
              ? "Hidden areas are filled in."
              : ist === "stale"
                ? "Selections changed — fill it again."
                : "Partly covered — fill so no hole shows when it moves."}
        </p>
        {#if ist !== "clear"}
          <button onclick={() => inpaintSelected(ist === "fresh")} disabled={inpainting}>
            {inpainting ? "Working…" : ist === "fresh" ? "Redo fill" : "Fill hidden areas"}
          </button>
        {/if}
        {#if selectedId && selPart.completedHash && filledThumbs[`${selectedId}:${selPart.completedHash}`]}
          <div class="fill-pair">
            <figure>
              {#if cutThumbs[selectedId]}<img src={cutThumbs[selectedId]} alt="cut" />{/if}
              <figcaption class="tiny muted">cut</figcaption>
            </figure>
            <figure>
              <img src={filledThumbs[`${selectedId}:${selPart.completedHash}`]} alt="filled" />
              <figcaption class="tiny muted">filled</figcaption>
            </figure>
          </div>
        {/if}
        {#if selPart.completedHash}
          <label class="tex-toggle">
            <input
              type="checkbox"
              checked={selPart.texture === "completed"}
              onchange={(e) => setTexture(e.currentTarget.checked)}
            />
            <span class="tiny">use filled-in texture</span>
          </label>
        {/if}
      </div>
    {/if}
    <div class="left-actions">
      <button
        disabled={maskedCount === 0 || cutting || autoRunning || inpainting}
        title={maskedCount === 0 ? "needs part selections first" : "split the artwork along the selections — parts reassemble pixel-identical"}
        onclick={cutParts}
      >
        {cutting ? "Cutting…" : needsCut && cutCount > 0 ? "Re-cut parts" : "Cut parts"}
      </button>
      <button
        disabled={!anyCut || needsCut || inpainting || cutting || autoRunning}
        title={!anyCut
          ? "needs cut parts first"
          : needsCut
            ? "selections changed — re-cut first"
            : "AI-fill the bands hidden behind overlapping parts"}
        onclick={inpaintAll}
      >
        {inpainting ? "Filling…" : "✦ Fill hidden"}
      </button>
      {#if fxOpen}
        <form
          class="fx-form"
          onsubmit={(e) => {
            e.preventDefault();
            generateFx();
          }}
        >
          <input class="mono" bind:value={fxName} placeholder="layer name (e.g. ember_glow)" />
          <textarea
            bind:value={fxBrief}
            rows="3"
            placeholder="describe the light… e.g. “warm fire glow seeping from inside the ribcage”"
          ></textarea>
          <div class="fx-row">
            <button type="submit" disabled={fxBusy || !fxBrief.trim()}>
              {fxBusy ? "Painting…" : "Generate"}
            </button>
            <button type="button" class="ghost" onclick={() => (fxOpen = false)}>Cancel</button>
          </div>
        </form>
      {:else}
        <button class="ghost fx-open" onclick={() => (fxOpen = true)} disabled={fxBusy}>
          ✦ Add light layer
        </button>
      {/if}
    </div>
  </aside>

  <section class="center">
    {#if downloading}
      <div class="statusline"><progress value={dlProgress}></progress></div>
    {:else if error}
      <div class="statusline err">{error}</div>
    {:else if statusLine}
      <div class="statusline">{statusLine}</div>
    {:else if samModel === "tiny"}
      <div class="statusline">
        <button class="ghost hq-link" onclick={() => downloadSam(true)}>
          ✦ Sharper selections available — get the HQ model (375 MB, one-time)
        </button>
      </div>
    {/if}
    <div class="canvas-holder" class:iso={isolate && isolateUrl}>
      <div class="tool-rail" role="toolbar" aria-label="selection tools">
        <button
          class="trail-btn"
          class:on={tool === "point"}
          title="AI points (V) — click to select; click a dot to delete it; right-click = undo last; ⇧-click = remove area"
          onclick={() => setTool("point")}
        >◎</button>
        <span class="trail-sep"></span>
        <button class="trail-btn mono" class:on={tool === "brush"} title="Brush (B) — paint the selection" onclick={() => setTool("brush")}>B</button>
        <button class="trail-btn mono" class:on={tool === "erase"} title="Erase (E) — unpaint the selection" onclick={() => setTool("erase")}>E</button>
        <span class="trail-sep"></span>
        <button class="trail-btn mono" class:on={tool === "lasso"} title="Lasso add (L) — click corners around an area; right-click = undo point" onclick={() => setTool("lasso")}>L+</button>
        <button class="trail-btn mono" class:on={tool === "lassoErase"} title="Lasso remove (⇧L)" onclick={() => setTool("lassoErase")}>L−</button>
        {#if tool === "brush" || tool === "erase"}
          <span class="trail-sep"></span>
          <div class="trail-size" title="brush size">
            <input type="range" min="4" max="120" bind:value={brushSize} />
            <span class="mono tiny">{brushSize}</span>
          </div>
        {/if}
      </div>

      {#if selectedId && !isFxLayer}
        <div class="action-bar">
          <span class="ab-name mono">{selectedId}</span>
          <button
            class="ghost ab-btn"
            onclick={cloudCut}
            disabled={cloudBusy || autoRunning || applying}
            title="an image model repaints everything except this part as flat magenta; keying it gives the selection — paid, ~20 s"
          >
            {cloudBusy ? "☁ Painting…" : "☁ Cloud cut"}
          </button>
          <label class="ab-iso" title="view only this part's shipped pixels">
            <input type="checkbox" bind:checked={isolate} />
            <span class="tiny">alone</span>
          </label>
          {#if candidate || prompts.length}
            <span class="ab-sep"></span>
            <button class="ghost ab-btn" onclick={resetPoints}>Reset</button>
            <button class="gold ab-btn" onclick={applyMask} disabled={applying || !candidate}>
              {applying ? "Applying…" : "Apply"}
            </button>
          {/if}
        </div>
      {/if}
      <EditCanvas
        imageUrl={isolate && isolateUrl ? isolateUrl : sourceUrl}
        width={doc.source.width}
        height={doc.source.height}
        overlays={isolate ? [] : overlays}
        prompts={!isolate && selectedId ? prompts : []}
        {tool}
        {brushSize}
        {onpoint}
        onpointremove={onPointRemove}
        onmaskedit={(url) => {
          candidate = { url };
          candidateFrom = "manual";
        }}
      />
      {#if segmenting}<span class="seg-badge mono">selecting…</span>{/if}
      {#if isolate && isolateUrl}
        <span class="iso-badge mono">isolated: {selectedId} — the exact pixels that ship</span>
      {/if}
    </div>
  </section>

</div>

<style>
  .cut {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr 220px;
    min-height: 0;
  }
  /* ── Canvas tool rail: every selection tool, always visible, keyboard-mapped ── */
  .tool-rail {
    position: absolute;
    top: 0.7rem;
    left: 0.7rem;
    z-index: 5;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
    padding: 0.35rem;
    background: var(--ink, rgba(16, 17, 21, 0.92));
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.35);
  }
  .trail-btn {
    width: 2.1rem;
    height: 2.1rem;
    display: grid;
    place-items: center;
    padding: 0;
    font-size: 0.8rem;
    color: var(--ash-deep);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
  }
  .trail-btn:hover {
    color: var(--bone);
    background: var(--wash);
  }
  .trail-btn.on {
    color: var(--gold);
    border-color: var(--gold-deep);
    background: var(--gold-glow);
  }
  .trail-sep {
    width: 1.4rem;
    height: 1px;
    background: var(--line);
    margin: 0.15rem 0;
  }
  .trail-size {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.15rem;
    padding: 0.2rem 0;
  }
  .trail-size input {
    width: 2.4rem;
  }
  /* Floating selection actions: live with the canvas they act on. */
  .action-bar {
    position: absolute;
    bottom: var(--space-4);
    left: 50%;
    transform: translateX(-50%);
    z-index: var(--z-rail);
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-4);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--elev-2);
    max-width: calc(100% - 2 * var(--space-6));
  }
  .ab-name {
    font-size: var(--text-xs);
    color: var(--ash);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 10rem;
  }
  .ab-btn {
    font-size: var(--text-sm);
    padding: var(--space-2) var(--space-4);
  }
  .ab-iso {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--ash);
  }
  .ab-sep {
    width: 1px;
    align-self: stretch;
    background: var(--line);
  }
  /* Fill-hidden box docked in the left rail under the parts list. */
  .inpaint-box {
    border-top: 1px solid var(--line);
    padding: var(--space-4) var(--space-4) 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .fill-pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.4rem;
  }
  .fill-pair figure {
    margin: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.15rem;
  }
  .fill-pair img {
    width: 100%;
    aspect-ratio: 1;
    object-fit: contain;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background:
      repeating-conic-gradient(var(--checker-a) 0% 25%, var(--checker-b) 0% 50%) 0 0 / 10px 10px;
  }
  /* Parts dock on the RIGHT — tools left on the canvas, inventory right,
     the classic editor split. */
  .left {
    order: 2;
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .center {
    order: 1;
  }
  .left :global(.panel) {
    flex: 1;
  }
  .left-actions {
    padding: 0.8rem;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .fx-open {
    font-size: 0.72rem;
    width: 100%;
  }
  .undo-auto {
    font-size: 0.72rem;
    width: 100%;
  }
  .fx-form {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .fx-form input,
  .fx-form textarea {
    font-size: 0.72rem;
    width: 100%;
    resize: vertical;
  }
  .fx-row {
    display: flex;
    gap: 0.4rem;
  }
  .fx-row button {
    flex: 1;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .panel-top {
    padding: var(--space-4);
    border-bottom: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .motion-input {
    resize: vertical;
    font-size: var(--text-sm);
    line-height: 1.4;
  }
  .statusline {
    padding: 0.35rem 1rem;
    font-size: 0.72rem;
    color: var(--lapis);
    border-bottom: 1px solid var(--line);
    display: flex;
    align-items: center;
  }
  .statusline.err {
    color: var(--oxblood);
  }
  .hq-link {
    font-size: 0.72rem;
    padding: 0;
    color: var(--gold);
  }
  .statusline progress {
    flex: 1;
    max-width: 300px;
  }
  .canvas-holder {
    flex: 1;
    position: relative;
    min-height: 0;
    overflow: hidden;
  }
  .seg-badge {
    position: absolute;
    top: 0.6rem;
    right: 0.8rem;
    font-size: 0.65rem;
    color: var(--gold);
    background: rgba(12, 13, 16, 0.7);
    padding: 0.2rem 0.55rem;
    border-radius: 999px;
    border: 1px solid var(--line);
  }
  .iso-badge {
    position: absolute;
    top: 0.6rem;
    left: 0.8rem;
    font-size: 0.65rem;
    color: var(--lapis);
    background: rgba(12, 13, 16, 0.7);
    padding: 0.2rem 0.55rem;
    border-radius: 999px;
    border: 1px solid var(--line);
    pointer-events: none;
  }
  /* Checkerboard behind the isolated texture so alpha edges are readable. */
  .canvas-holder.iso :global(canvas) {
    background:
      repeating-conic-gradient(var(--wash) 0% 25%, transparent 0% 50%)
      0 0 / 22px 22px;
  }

  .tex-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--bone-dim);
  }
</style>
