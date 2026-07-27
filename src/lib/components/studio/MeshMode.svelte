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

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let rebuildTimer: ReturnType<typeof setTimeout> | null = null;
  let densityTimer: ReturnType<typeof setTimeout> | null = null;

  const deformableParts = $derived(doc.parts.filter((p) => p.deformable));

  const hasMesh = (partId: string) => {
    const a = doc.slots.find((s) => s.partId === partId)?.attachment;
    return !!a && typeof a === "object" && "mesh" in a;
  };

  onMount(() => {
    (async () => {
      sourceUrl = await unwrap(commands.studioGetImage(gameId, assetKey, "source.png")).catch(
        () => null,
      );
      const first = deformableParts[0];
      if (first) selectPart(first.id);
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
</script>

<div class="mesh">
  <aside class="left">
    <span class="u-label">Deformable parts</span>
    {#if deformableParts.length}
      <ul>
        {#each deformableParts as p (p.id)}
          <li class:sel={p.id === selectedId}>
            <button class="row" onclick={() => selectPart(p.id)}>
              <span class="wave">∿</span>
              <span class="pname mono">{p.id}</span>
              <span class="muted tiny" class:dim={!hasMesh(p.id)}>
                {hasMesh(p.id) ? "mesh" : "no mesh"}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="muted small empty">
        No deformable parts yet. In the <b>Cut</b> tab, click the ∿ on a part (hair, cloak,
        cloth) to mark it deformable, then run auto-rig in <b>Rig</b>.
      </p>
    {/if}
  </aside>

  <section class="center">
    <nav class="subtabs">
      <button class="stab" class:on={centerTab === "mesh"} onclick={() => (centerTab = "mesh")}>Mesh</button>
      <button class="stab" class:on={centerTab === "watch"} onclick={() => (centerTab = "watch")}>Watch</button>
      <span class="hint muted tiny">
        {centerTab === "mesh"
          ? "vertices are tinted by the bone that moves them"
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
        <SpinePreview {bundle} clip={WIGGLE} playing loop />
      {/if}
    </div>
  </section>

  <aside class="right">
    <div class="inspector">
      {#if selectedPart}
        <span class="u-label">Part</span>
        <h3 class="mono sel-name">∿ {selectedPart.id}</h3>
        <button class="ghost tiny-btn" onclick={makeRigid}>Make rigid</button>

        {#if !hasChain}
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
          <label class="field">
            <span class="muted tiny">
              detail — {density} cells · {selectedMesh.triangles.length / 3} triangles
            </span>
            <input
              type="range"
              min="3"
              max="16"
              step="1"
              value={density}
              disabled={meshBusy}
              oninput={(e) => onDensity(+e.currentTarget.value)}
            />
          </label>
        {/if}

        <span class="grp u-label">Sway</span>
        <label class="toggle">
          <input type="checkbox" checked={swayOn} onchange={(e) => toggleSway(e.currentTarget.checked)} />
          <span class="tiny">physics sway ⚡ {chainPhysics.length ? `· ${chainPhysics.length} bones` : ""}</span>
        </label>
        {#if swayOn && sway}
          <div class="grid2">
            <label class="field">
              <span class="muted tiny">stiffness</span>
              <input type="number" min="1" max="300" step="5" value={sway.strength ?? 60}
                oninput={(e) => patchSway({ strength: +e.currentTarget.value })} />
            </label>
            <label class="field">
              <span class="muted tiny">damping</span>
              <input type="number" min="0.3" max="1" step="0.05" value={sway.damping ?? 0.85}
                oninput={(e) => patchSway({ damping: +e.currentTarget.value })} />
            </label>
            <label class="field">
              <span class="muted tiny">inertia</span>
              <input type="number" min="0" max="1" step="0.05" value={sway.inertia ?? 0.5}
                oninput={(e) => patchSway({ inertia: +e.currentTarget.value })} />
            </label>
            <label class="field">
              <span class="muted tiny">gravity</span>
              <input type="number" step="5" value={sway.gravity ?? 0}
                oninput={(e) => patchSway({ gravity: +e.currentTarget.value })} />
            </label>
          </div>
          <p class="muted tiny">Runtime-simulated — flip to <b>Watch</b> to see it move.</p>
        {/if}
      {:else}
        <p class="muted small">Select a deformable part on the left.</p>
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
