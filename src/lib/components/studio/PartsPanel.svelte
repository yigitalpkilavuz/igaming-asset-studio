<script lang="ts">
  /** Cut-mode part list: selection, z-order (top of list = drawn behind), status badges. */
  import type { Part } from "$lib/ipc";

  let {
    parts,
    selectedId,
    maskReady,
    thumbs = {},
    onselect,
    onadd,
    ondelete,
    onmove,
  }: {
    parts: Part[];
    selectedId: string | null;
    /** part id → has a persisted mask. */
    maskReady: Record<string, boolean>;
    /** part id → cut-texture thumbnail data URL (post-cut verification). */
    thumbs?: Record<string, string>;
    onselect: (id: string) => void;
    onadd: (name: string) => void;
    ondelete: (id: string) => void;
    onmove: (id: string, dir: -1 | 1) => void;
  } = $props();

  let adding = $state(false);
  let newName = $state("");

  function submitAdd() {
    const n = newName.trim();
    if (n) onadd(n);
    newName = "";
    adding = false;
  }

  function status(p: Part): { glyph: string; label: string; cls: string } {
    if (p.bbox && !p.maskHash) return { glyph: "✦", label: "fx", cls: "ok" };
    if (p.bbox && maskReady[p.id]) return { glyph: "✂", label: "cut", cls: "ok" };
    if (maskReady[p.id]) return { glyph: "◐", label: "selected", cls: "mid" };
    return { glyph: "○", label: "no selection", cls: "dim" };
  }
</script>

<div class="panel">
  <div class="head">
    <span class="u-label">Parts</span>
    <button class="ghost tiny-btn" onclick={() => (adding = true)}>+ Add</button>
  </div>
  <p class="muted order-hint">back → front</p>

  {#if adding}
    <form
      class="addrow"
      onsubmit={(e) => {
        e.preventDefault();
        submitAdd();
      }}
    >
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={newName} placeholder="part name (e.g. head)" autofocus />
      <button type="submit">Add</button>
    </form>
  {/if}

  <ul>
    {#each parts as p, i (p.id)}
      {@const s = status(p)}
      <li class:sel={p.id === selectedId}>
        <button class="row" onclick={() => onselect(p.id)}>
          {#if thumbs[p.id]}
            <span class="thumb"><img src={thumbs[p.id]} alt={p.id} /></span>
          {:else}
            <span class="glyph {s.cls}">{s.glyph}</span>
          {/if}
          <span class="pname mono">{p.id}</span>
          <span class="muted tiny">{s.label}</span>
        </button>
        <span class="ops">
          <button class="ghost op" title="move back" disabled={i === 0} onclick={() => onmove(p.id, -1)}>↑</button>
          <button class="ghost op" title="move front" disabled={i === parts.length - 1} onclick={() => onmove(p.id, 1)}>↓</button>
          <button class="ghost op del" title="delete part" onclick={() => ondelete(p.id)}>×</button>
        </span>
      </li>
    {/each}
  </ul>
  {#if !parts.length}
    <p class="muted small empty">No parts yet — run “Select parts (AI)” above, or add parts by hand.</p>
  {/if}
</div>

<style>
  .panel {
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
    display: flex;
    align-items: center;
    border-radius: var(--radius-sm);
  }
  li.sel {
    background: var(--wash);
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
  .thumb {
    width: 26px;
    height: 26px;
    flex: none;
    border-radius: 4px;
    overflow: hidden;
    border: 1px solid var(--line);
    background:
      repeating-conic-gradient(var(--wash) 0% 25%, transparent 0% 50%)
      0 0 / 8px 8px;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .thumb img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .glyph.ok {
    color: var(--gold);
  }
  .glyph.mid {
    color: var(--lapis);
  }
  .glyph.dim {
    color: var(--ash-deep);
  }
  .pname {
    font-size: 0.75rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  li.sel .pname {
    color: var(--bone);
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
  .empty {
    margin-top: 0.8rem;
  }
</style>
