<script lang="ts">
  /**
   * Attachment-swap ("part swapping") manager: pick any slotted part, add alternate looks
   * (AI-generate a region-masked variant, or import a PNG), preview them as thumbnails, and —
   * in the Animate context — drop a ready-to-retime swap onto the timeline. Self-contained: it
   * has its own part picker, so it isn't gated on selecting the exact bone.
   */
  import { commands, unwrap, type StudioDoc } from "$lib/ipc";

  let {
    gameId,
    assetKey,
    doc,
    applyDoc,
    preselect = null,
    onAnimate,
  }: {
    gameId: string;
    assetKey: string;
    doc: StudioDoc;
    applyDoc: (updated: StudioDoc) => void;
    preselect?: string | null;
    onAnimate?: (slotName: string) => void;
  } = $props();

  // A part can hold alternates when it owns a slot and isn't itself a swap-only alternate.
  const swappable = $derived(
    doc.parts.filter((p) => !p.attachmentOnly && doc.slots.some((s) => s.partId === p.id)),
  );
  let hostId = $state<string | null>(null);
  $effect(() => {
    if (!hostId || !swappable.some((p) => p.id === hostId)) {
      hostId =
        preselect && swappable.some((p) => p.id === preselect)
          ? preselect
          : (swappable[0]?.id ?? null);
    }
  });
  const hostSlot = $derived(hostId ? (doc.slots.find((s) => s.partId === hostId) ?? null) : null);
  const alternates = $derived(hostSlot?.alternates ?? []);

  let name = $state("");
  let prompt = $state("");
  let busy = $state(false);
  let error = $state("");
  let fileEl = $state<HTMLInputElement | null>(null);

  // Thumbnails (loaded once per alternate; requested set is non-reactive to avoid loops).
  let thumbs = $state<Record<string, string>>({});
  const requested = new Set<string>();
  $effect(() => {
    for (const a of alternates) {
      if (!requested.has(a)) {
        requested.add(a);
        unwrap(commands.studioGetImage(gameId, assetKey, `parts/${a}/cut.png`))
          .then((u) => (thumbs[a] = u))
          .catch(() => {});
      }
    }
  });

  async function generate() {
    const id = hostId;
    if (!id || !prompt.trim()) return;
    busy = true;
    error = "";
    try {
      const nm = name.trim() || prompt.trim().slice(0, 24);
      applyDoc(await unwrap(commands.studioGenerateAlternate(gameId, assetKey, id, nm, prompt.trim())));
      prompt = "";
      name = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
  async function importFile(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const file = el.files?.[0];
    el.value = "";
    const id = hostId;
    if (!file || !id) return;
    busy = true;
    error = "";
    try {
      const dataUrl = await new Promise<string>((res, rej) => {
        const r = new FileReader();
        r.onload = () => res(r.result as string);
        r.onerror = () => rej(r.error);
        r.readAsDataURL(file);
      });
      const nm = name.trim() || file.name.replace(/\.[^.]+$/, "");
      applyDoc(await unwrap(commands.studioAddAlternate(gameId, assetKey, id, nm, dataUrl)));
      name = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
  async function remove(alt: string) {
    const id = hostId;
    if (!id) return;
    busy = true;
    error = "";
    try {
      applyDoc(await unwrap(commands.studioRemoveAlternate(gameId, assetKey, id, alt)));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="swaps">
  <label class="pick">
    <span class="lbl">Swap art on</span>
    {#if swappable.length}
      <select class="dd mono" bind:value={hostId}>
        {#each swappable as p (p.id)}
          <option value={p.id}>{p.id}</option>
        {/each}
      </select>
    {:else}
      <p class="hint">No cut parts with a slot yet — cut &amp; rig a part first.</p>
    {/if}
  </label>

  {#if hostId}
    {#if alternates.length}
      <ul class="alts">
        {#each alternates as alt (alt)}
          <li class="alt">
            {#if thumbs[alt]}
              <img class="th" src={thumbs[alt]} alt={alt} />
            {:else}
              <span class="th ph"></span>
            {/if}
            <span class="mono nm">{alt}</span>
            <button class="ghost x" title="remove" disabled={busy} onclick={() => remove(alt)}>×</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="hint">
        No alternate looks yet. Add closed eyes, an open mouth, a lit gem… then key the swap (or
        let the AI blink it).
      </p>
    {/if}

    <div class="add">
      <input class="in" placeholder="name (optional)" bind:value={name} disabled={busy} />
      <div class="gen">
        <input
          class="in"
          placeholder="describe a variant — “eyes closed”"
          bind:value={prompt}
          disabled={busy}
          onkeydown={(e) => e.key === "Enter" && generate()}
        />
        <button class="ghost sm" onclick={generate} disabled={busy || !prompt.trim()}>
          {busy ? "…" : "Generate"}
        </button>
      </div>
      <button class="ghost sm" onclick={() => fileEl?.click()} disabled={busy}>Import PNG…</button>
      <input bind:this={fileEl} type="file" accept="image/png,image/webp" hidden onchange={importFile} />
    </div>

    {#if onAnimate && hostSlot}
      <button class="animate-btn" disabled={busy} onclick={() => onAnimate?.(hostSlot.name)}>
        ＋ Add swap to timeline
      </button>
      <p class="hint">
        Drops a base → {alternates[0] ?? "hidden"} → base blink on the current clip — retime the
        diamonds however you like.
      </p>
    {/if}

    {#if error}<p class="err">{error}</p>{/if}
    <p class="hint dim">
      <b>Generate</b> only repaints the described region so it lines up with the base (needs an
      OpenAI key). <b>Import</b> works offline.
    </p>
  {/if}
</div>

<style>
  .swaps {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
  }
  .pick {
    display: flex;
    flex-direction: column;
    gap: 0.28rem;
  }
  .lbl {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ash);
    font-weight: 600;
  }
  .dd {
    font-size: 0.75rem;
    color: var(--bone);
    background: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: var(--radius-sm, 6px);
    padding: 0.35rem 0.4rem;
    width: 100%;
  }
  .alts {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .alt {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.2rem 0.3rem;
    border-radius: var(--radius-sm, 6px);
    background: var(--wash);
  }
  .th {
    width: 28px;
    height: 28px;
    object-fit: contain;
    background: var(--ink-2);
    border-radius: 4px;
    flex: none;
  }
  .th.ph {
    display: inline-block;
  }
  .nm {
    flex: 1;
    font-size: 0.72rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .x {
    flex: none;
    color: var(--ash);
    background: transparent;
    border: none;
    padding: 0 0.25rem;
    font-size: 0.9rem;
  }
  .x:hover:not(:disabled) {
    color: var(--oxblood);
  }
  .add {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .gen {
    display: flex;
    gap: 0.3rem;
  }
  .in {
    flex: 1;
    min-width: 0;
    font-size: 0.72rem;
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm, 6px);
    color: var(--bone);
    padding: 0.3rem 0.45rem;
  }
  .sm {
    font-size: 0.7rem;
    padding: 0.2rem 0.55rem;
    white-space: nowrap;
  }
  .animate-btn {
    align-self: stretch;
    font-size: 0.75rem;
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--gold-deep);
    color: #17130a;
    background: var(--gold);
    border-radius: var(--radius-sm, 6px);
    font-weight: 600;
    margin-top: 0.15rem;
  }
  .animate-btn:disabled {
    opacity: 0.5;
  }
  .hint {
    font-size: 0.66rem;
    color: var(--ash);
    line-height: 1.45;
    margin: 0;
  }
  .hint.dim {
    color: var(--ash-deep);
  }
  .hint b {
    color: var(--bone-dim);
    font-weight: 600;
  }
  .err {
    font-size: 0.68rem;
    color: var(--oxblood);
    margin: 0;
  }
</style>
