<script lang="ts">
  /**
   * The Library: every game as a catalogue plate, plus the entry point for a new one.
   * First surface on the "Instrument, refined" system — tokens only, elevation over
   * hairlines, one gold action, quiet mono readouts.
   */
  import autoAnimate from "@formkit/auto-animate";
  import { Plus, Trash2 } from "@lucide/svelte";
  import { commands, unwrap, type ProjectSummary } from "$lib/ipc";
  import { appState, gameDeleted, goEditor } from "$lib/stores/app.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  let projects = $state<ProjectSummary[]>([]);
  let root = $state("");
  let error = $state("");
  let loading = $state(true);

  // Reload when data changes (e.g. the projects folder was relocated in Settings).
  $effect(() => {
    appState.dataRev;
    (async () => {
      try {
        projects = await unwrap(commands.listProjects());
        root = await unwrap(commands.projectsRootPath());
      } catch (e) {
        error = String(e);
      } finally {
        loading = false;
      }
    })();
  });

  /** The game awaiting delete confirmation (null = dialog closed). */
  let pendingDelete = $state<ProjectSummary | null>(null);
  let deleting = $state(false);

  async function confirmDelete() {
    const p = pendingDelete;
    if (!p) return;
    deleting = true;
    try {
      await unwrap(commands.deleteProject(p.gameId));
      gameDeleted(p.gameId);
      projects = projects.filter((x) => x.gameId !== p.gameId);
      pendingDelete = null;
    } catch (e) {
      error = String(e);
      pendingDelete = null;
    } finally {
      deleting = false;
    }
  }

  function fmtDate(ms: number | null): string {
    if (!ms) return "—";
    return new Date(ms).toLocaleDateString(undefined, {
      day: "2-digit",
      month: "short",
      year: "numeric",
    });
  }
</script>

<div class="library">
  <header class="intro">
    <div class="intro-row">
      <h1 class="display title">Games</h1>
      {#if projects.length}<span class="count mono">{projects.length}</span>{/if}
    </div>
    {#if root}<p class="root mono" title="projects folder — change it in Settings">{root}</p>{/if}
  </header>

  {#if error}<p class="err">{error}</p>{/if}

  {#if loading}
    <div class="grid">
      {#each [0, 1, 2] as i (i)}
        <div class="plate skeleton" style:animation-delay={`${i * 120}ms`}></div>
      {/each}
    </div>
  {:else if !projects.length}
    <div class="empty rise">
      <div class="empty-mark" aria-hidden="true">
        <Plus size={22} strokeWidth={1.5} />
      </div>
      <h2 class="display empty-title">No games yet</h2>
      <p class="muted empty-line">
        A game starts as a Blueprint — grid, symbols, mechanics, and the style master
        that drives every image it will ever generate.
      </p>
      <button class="gold" onclick={() => goEditor(null)}>Create your first game</button>
    </div>
  {:else}
    <div class="grid rise" use:autoAnimate>
      <button class="plate new" onclick={() => goEditor(null)}>
        <Plus size={20} strokeWidth={1.75} />
        <span class="new-label">New game</span>
      </button>

      {#each projects as p (p.gameId)}
        <div class="plate-wrap">
          <button class="plate game" onclick={() => goEditor(p.gameId)}>
            <div class="plate-top">
              <span class="shelf mono">{p.gameId}</span>
            </div>
            <h2 class="name display">{p.name || p.gameId}</h2>
            <div class="plate-foot">
              <span class="foot-meta mono">{p.symbolCount} symbols</span>
              <span class="foot-date mono">{fmtDate(p.updatedAt)}</span>
            </div>
          </button>
          <button class="del" title="delete this game (permanent)" onclick={() => (pendingDelete = p)}>
            <Trash2 size={14} strokeWidth={1.75} />
          </button>
        </div>
      {/each}
    </div>
  {/if}
</div>

{#if pendingDelete}
  <ConfirmDialog
    title={`Delete “${pendingDelete.name || pendingDelete.gameId}”?`}
    message={`This permanently removes the game and everything under it — every generated take, studio rig and export — from disk.\n\nThere is no undo.`}
    confirmLabel="Delete game"
    busy={deleting}
    onconfirm={confirmDelete}
    oncancel={() => (pendingDelete = null)}
  />
{/if}

<style>
  .library {
    height: 100%;
    overflow: auto;
    padding: var(--space-8) 2.5rem var(--space-8);
  }
  .intro {
    max-width: 1160px;
    margin: 0 auto var(--space-7);
  }
  .intro-row {
    display: flex;
    align-items: baseline;
    gap: var(--space-4);
  }
  .title {
    font-size: 2rem;
  }
  .count {
    font-size: var(--text-body);
    color: var(--ash-deep);
  }
  .root {
    font-size: var(--text-xs);
    color: var(--ash-deep);
    margin: var(--space-3) 0 0;
    word-break: break-all;
  }
  .err {
    color: var(--oxblood);
    max-width: 1160px;
    margin: 0 auto;
  }

  .grid {
    max-width: 1160px;
    margin: 0 auto;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: var(--space-5);
  }

  /* ── Plates: elevated surfaces, not outlined boxes ── */
  .plate {
    text-align: left;
    padding: var(--space-6) var(--space-6) var(--space-5);
    min-height: 172px;
    display: flex;
    flex-direction: column;
    font-weight: var(--weight-normal);
    letter-spacing: normal;
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-1);
    transition:
      transform var(--dur-med) var(--ease-out),
      box-shadow var(--dur-med) var(--ease-out),
      border-color var(--dur-fast) var(--ease),
      background var(--dur-fast) var(--ease);
  }
  .plate:hover:not(:disabled) {
    transform: translateY(-2px);
    background: var(--ink-3);
    border-color: var(--line-2);
    box-shadow: var(--elev-2);
  }
  .plate:active:not(:disabled) {
    transform: translateY(0);
    box-shadow: var(--elev-1);
  }

  .new {
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    background: transparent;
    border-style: dashed;
    border-color: var(--line-2);
    box-shadow: none;
    color: var(--ash);
  }
  .new:hover:not(:disabled) {
    color: var(--bone);
    background: var(--ink-2);
    border-color: var(--ash-deep);
    box-shadow: var(--elev-1);
  }
  .new-label {
    font-size: var(--text-body);
    font-weight: var(--weight-medium);
  }

  .plate-top {
    display: flex;
    margin-bottom: auto;
  }
  .shelf {
    font-size: var(--text-xs);
    color: var(--ash-deep);
  }
  .name {
    font-size: 1.35rem;
    line-height: 1.15;
    margin: var(--space-5) 0 0;
    letter-spacing: var(--tracking-tight);
  }
  .plate-foot {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    margin-top: var(--space-4);
    padding-top: var(--space-4);
    border-top: 1px solid var(--line);
  }
  .foot-meta,
  .foot-date {
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .foot-date {
    color: var(--ash-deep);
  }

  .plate-wrap {
    position: relative;
    display: flex;
  }
  .plate-wrap .plate {
    flex: 1;
  }
  .del {
    position: absolute;
    top: var(--space-3);
    right: var(--space-3);
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    padding: 0;
    color: var(--ash-deep);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    opacity: 0;
    transition:
      opacity var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .plate-wrap:hover .del,
  .del:focus-visible {
    opacity: 1;
  }
  .del:hover {
    color: var(--oxblood);
    background: var(--wash);
    border-color: var(--line);
  }

  /* ── Loading skeletons ── */
  .skeleton {
    pointer-events: none;
    border-color: transparent;
    box-shadow: none;
    background: var(--ink-2);
    animation: pulse 1.4s var(--ease-in-out) infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 0.55;
    }
    50% {
      opacity: 0.95;
    }
  }

  /* ── Empty state ── */
  .empty {
    max-width: 420px;
    margin: 14vh auto 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: var(--space-4);
  }
  .empty-mark {
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    color: var(--ash);
    background: var(--ink-2);
    border: 1px dashed var(--line-2);
    border-radius: var(--radius-round);
  }
  .empty-title {
    font-size: 1.25rem;
  }
  .empty-line {
    font-size: var(--text-body);
    line-height: 1.55;
    margin: 0 0 var(--space-3);
  }
</style>
