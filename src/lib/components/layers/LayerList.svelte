<script lang="ts">
  /**
   * The depth-layer rail, back → front. Row 0 is the catch-all backdrop (no selection
   * needed, can't move or be deleted). Status chips share the studio's fill vocabulary:
   * clear · pending · fresh · stale.
   */
  import type { InpaintState, Layer } from "$lib/ipc";

  let {
    layers,
    selectedId,
    maskReady,
    fillStates,
    busy,
    onselect,
    onname,
    onmove,
    ondelete,
    onadd,
    onrefill,
  }: {
    layers: Layer[];
    selectedId: string | null;
    /** layer id → has a persisted selection. */
    maskReady: Record<string, boolean>;
    fillStates: Record<string, InpaintState>;
    busy: boolean;
    onselect: (id: string) => void;
    onname: (id: string, name: string) => void;
    onmove: (id: string, dir: -1 | 1) => void;
    ondelete: (id: string) => void;
    onadd: (name: string) => void;
    onrefill: (id: string) => void;
  } = $props();

  let adding = $state(false);
  let newName = $state("");

  function submitAdd() {
    const n = newName.trim();
    if (n) onadd(n);
    newName = "";
    adding = false;
  }

  function stateChip(l: Layer, i: number): { label: string; cls: string } {
    if (i > 0 && !maskReady[l.id]) return { label: "no selection", cls: "dim" };
    if (!l.bbox) return { label: i === 0 || maskReady[l.id] ? "not cut" : "—", cls: "dim" };
    const fs = fillStates[l.id];
    if (fs === "fresh") return { label: "filled", cls: "ok" };
    if (fs === "stale") return { label: "fill stale", cls: "warn" };
    if (fs === "pending") return { label: "needs fill", cls: "mid" };
    return { label: "cut", cls: "ok" }; // clear — nothing covers it
  }
</script>

<div class="rail">
  <div class="head">
    <span class="u-label">Layers</span>
    <button class="ghost tiny-btn" onclick={() => (adding = true)} disabled={busy}>+ Add</button>
  </div>
  <p class="muted order-hint">farthest → nearest</p>

  {#if adding}
    <form
      class="addrow"
      onsubmit={(e) => {
        e.preventDefault();
        submitAdd();
      }}
    >
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={newName} placeholder="layer name (e.g. canopy)" autofocus />
      <button type="submit">Add</button>
    </form>
  {/if}

  <ul>
    {#each layers as l, i (l.id)}
      {@const chip = stateChip(l, i)}
      <li class:sel={l.id === selectedId}>
        <div class="row-top">
          <button class="row" onclick={() => onselect(l.id)}>
            <span class="depth mono">{i}</span>
            <span class="lname">{l.name}</span>
            <span class="chip {chip.cls} tiny">{chip.label}</span>
          </button>
          <span class="ops">
            {#if l.bbox && fillStates[l.id] && fillStates[l.id] !== "clear"}
              <button
                class="ghost op"
                title="re-fill just this layer (others untouched)"
                disabled={busy}
                onclick={() => onrefill(l.id)}
              >
                ↻
              </button>
            {/if}
            <button class="ghost op" title="move deeper" disabled={i <= 1 || busy} onclick={() => onmove(l.id, -1)}>↑</button>
            <button class="ghost op" title="move nearer" disabled={i === 0 || i === layers.length - 1 || busy} onclick={() => onmove(l.id, 1)}>↓</button>
            {#if i > 0}
              <button class="ghost op del" title="delete layer" disabled={busy} onclick={() => ondelete(l.id)}>×</button>
            {/if}
          </span>
        </div>
        {#if l.id === selectedId}
          <div class="detail">
            <input
              class="name-edit"
              value={l.name}
              onchange={(e) => onname(l.id, e.currentTarget.value)}
            />
            {#if i === 0}
              <p class="muted tiny">
                Backdrop — automatically owns every pixel the nearer layers don't claim.
              </p>
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    padding: 0.9rem;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .order-hint {
    font-size: 0.62rem;
    margin: 0.15rem 0 0.5rem;
  }
  .tiny-btn {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
  }
  .addrow {
    display: flex;
    gap: 0.4rem;
    margin-bottom: 0.5rem;
  }
  .addrow input {
    flex: 1;
    min-width: 0;
    font-size: 0.75rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    border-radius: var(--radius-sm);
    margin-bottom: 2px;
  }
  li.sel {
    background: var(--wash);
  }
  .row-top {
    display: flex;
    align-items: center;
  }
  .row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: none;
    padding: 0.35rem 0.5rem;
    text-align: left;
    min-width: 0;
  }
  .depth {
    font-size: 0.6rem;
    color: var(--ash-deep);
    width: 0.9rem;
    flex: none;
  }
  .lname {
    font-size: 0.78rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  li.sel .lname {
    color: var(--bone);
  }
  .chip {
    flex: none;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 0.4rem;
    color: var(--ash-deep);
  }
  .chip.ok {
    color: var(--sage);
    border-color: rgba(127, 197, 132, 0.4);
  }
  .chip.mid {
    color: var(--lapis);
  }
  .chip.warn {
    color: var(--gold);
    border-color: var(--gold-deep);
  }
  .ops {
    display: flex;
    opacity: 0;
    transition: opacity 120ms var(--ease);
  }
  li:hover .ops,
  li.sel .ops {
    opacity: 1;
  }
  .op {
    padding: 0.15rem 0.3rem;
    font-size: 0.7rem;
  }
  .op.del:hover {
    color: var(--oxblood);
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.2rem 0.5rem 0.6rem 1.6rem;
  }
  .name-edit {
    font-size: 0.75rem;
  }
  .speed {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
</style>
