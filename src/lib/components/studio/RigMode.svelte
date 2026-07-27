<script lang="ts">
  /**
   * Rig mode: auto-rig proposal (AI tree + Rust geometry), bone gizmo editing on the
   * canvas, numeric inspector escape hatch, and the wiggle test — a generated clip that
   * rotates each bone ±10° in sequence so hinge quality is visible before any timeline
   * work exists. The wiggle preview plays through the real runtime, like everything else.
   */
  import { onMount } from "svelte";
  import {
    commands,
    unwrap,
    type Bone,
    type Clip,
    type PreviewBundle,
    type StudioDoc,
  } from "$lib/ipc";
  import EditCanvas, { type GizmoBone } from "./EditCanvas.svelte";
  import BoneTree from "./BoneTree.svelte";
  import PoseOverlay from "./PoseOverlay.svelte";
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

  let sourceUrl = $state<string | null>(null);
  let selected = $state<string | null>(null);
  let rigging = $state(false);
  let error = $state("");
  let bundle = $state<PreviewBundle | null>(null);
  let wiggle = $state(true);
  let rebuilding = $state(false);
  /** Center view: edit joints on the flat art, or pose-test the live skeleton. */
  let centerTab = $state<"joints" | "pose">("joints");
  let posePreview = $state<ReturnType<typeof SpinePreview> | null>(null);

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let rebuildTimer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    (async () => {
      sourceUrl = await unwrap(commands.studioGetImage(gameId, assetKey, "source.png")).catch(
        () => null,
      );
      rebuildPreview();
    })();
    return () => {
      if (saveTimer) clearTimeout(saveTimer);
      if (rebuildTimer) clearTimeout(rebuildTimer);
    };
  });

  const gizmoBones = $derived<GizmoBone[]>(
    doc.bones.map((b) => ({
      name: b.name,
      parent: b.parent ?? null,
      x: b.x ?? 0,
      y: b.y ?? 0,
      rotation: b.rotation ?? 0,
      length: b.length ?? 0,
      selected: b.name === selected,
    })),
  );

  const selectedBone = $derived(doc.bones.find((b) => b.name === selected) ?? null);

  /** Parent options for the selected bone: any bone that isn't itself or a descendant. */
  const parentOptions = $derived.by(() => {
    if (!selectedBone || !selectedBone.parent) return [];
    const descendants = new Set<string>([selectedBone.name]);
    let grew = true;
    while (grew) {
      grew = false;
      for (const b of doc.bones) {
        if (b.parent && descendants.has(b.parent) && !descendants.has(b.name)) {
          descendants.add(b.name);
          grew = true;
        }
      }
    }
    return doc.bones.filter((b) => !descendants.has(b.name)).map((b) => b.name);
  });

  // ── Editing ──────────────────────────────────────────────────────────────────
  function patchBone(name: string, patch: Partial<Pick<Bone, "x" | "y" | "rotation" | "length">>) {
    const bone = doc.bones.find((b) => b.name === name);
    if (!bone) return;
    Object.assign(bone, patch);
    doc.bones = [...doc.bones];
  }

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

  function reparent(name: string, parent: string) {
    const bone = doc.bones.find((b) => b.name === name);
    if (!bone || !bone.parent) return; // root can't reparent
    bone.parent = parent;
    // Keep topological order: parents before children.
    const order: Bone[] = [];
    const place = (p: string | null) => {
      for (const b of doc.bones) {
        if ((b.parent ?? null) === p && !order.includes(b)) {
          order.push(b);
          place(b.name);
        }
      }
    };
    place(null);
    for (const b of doc.bones) if (!order.includes(b)) order.push(b);
    doc.bones = order;
    commit();
  }

  // ── Chain bones ──────────────────────────────────────────────────────────────
  const slotBound = $derived(new Set(doc.slots.map((s) => s.bone)));

  /** Add a child bone at the selected bone's tip (a chain segment). */
  function addChildBone() {
    const parent = selectedBone;
    if (!parent) return;
    const base = `${parent.name}_seg`;
    let n = 2;
    let name = `${base}${n}`;
    while (doc.bones.some((b) => b.name === name)) name = `${base}${++n}`;
    const rot = ((parent.rotation ?? 0) * Math.PI) / 180;
    const len = Math.max(parent.length ?? 0, 40);
    const child: Bone = {
      name,
      parent: parent.name,
      x: (parent.x ?? 0) + Math.cos(rot) * len,
      y: (parent.y ?? 0) - Math.sin(rot) * len,
      rotation: parent.rotation ?? 0,
      length: Math.max((parent.length ?? 0) * 0.8, 40),
      scaleX: 1,
      scaleY: 1,
    };
    const idx = doc.bones.findIndex((b) => b.name === parent.name);
    doc.bones.splice(idx + 1, 0, child); // right after the parent keeps topo order
    doc.bones = [...doc.bones];
    selected = name;
    commit();
  }

  /** Delete a chain bone (never root or a part's slot bone); children reparent up. */
  function deleteBone() {
    const bone = selectedBone;
    if (!bone || !bone.parent || slotBound.has(bone.name)) return;
    for (const b of doc.bones) {
      if (b.parent === bone.name) b.parent = bone.parent;
    }
    doc.bones = doc.bones.filter((b) => b.name !== bone.name);
    selected = bone.parent;
    commit();
  }

  // ── Physics sway (Spine 4.2 — the runtime simulates it, no keyframes) ────────
  const bonePhysics = $derived((doc.physics ?? []).find((p) => p.bone === selected) ?? null);

  function togglePhysics(on: boolean) {
    if (!selected) return;
    const list = doc.physics ?? [];
    doc.physics = on
      ? [
          ...list,
          { bone: selected, rotate: 1, inertia: 0.5, strength: 60, damping: 0.85, wind: 0, gravity: 0, mix: 1 },
        ]
      : list.filter((p) => p.bone !== selected);
    commit();
  }

  function patchPhysics(patch: Partial<Omit<NonNullable<typeof bonePhysics>, "bone">>) {
    if (!bonePhysics) return;
    Object.assign(bonePhysics, patch);
    doc.physics = [...(doc.physics ?? [])];
    commit();
  }

  // Deformable mesh toggle (Phase 7): only for bones that carry a part slot.
  const selectedSlot = $derived(doc.slots.find((s) => s.bone === selected) ?? null);
  const meshEnabled = $derived(
    !!selectedSlot && typeof selectedSlot.attachment === "object" && "mesh" in selectedSlot.attachment,
  );
  /** The selected part's mesh (for the wireframe overlay), or null when it's a rigid region. */
  const selectedMesh = $derived.by(() => {
    const a = selectedSlot?.attachment;
    return a && typeof a === "object" && "mesh" in a ? a.mesh : null;
  });
  const selectedPart = $derived(
    selectedSlot ? (doc.parts.find((p) => p.id === selectedSlot.partId) ?? null) : null,
  );
  let meshBusy = $state(false);

  async function toggleMesh(enabled: boolean) {
    if (!selectedSlot) return;
    meshBusy = true;
    error = "";
    try {
      const updated = await unwrap(
        commands.studioSetMesh(gameId, assetKey, selectedSlot.partId, enabled, 0),
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

  // ── Attachment swap (alternate art on this slot) ─────────────────────────────
  let altBusy = $state(false);
  let altPrompt = $state("");
  let altName = $state("");
  let altFileEl = $state<HTMLInputElement | null>(null);

  // Load a thumbnail for each alternate so misalignment is caught at a glance. The requested set
  // is plain (non-reactive) so writing a thumbnail never re-triggers this loader.
  let altThumbs = $state<Record<string, string>>({});
  const altRequested = new Set<string>();
  $effect(() => {
    for (const a of selectedSlot?.alternates ?? []) {
      if (!altRequested.has(a)) {
        altRequested.add(a);
        unwrap(commands.studioGetImage(gameId, assetKey, `parts/${a}/cut.png`))
          .then((url) => (altThumbs[a] = url))
          .catch(() => {});
      }
    }
  });

  function applyAlt(updated: StudioDoc) {
    Object.assign(doc, updated);
    doc.slots = [...doc.slots];
    doc.parts = [...doc.parts];
    scheduleRebuild();
  }
  async function generateAlt() {
    if (!selectedPart || !altPrompt.trim()) return;
    altBusy = true;
    error = "";
    try {
      const nm = altName.trim() || altPrompt.trim().slice(0, 24);
      applyAlt(
        await unwrap(
          commands.studioGenerateAlternate(gameId, assetKey, selectedPart.id, nm, altPrompt.trim()),
        ),
      );
      altPrompt = "";
      altName = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      altBusy = false;
    }
  }
  async function importAlt(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const file = el.files?.[0];
    el.value = "";
    if (!file || !selectedPart) return;
    altBusy = true;
    error = "";
    try {
      const dataUrl = await new Promise<string>((res, rej) => {
        const r = new FileReader();
        r.onload = () => res(r.result as string);
        r.onerror = () => rej(r.error);
        r.readAsDataURL(file);
      });
      const nm = altName.trim() || file.name.replace(/\.[^.]+$/, "");
      applyAlt(await unwrap(commands.studioAddAlternate(gameId, assetKey, selectedPart.id, nm, dataUrl)));
      altName = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      altBusy = false;
    }
  }
  async function removeAlt(altId: string) {
    if (!selectedPart) return;
    altBusy = true;
    error = "";
    try {
      applyAlt(await unwrap(commands.studioRemoveAlternate(gameId, assetKey, selectedPart.id, altId)));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      altBusy = false;
    }
  }

  async function autoRig() {
    rigging = true;
    error = "";
    try {
      const updated = await unwrap(commands.studioAutoRig(gameId, assetKey));
      Object.assign(doc, updated);
      doc.bones = [...doc.bones];
      selected = null;
      scheduleRebuild();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      rigging = false;
    }
  }

  // ── Wiggle preview ───────────────────────────────────────────────────────────
  const WIGGLE = "__wiggle";

  function wiggleClip(): Clip {
    const targets = doc.bones.filter((b) => b.parent);
    const per = 0.7;
    return {
      id: WIGGLE,
      name: WIGGLE,
      duration: Math.max(targets.length * per, per),
      looping: true,
      timelines: targets.map((b, i) => ({
        target: { boneRotate: b.name },
        keys: [
          { time: i * per, v: [0], curve: "linear" as const },
          { time: i * per + per * 0.25, v: [10], curve: "linear" as const },
          { time: i * per + per * 0.75, v: [-10], curve: "linear" as const },
          { time: (i + 1) * per, v: [0], curve: "linear" as const },
        ],
      })),
    };
  }

  function scheduleRebuild() {
    if (rebuildTimer) clearTimeout(rebuildTimer);
    rebuildTimer = setTimeout(rebuildPreview, 350);
  }

  async function rebuildPreview() {
    rebuilding = true;
    try {
      const snap = $state.snapshot(doc) as StudioDoc;
      snap.clips = [...snap.clips, wiggleClip()];
      bundle = await unwrap(commands.studioPreviewBundle(gameId, assetKey, snap));
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      rebuilding = false;
    }
  }

  const previewClip = $derived(wiggle ? WIGGLE : (doc.clips[0]?.name ?? null));

  function num(v: number | null | undefined): number {
    return Math.round(((v ?? 0) + Number.EPSILON) * 10) / 10;
  }
</script>

<div class="rig">
  <aside class="left">
    <BoneTree bones={doc.bones} {selected} onselect={(n) => (selected = n)} />
    <div class="left-actions">
      <button onclick={autoRig} disabled={rigging}>
        {rigging ? "Rigging…" : "✦ Auto-rig (AI)"}
      </button>
    </div>
  </aside>

  <section class="center">
    {#if error}<div class="banner err">{error}</div>{/if}
    <nav class="center-tabs">
      <button class="ct" class:on={centerTab === "joints"} onclick={() => (centerTab = "joints")}>
        Joints
      </button>
      <button class="ct" class:on={centerTab === "pose"} onclick={() => (centerTab = "pose")}>
        Test pose
      </button>
      {#if centerTab === "pose"}
        <span class="muted tiny">drag tip = rotate · ⌘-drag = move · nothing is saved</span>
      {/if}
    </nav>
    <div class="canvas-holder">
      {#if centerTab === "joints"}
        <EditCanvas
          imageUrl={sourceUrl}
          width={doc.source.width}
          height={doc.source.height}
          tool="bones"
          bones={gizmoBones}
          mesh={selectedMesh}
          onboneselect={(n) => (selected = n)}
          onbonechange={patchBone}
          onbonecommit={commit}
        />
      {:else}
        <SpinePreview
          bind:this={posePreview}
          {bundle}
          clip={null}
          playing={false}
          loop={false}
          onstatus={(s) => {
            if (s.error) error = `pose stage: ${s.error}`;
          }}
        />
        <PoseOverlay preview={posePreview} />
      {/if}
    </div>
  </section>

  <aside class="right">
    <div class="inspector">
      <span class="u-label">Bone</span>
      {#if selectedBone}
        <h3 class="mono sel-name">{selectedBone.name}</h3>

        <span class="grp u-label">Transform</span>
        <div class="grid2">
          <label class="field">
            <span class="muted tiny">x</span>
            <input
              type="number"
              value={num(selectedBone.x)}
              oninput={(e) => {
                patchBone(selectedBone!.name, { x: +e.currentTarget.value });
                commit();
              }}
            />
          </label>
          <label class="field">
            <span class="muted tiny">y</span>
            <input
              type="number"
              value={num(selectedBone.y)}
              oninput={(e) => {
                patchBone(selectedBone!.name, { y: +e.currentTarget.value });
                commit();
              }}
            />
          </label>
          <label class="field">
            <span class="muted tiny">rotation°</span>
            <input
              type="number"
              value={num(selectedBone.rotation)}
              oninput={(e) => {
                patchBone(selectedBone!.name, { rotation: +e.currentTarget.value });
                commit();
              }}
            />
          </label>
          <label class="field">
            <span class="muted tiny">length</span>
            <input
              type="number"
              min="0"
              value={num(selectedBone.length)}
              oninput={(e) => {
                patchBone(selectedBone!.name, { length: Math.max(+e.currentTarget.value, 0) });
                commit();
              }}
            />
          </label>
        </div>

        <span class="grp u-label">Hierarchy</span>
        {#if selectedBone.parent}
          <label class="field">
            <span class="muted tiny">parent</span>
            <select
              value={selectedBone.parent}
              onchange={(e) => reparent(selectedBone!.name, e.currentTarget.value)}
            >
              {#each parentOptions as p (p)}
                <option value={p}>{p}</option>
              {/each}
            </select>
          </label>
        {/if}
        <div class="bone-ops">
          <button class="ghost" onclick={addChildBone} title="Add a child bone to extend the chain">
            + Child bone
          </button>
          {#if selectedBone.parent && !slotBound.has(selectedBone.name)}
            <button class="ghost del-bone" onclick={deleteBone}>Delete</button>
          {/if}
        </div>

        {#if selectedBone.parent}
          <span class="grp u-label">Motion</span>
          <label class="phys-toggle">
            <input
              type="checkbox"
              checked={!!bonePhysics}
              onchange={(e) => togglePhysics(e.currentTarget.checked)}
            />
            <span class="tiny">physics sway ⚡</span>
          </label>
          {#if bonePhysics}
            <div class="grid2 phys-grid">
              <label class="field">
                <span class="muted tiny">stiffness</span>
                <input
                  type="number"
                  min="1"
                  max="300"
                  step="5"
                  value={bonePhysics.strength ?? 60}
                  oninput={(e) => patchPhysics({ strength: +e.currentTarget.value })}
                />
              </label>
              <label class="field">
                <span class="muted tiny">damping</span>
                <input
                  type="number"
                  min="0.3"
                  max="1"
                  step="0.05"
                  value={bonePhysics.damping ?? 0.85}
                  oninput={(e) => patchPhysics({ damping: +e.currentTarget.value })}
                />
              </label>
              <label class="field">
                <span class="muted tiny">inertia</span>
                <input
                  type="number"
                  min="0"
                  max="1"
                  step="0.05"
                  value={bonePhysics.inertia ?? 0.5}
                  oninput={(e) => patchPhysics({ inertia: +e.currentTarget.value })}
                />
              </label>
              <label class="field">
                <span class="muted tiny">gravity</span>
                <input
                  type="number"
                  step="5"
                  value={bonePhysics.gravity ?? 0}
                  oninput={(e) => patchPhysics({ gravity: +e.currentTarget.value })}
                />
              </label>
            </div>
            <p class="muted tiny">
              Runtime-simulated — no keyframes. Try Test pose: yank a parent bone and watch
              this one react.
            </p>
          {/if}
        {/if}
        {#if selectedSlot}
          <span class="grp u-label">Appearance</span>
          <label class="field">
            <span class="muted tiny">blend</span>
            <select
              value={selectedSlot.blend ?? "normal"}
              onchange={(e) => {
                selectedSlot!.blend = e.currentTarget.value as NonNullable<typeof selectedSlot>["blend"];
                doc.slots = [...doc.slots];
                commit();
              }}
            >
              <option value="normal">normal</option>
              <option value="additive">additive (light)</option>
              <option value="multiply">multiply</option>
              <option value="screen">screen</option>
            </select>
          </label>
          <label class="mesh-toggle">
            <input
              type="checkbox"
              checked={meshEnabled}
              disabled={meshBusy}
              onchange={(e) => toggleMesh(e.currentTarget.checked)}
            />
            <span class="tiny">deformable mesh {meshBusy ? "…" : ""}</span>
          </label>
          {#if selectedMesh}
            <p class="muted tiny mesh-stat">
              ∿ {selectedMesh.triangles.length / 3} triangles — vertices on the canvas are
              tinted by the bone that moves them.
            </p>
          {/if}
          <p class="muted tiny">
            {selectedPart?.deformable
              ? "Marked deformable — auto-rigged as a waving mesh on its bone chain. "
              : "Auto-weighted grid mesh — bends at the joint and along any chain bones. "}
            Re-tick after re-cutting, filling hidden areas, or changing this part's bones.
          </p>

          <span class="grp u-label">Attachments (swap)</span>
          {#if selectedSlot.alternates?.length}
            <ul class="alts">
              {#each selectedSlot.alternates as alt (alt)}
                <li class="alt">
                  {#if altThumbs[alt]}
                    <img class="althumb" src={altThumbs[alt]} alt={alt} />
                  {:else}
                    <span class="althumb ph"></span>
                  {/if}
                  <span class="mono tiny aname">{alt}</span>
                  <button class="ghost x" title="remove" disabled={altBusy} onclick={() => removeAlt(alt)}>×</button>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="muted tiny">
              No alternates. Add closed eyes, a mouth shape, a lit gem… then key the swap (or a
              blink) in <b>Animate</b>. A slot can also just hide/show with no alternate.
            </p>
          {/if}
          <div class="alt-add">
            <input
              class="alt-in"
              placeholder="name (optional)"
              bind:value={altName}
              disabled={altBusy}
            />
            <div class="alt-gen">
              <input
                class="alt-in"
                placeholder="describe a variant…"
                bind:value={altPrompt}
                disabled={altBusy}
                onkeydown={(e) => e.key === "Enter" && generateAlt()}
              />
              <button class="ghost tiny-btn" disabled={altBusy || !altPrompt.trim()} onclick={generateAlt}>
                {altBusy ? "…" : "Generate"}
              </button>
            </div>
            <button class="ghost tiny-btn" disabled={altBusy} onclick={() => altFileEl?.click()}>
              Import PNG…
            </button>
            <input
              bind:this={altFileEl}
              type="file"
              accept="image/png,image/webp"
              hidden
              onchange={importAlt}
            />
          </div>
        {/if}
      {:else}
        <p class="muted small">Select a bone in the canvas or tree.</p>
      {/if}
    </div>

    <div class="preview-card">
      <div class="pc-head">
        <span class="u-label">Preview</span>
        <label class="wiggle-toggle">
          <input type="checkbox" bind:checked={wiggle} />
          <span class="tiny">wiggle test</span>
        </label>
      </div>
      <div class="pc-stage">
        <SpinePreview {bundle} clip={previewClip} playing loop />
        {#if rebuilding}<span class="pc-badge mono">building…</span>{/if}
      </div>
    </div>
  </aside>
</div>

<style>
  .rig {
    flex: 1;
    display: grid;
    grid-template-columns: 210px 1fr 250px;
    min-height: 0;
  }
  .left {
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .left :global(.tree) {
    flex: 1;
  }
  .left-actions {
    padding: 0.8rem;
    border-top: 1px solid var(--line);
  }
  .left-actions button {
    width: 100%;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .banner {
    padding: 0.45rem 1rem;
    font-size: 0.75rem;
    border-bottom: 1px solid var(--line);
  }
  .banner.err {
    color: var(--oxblood);
  }
  .center-tabs {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    padding: 0.35rem 0.9rem;
    border-bottom: 1px solid var(--line);
  }
  .ct {
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: transparent;
    padding: 0.15rem 0.05rem 0.35rem;
    color: var(--ash);
    font-size: 0.72rem;
  }
  .ct:hover:not(:disabled) {
    color: var(--bone-dim);
    background: transparent;
  }
  .ct.on {
    color: var(--bone);
    border-bottom-color: var(--gold);
  }
  .canvas-holder {
    flex: 1;
    position: relative;
    min-height: 0;
  }
  .bone-ops {
    display: flex;
    gap: 0.5rem;
  }
  .bone-ops button {
    flex: 1;
    font-size: 0.7rem;
  }
  .del-bone:hover {
    color: var(--oxblood);
  }
  .right {
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .inspector {
    padding: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
  }
  .sel-name {
    margin: 0;
    font-size: 0.9rem;
    color: var(--bone);
  }
  .grp {
    display: block;
    margin-top: 0.9rem;
    padding-top: 0.6rem;
    border-top: 1px solid var(--line);
    color: var(--ash-deep);
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
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
  }
  .mesh-toggle,
  .phys-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--bone-dim);
    margin-top: 0.3rem;
  }
  .phys-grid input {
    font-size: 0.72rem;
  }
  .tiny-btn {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    align-self: flex-start;
  }
  .alts {
    list-style: none;
    margin: 0.3rem 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .althumb {
    width: 26px;
    height: 26px;
    object-fit: contain;
    background: var(--ink-2, rgba(16, 17, 21, 0.6));
    border-radius: 3px;
    flex: none;
  }
  .althumb.ph {
    display: inline-block;
  }
  .alt {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.15rem 0.35rem;
    border-radius: var(--radius-sm);
    background: var(--wash);
  }
  .aname {
    flex: 1;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .alt .x {
    color: var(--oxblood);
    background: transparent;
    border: none;
    padding: 0 0.2rem;
    font-size: 0.85rem;
    flex: none;
  }
  .alt-add {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    margin-top: 0.3rem;
  }
  .alt-gen {
    display: flex;
    gap: 0.3rem;
  }
  .alt-in {
    flex: 1;
    font-size: 0.72rem;
    min-width: 0;
  }
  .preview-card {
    margin-top: auto;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    height: 260px;
    flex: none;
  }
  .pc-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.6rem 0.9rem 0.2rem;
  }
  .wiggle-toggle {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    color: var(--bone-dim);
  }
  .pc-stage {
    flex: 1;
    position: relative;
    min-height: 0;
  }
  .pc-badge {
    position: absolute;
    top: 0.4rem;
    right: 0.6rem;
    font-size: 0.62rem;
    color: var(--gold);
  }
</style>
