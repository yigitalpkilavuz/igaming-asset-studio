<script lang="ts">
  /**
   * The dock's disclosure primitive: a titled, collapsible section. Collapsed it shows a
   * one-line summary of its state, so the dock reads as a table of contents; open it
   * shows the controls. Controlled by the parent (the dock is a one-open-at-a-time
   * guided accordion); `done` marks completed steps with a ✓ instead of a chevron.
   */
  import type { Snippet } from "svelte";

  let {
    title,
    summary = "",
    badge = "",
    open = false,
    done = false,
    ontoggle,
    children,
  }: {
    title: string;
    /** One-line state summary shown while collapsed. */
    summary?: string;
    /** Small trailing tag (e.g. "busy", "ready"). */
    badge?: string;
    open?: boolean;
    /** This step is complete — collapsed header shows a ✓. */
    done?: boolean;
    ontoggle?: () => void;
    children: Snippet;
  } = $props();
</script>

<section class="dock-sec" class:open>
  <button class="head" onclick={() => ontoggle?.()}>
    <span class="chev" class:done={done && !open}>{open ? "▾" : done ? "✓" : "▸"}</span>
    <span class="title u-label" class:on={open}>{title}</span>
    {#if !open && summary}<span class="summary">{summary}</span>{/if}
    {#if badge}<span class="badge mono">{badge}</span>{/if}
  </button>
  {#if open}
    <div class="body">
      {@render children()}
    </div>
  {/if}
</section>

<style>
  .dock-sec {
    border-bottom: 1px solid var(--line);
  }
  .head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: none;
    padding: 0.6rem 0.9rem;
    text-align: left;
    border-radius: 0;
  }
  .head:hover {
    background: var(--wash-faint);
  }
  .chev {
    color: var(--ash-deep);
    font-size: 0.7rem;
    width: 0.8rem;
    flex: none;
  }
  .chev.done {
    color: var(--sage);
  }
  .dock-sec.open .chev {
    color: var(--gold);
  }
  .title {
    flex: none;
  }
  .title.on {
    color: var(--bone);
  }
  .summary {
    flex: 1;
    min-width: 0;
    font-size: 0.68rem;
    color: var(--ash);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    flex: none;
    font-size: 0.62rem;
    color: var(--gold);
  }
  .body {
    padding: 0 0.9rem 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
  }
</style>
