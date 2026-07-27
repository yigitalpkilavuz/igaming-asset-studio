<script lang="ts">
  /**
   * Mesh mode: review + tune the DEFORMABLE parts (hair, cloak, tail, cloth). Each is a
   * mesh on a bone chain with sway physics, auto-built at rig time. Here you see the
   * weighted wireframe (vertices tinted by the bone that moves them), adjust grid density
   * and sway, and watch it move. No weight painting — the auto-weights do that.
   */
  import { onMount } from "svelte";
  import { commands, unwrap, type Clip, type PreviewBundle, type StudioDoc } from "$lib/ipc";
  import EditCanvas, { type GizmoBone } from "./EditCanvas.svelte";
  import SpinePreview from "./SpinePreview.svelte";
  import Slider from "./Slider.svelte";

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

  const WIGGLE = "__mesh_wiggle__";

  let sourceUrl = $state<string | null>(null);
  let selectedId = $state<string | null>(null);
  let error = $state("");
  let bundle = $state<PreviewBundle | null>(null);
  let centerTab = $state<"mesh" | "watch">("mesh");
  let meshBusy = $state(false);
  let density = $state(8);

  // 2.5D depth turn.
  const TURN_CLIP = "turn";
  let watchClip = $state(WIGGLE);
  let depthReady = $state<boolean | null>(null);
  let turnBusy = $state(false);
  let turn = $state({
    yawAmp: 22,
    pitchAmp: 6,
    depth: 0.12,
    lens: 2.5,
    cycles: 1,
    duration: 2.5,
    edgePin: 0.5,
  });

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let rebuildTimer: ReturnType<typeof setTimeout> | null = null;
  let densityTimer: ReturnType<typeof setTimeout> | null = null;

  const deformableParts = $derived(doc.parts.filter((p) => p.deformable));
  // Every rigged part (has its own slot) — any can take a mesh + a 2.5D turn; deformable ones
  // additionally carry a sway chain. Alternate (attachment-only) parts are excluded.
  const parts = $derived(
    doc.parts.filter((p) => !p.attachmentOnly && doc.slots.some((s) => s.partId === p.id)),
  );

  const hasMesh = (partId: string) => {
    const a = doc.slots.find((s) => s.partId === partId)?.attachment;
    return !!a && typeof a === "object" && "mesh" in a;
  };

  onMount(() => {
    (async () => {
      sourceUrl = await unwrap(commands.studioGetImage(gameId, assetKey, "source.png")).catch(
        () => null,
      );
      const first = parts[0];
      if (first) selectPart(first.id);
      depthReady =
        (
          await unwrap(commands.layersDepthStatus()).catch(() => ({ state: "missing" as const }))
        ).state === "ready";
      rebuildPreview();
    })();
    return () => {
      for (const t of [saveTimer, rebuildTimer, densityTimer]) if (t) clearTimeout(t);
    };
  });

  const selectedPart = $derived(
    selectedId ? (doc.parts.find((p) => p.id === selectedId) ?? null) : null,
  );
  const selectedSlot = $derived(
    selectedId ? (doc.slots.find((s) => s.partId === selectedId) ?? null) : null,
  );
  const selectedMesh = $derived.by(() => {
    const a = selectedSlot?.attachment;
    return a && typeof a === "object" && "mesh" in a ? a.mesh : null;
  });
  const meshEnabled = $derived(!!selectedMesh);

  // The selected part's bone chain: `part.id` (root) + `part.id_seg*` (segments).
  const chainBones = $derived.by(() => {
    if (!selectedId) return [];
    const pre = `${selectedId}_seg`;
    return doc.bones.filter((b) => b.name === selectedId || b.name.startsWith(pre));
  });
  const chainNames = $derived(new Set(chainBones.map((b) => b.name)));
  const hasChain = $derived(chainBones.length > 1);

  // Sway physics on the chain (uniform across its segments).
  const chainPhysics = $derived((doc.physics ?? []).filter((p) => chainNames.has(p.bone)));
  const swayOn = $derived(chainPhysics.length > 0);
  const sway = $derived(chainPhysics[0] ?? null);

  const gizmoBones = $derived<GizmoBone[]>(
    doc.bones.map((b) => ({
      name: b.name,
      parent: b.parent ?? null,
      x: b.x ?? 0,
      y: b.y ?? 0,
      rotation: b.rotation ?? 0,
      length: b.length ?? 0,
      selected: chainNames.has(b.name), // highlight the selected part's chain
    })),
  );

  function selectPart(id: string) {
    selectedId = id;
    // Reflect the current mesh density in the slider (estimate from triangle count).
    const a = doc.slots.find((s) => s.partId === id)?.attachment;
    const m = a && typeof a === "object" && "mesh" in a ? a.mesh : null;
    density = m
      ? Math.min(16, Math.max(3, Math.round(Math.sqrt(m.triangles.length / 3 / 2))))
      : 8;
  }

  function selectByBone(name: string) {
    const p = deformableParts.find((q) => name === q.id || name.startsWith(`${q.id}_seg`));
    if (p) selectPart(p.id);
  }

  // ── Persistence + preview ──────────────────────────────────────────────────────
  function commit() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await save($state.snapshot(doc) as StudioDoc);
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }, 500);
    scheduleRebuild();
  }
  function scheduleRebuild() {
    if (rebuildTimer) clearTimeout(rebuildTimer);
    rebuildTimer = setTimeout(rebuildPreview, 350);
  }
  async function rebuildPreview() {
    try {
      const snap = $state.snapshot(doc) as StudioDoc;
      snap.clips = [...snap.clips, wiggleClip()];
      bundle = await unwrap(commands.studioPreviewBundle(gameId, assetKey, snap));
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }
  /** Rock the body anchor so the physics chains visibly sway in the Watch view. */
  function wiggleClip(): Clip {
    const anchor = doc.bones.find((b) => b.parent === "root") ?? doc.bones.find((b) => b.parent);
    return {
      id: WIGGLE,
      name: WIGGLE,
      duration: 1.6,
      looping: true,
      timelines: anchor
        ? [
            {
              target: { boneRotate: anchor.name },
              keys: [
                { time: 0, v: [0], curve: "linear" as const },
                { time: 0.4, v: [10], curve: "linear" as const },
                { time: 1.2, v: [-10], curve: "linear" as const },
                { time: 1.6, v: [0], curve: "linear" as const },
              ],
            },
          ]
        : [],
    };
  }

  // ── Mesh density ──────────────────────────────────────────────────────────────
  async function setMesh(enabled: boolean, cells: number) {
    if (!selectedId) return;
    meshBusy = true;
    error = "";
    try {
      const updated = await unwrap(
        commands.studioSetMesh(gameId, assetKey, selectedId, enabled, cells),
      );
      Object.assign(doc, updated);
      doc.slots = [...doc.slots];
      scheduleRebuild();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      meshBusy = false;
    }
  }
  function onDensity(cells: number) {
    density = cells;
    if (densityTimer) clearTimeout(densityTimer);
    densityTimer = setTimeout(() => setMesh(true, cells), 400);
  }

  // ── Sway ────────────────────────────────────────────────────────────────────────
  function toggleSway(on: boolean) {
    const segs = chainBones.filter((b) => b.name !== selectedId).map((b) => b.name);
    const targets = segs.length ? segs : chainBones.map((b) => b.name);
    const list = doc.physics ?? [];
    doc.physics = on
      ? [
          ...list,
          ...targets
            .filter((n) => !list.some((p) => p.bone === n))
            .map((n) => ({
              bone: n,
              rotate: 1,
              inertia: 0.5,
              strength: 60,
              damping: 0.85,
              wind: 0,
              gravity: 0,
              mix: 1,
            })),
        ]
      : list.filter((p) => !chainNames.has(p.bone));
    commit();
  }
  function patchSway(patch: Record<string, number>) {
    for (const p of doc.physics ?? []) if (chainNames.has(p.bone)) Object.assign(p, patch);
    doc.physics = [...(doc.physics ?? [])];
    commit();
  }

  function makeRigid() {
    if (!selectedPart) return;
    selectedPart.deformable = false;
    doc.parts = [...doc.parts];
    commit();
  }

  // ── 2.5D depth turn ──────────────────────────────────────────────────────────────
  async function downloadDepth() {
    turnBusy = true;
    error = "";
    try {
      const s = await unwrap(commands.layersDepthDownload());
      depthReady = s.state === "ready";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      turnBusy = false;
    }
  }
  async function bakeTurn() {
    if (!selectedId) return;
    turnBusy = true;
    error = "";
    try {
      const updated = await unwrap(
        commands.studioBakeTurn(
          gameId,
          assetKey,
          selectedId,
          TURN_CLIP,
          $state.snapshot(turn),
          density,
        ),
      );
      Object.assign(doc, updated);
      doc.slots = [...doc.slots];
      doc.clips = [...doc.clips];
      watchClip = TURN_CLIP;
      centerTab = "watch";
      await rebuildPreview();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      turnBusy = false;
    }
  }
</script>

<div class="mesh">
  <aside class="left">
    <span class="u-label">Parts</span>
    {#if parts.length}
      <ul>
        {#each parts as p (p.id)}
          <li class:sel={p.id === selectedId}>
            <button class="row" onclick={() => selectPart(p.id)}>
              <span class="wave" class:rigid={!p.deformable}>{p.deformable ? "∿" : "◆"}</span>
              <span class="pname mono">{p.id}</span>
              <span class="muted tiny" class:dim={!hasMesh(p.id)}>
                {hasMesh(p.id) ? "mesh" : "—"}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="muted small empty">
        No cut parts yet. Cut a part in the <b>Cut</b> tab first. Mark hair/cloak/cloth
        deformable (∿) for a sway chain; any part can take a 2.5D turn.
      </p>
    {/if}
  </aside>

  <section class="center">
    <nav class="subtabs">
      <button class="stab" class:on={centerTab === "mesh"} onclick={() => (centerTab = "mesh")}>Mesh</button>
      <button class="stab" class:on={centerTab === "watch"} onclick={() => (centerTab = "watch")}>Watch</button>
      {#if centerTab === "watch"}
        <button class="stab mini" class:on={watchClip === WIGGLE} onclick={() => (watchClip = WIGGLE)}>sway</button>
        {#if doc.clips.some((c) => c.name === TURN_CLIP)}
          <button class="stab mini" class:on={watchClip === TURN_CLIP} onclick={() => (watchClip = TURN_CLIP)}>turn</button>
        {/if}
      {/if}
      <span class="hint muted tiny">
        {centerTab === "mesh"
          ? "vertices are tinted by the bone that moves them"
          : watchClip === TURN_CLIP
            ? "2.5D depth turn — tune it on the right"
            : "the body rocks so the chains sway — tune it on the right"}
      </span>
    </nav>
    <div class="canvas-holder">
      {#if error}<div class="err-strip">{error}</div>{/if}
      {#if centerTab === "mesh"}
        <EditCanvas
          imageUrl={sourceUrl}
          width={doc.source.width}
          height={doc.source.height}
          tool="bones"
          bones={gizmoBones}
          mesh={selectedMesh}
          onboneselect={selectByBone}
        />
      {:else}
        <SpinePreview {bundle} clip={watchClip} playing loop onstatus={(s) => (error = s.error ?? "")} />
      {/if}
    </div>
  </section>

  <aside class="right">
    <div class="inspector">
      {#if selectedPart}
        <span class="u-label">Part</span>
        <h3 class="mono sel-name" class:rigid-name={!selectedPart.deformable}>
          {selectedPart.deformable ? "∿" : "◆"} {selectedPart.id}
        </h3>
        {#if selectedPart.deformable}
          <button class="ghost tiny-btn" onclick={makeRigid}>Make rigid</button>
        {/if}

        {#if selectedPart.deformable && !hasChain}
          <p class="muted tiny warn">
            No bone chain yet — run auto-rig in the <b>Rig</b> tab to build this part's chain
            and mesh.
          </p>
        {/if}

        <span class="grp u-label">Mesh</span>
        <label class="toggle">
          <input
            type="checkbox"
            checked={meshEnabled}
            disabled={meshBusy}
            onchange={(e) => setMesh(e.currentTarget.checked, density)}
          />
          <span class="tiny">deformable mesh {meshBusy ? "…" : ""}</span>
        </label>
        {#if meshEnabled && selectedMesh}
          <Slider
            label="detail"
            value={density}
            min={3}
            max={16}
            step={1}
            decimals={0}
            suffix=" cells"
            disabled={meshBusy}
            oninput={onDensity}
          />
          <span class="muted tiny">{selectedMesh.triangles.length / 3} triangles</span>
        {/if}

        {#if selectedPart.deformable}
        <span class="grp u-label">Sway</span>
        <label class="toggle">
          <input type="checkbox" checked={swayOn} onchange={(e) => toggleSway(e.currentTarget.checked)} />
          <span class="tiny">physics sway ⚡ {chainPhysics.length ? `· ${chainPhysics.length} bones` : ""}</span>
        </label>
        {#if swayOn && sway}
          <div class="grid2">
            <Slider label="stiffness" value={sway.strength ?? 60} min={1} max={300} step={5} decimals={0}
              oninput={(v) => patchSway({ strength: v })} />
            <Slider label="damping" value={sway.damping ?? 0.85} min={0.3} max={1} step={0.05} decimals={2}
              oninput={(v) => patchSway({ damping: v })} />
            <Slider label="inertia" value={sway.inertia ?? 0.5} min={0} max={1} step={0.05} decimals={2}
              oninput={(v) => patchSway({ inertia: v })} />
            <Slider label="gravity" value={sway.gravity ?? 0} min={-40} max={40} step={5} decimals={0}
              oninput={(v) => patchSway({ gravity: v })} />
          </div>
          <p class="muted tiny">Runtime-simulated — flip to <b>Watch</b> to see it move.</p>
        {/if}
        {/if}

        <span class="grp u-label">Depth / Turn (2.5D)</span>
        {#if depthReady === false}
          <p class="muted tiny warn">Depth model isn't downloaded yet (~190 MB).</p>
          <button class="ghost tiny-btn" onclick={downloadDepth} disabled={turnBusy}>
            {turnBusy ? "downloading…" : "Download depth model"}
          </button>
        {:else}
          <p class="muted tiny">
            Give this part volume: a looping turn baked from an estimated relief map, played as a
            mesh deform.
          </p>
          <div class="grid2">
            <Slider label="yaw" value={turn.yawAmp} min={0} max={60} step={1} decimals={0} suffix="°" oninput={(v) => (turn.yawAmp = v)} />
            <Slider label="pitch" value={turn.pitchAmp} min={0} max={45} step={1} decimals={0} suffix="°" oninput={(v) => (turn.pitchAmp = v)} />
            <Slider label="depth" value={turn.depth} min={0} max={0.4} step={0.02} decimals={2} oninput={(v) => (turn.depth = v)} />
            <Slider label="lens" value={turn.lens} min={1} max={8} step={0.5} decimals={1} oninput={(v) => (turn.lens = v)} />
            <Slider label="cycles" value={turn.cycles} min={1} max={4} step={1} decimals={0} oninput={(v) => (turn.cycles = v)} />
            <Slider label="seconds" value={turn.duration} min={0.5} max={8} step={0.5} decimals={1} suffix="s" oninput={(v) => (turn.duration = v)} />
            <Slider label="edge pin" value={turn.edgePin} min={0} max={1} step={0.1} decimals={2} amber oninput={(v) => (turn.edgePin = v)} />
          </div>
          <button class="ghost tiny-btn" onclick={bakeTurn} disabled={turnBusy || !selectedId}>
            {turnBusy ? "baking…" : "Generate turn"}
          </button>
          <p class="muted tiny">
            Replaces this part's mesh with an unweighted turn mesh. Watch it in
            <b>Watch ▸ turn</b>.
          </p>
        {/if}
      {:else}
        <p class="muted small">Select a part on the left.</p>
      {/if}
    </div>
  </aside>
</div>

<style>
  .mesh {
    flex: 1;
    display: grid;
    grid-template-columns: 200px 1fr 244px;
    min-height: 0;
  }
  .left {
    border-right: 1px solid var(--line);
    padding: 0.9rem;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .left ul {
    list-style: none;
    margin: 0.3rem 0 0;
    padding: 0;
  }
  .left li {
    border-radius: var(--radius-sm);
  }
  .left li.sel {
    background: var(--wash);
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: none;
    padding: 0.35rem 0.5rem;
    text-align: left;
    min-width: 0;
  }
  .wave {
    color: var(--lapis);
    font-size: 0.95rem;
    flex: none;
  }
  .wave.rigid {
    color: var(--ash);
    font-size: 0.7rem;
  }
  .rigid-name {
    color: var(--bone);
  }
  .stab.mini {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
  }
  .pname {
    flex: 1;
    font-size: 0.75rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .left li.sel .pname {
    color: var(--bone);
  }
  .dim {
    color: var(--ash-deep);
  }
  .empty {
    margin-top: 0.6rem;
    line-height: 1.5;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .subtabs {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.7rem;
    border-bottom: 1px solid var(--line);
  }
  .stab {
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    padding: 0.15rem 0.6rem;
    color: var(--ash);
    font-size: 0.75rem;
  }
  .stab.on {
    color: var(--bone);
    border-color: var(--line);
    background: var(--wash);
  }
  .hint {
    margin-left: 0.4rem;
  }
  .canvas-holder {
    flex: 1;
    position: relative;
    min-height: 0;
    overflow: hidden;
  }
  .err-strip {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 5;
    padding: 0.35rem 0.8rem;
    font-size: 0.72rem;
    color: var(--oxblood);
    background: var(--ink-2, rgba(16, 17, 21, 0.9));
    border-bottom: 1px solid var(--line);
  }
  .right {
    border-left: 1px solid var(--line);
    overflow-y: auto;
  }
  .inspector {
    padding: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .sel-name {
    margin: 0.1rem 0 0.2rem;
    font-size: 0.9rem;
    color: var(--lapis);
  }
  .tiny-btn {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    align-self: flex-start;
  }
  .grp {
    margin-top: 0.6rem;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--bone-dim);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .field input[type="range"] {
    width: 100%;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.4rem;
  }
  .grid2 input {
    width: 100%;
    font-size: 0.75rem;
  }
  .warn {
    color: var(--gold);
    line-height: 1.4;
  }
</style>
