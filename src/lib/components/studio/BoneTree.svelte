<script lang="ts">
  /** Rig-mode bone hierarchy: indented tree, click to select. */
  import type { Bone } from "$lib/ipc";

  let {
    bones,
    selected,
    onselect,
  }: {
    bones: Bone[];
    selected: string | null;
    onselect: (name: string) => void;
  } = $props();

  type Row = { bone: Bone; depth: number };

  const rows = $derived.by(() => {
    const out: Row[] = [];
    const children = new Map<string | null, Bone[]>();
    for (const b of bones) {
      const key = b.parent ?? null;
      if (!children.has(key)) children.set(key, []);
      children.get(key)!.push(b);
    }
    const walk = (parent: string | null, depth: number) => {
      for (const b of children.get(parent) ?? []) {
        out.push({ bone: b, depth });
        walk(b.name, depth + 1);
      }
    };
    walk(null, 0);
    // Orphans (shouldn't happen, but never hide data).
    for (const b of bones) {
      if (!out.some((r) => r.bone.name === b.name)) out.push({ bone: b, depth: 0 });
    }
    return out;
  });
</script>

<div class="tree">
  <span class="u-label">Bones</span>
  <ul>
    {#each rows as r (r.bone.name)}
      <li>
        <button
          class="row"
          class:sel={r.bone.name === selected}
          style:padding-left={`${0.5 + r.depth * 0.85}rem`}
          onclick={() => onselect(r.bone.name)}
        >
          <span class="dot" class:root={!r.bone.parent}>{r.bone.parent ? "◆" : "✛"}</span>
          <span class="bname mono">{r.bone.name}</span>
          {#if (r.bone.length ?? 0) > 0}
            <span class="muted tiny mono">{Math.round(r.bone.length ?? 0)}px</span>
          {/if}
        </button>
      </li>
    {/each}
  </ul>
</div>

<style>
  .tree {
    padding: 0.9rem;
    overflow-y: auto;
  }
  ul {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.45rem;
    background: transparent;
    border: none;
    padding: 0.28rem 0.5rem;
    text-align: left;
    border-radius: var(--radius-sm);
  }
  .row.sel {
    background: var(--wash);
  }
  .dot {
    font-size: 0.55rem;
    color: var(--ash-deep);
  }
  .dot.root {
    color: var(--ash);
  }
  .row.sel .dot {
    color: var(--gold);
  }
  .bname {
    font-size: 0.75rem;
    color: var(--bone-dim);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.sel .bname {
    color: var(--bone);
  }
</style>
