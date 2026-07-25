<script lang="ts">
  /** Global activity stack (bottom-right): running jobs with a spinner, finished ones
   *  with ✓/✗ for a few seconds. Clicking a job with an asset jumps to it. Visible in
   *  every view, so background work is never invisible. */
  import { jobsState, dismissJob, type Job } from "$lib/stores/jobs.svelte";
  import { appState, goProducer } from "$lib/stores/app.svelte";

  function open(job: Job) {
    if (job.status !== "running") dismissJob(job.id);
    if (job.assetKey && appState.section !== "animate") {
      goProducer(job.gameId, job.assetKey);
    }
  }
</script>

{#if jobsState.jobs.length}
  <div class="toasts">
    {#each jobsState.jobs as job (job.id)}
      <button
        class="toast {job.status}"
        onclick={() => open(job)}
        title={job.status === "error" ? job.message : job.assetKey ?? ""}
      >
        {#if job.status === "running"}
          <span class="spin"></span>
        {:else if job.status === "done"}
          <span class="mark ok">✓</span>
        {:else}
          <span class="mark err">✗</span>
        {/if}
        <span class="body">
          <span class="label">{job.label}</span>
          {#if job.message}
            <span class="msg">{job.message}</span>
          {/if}
        </span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .toasts {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 200;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    max-width: 320px;
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 0.55rem;
    text-align: left;
    background: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: var(--radius);
    padding: 0.5rem 0.75rem;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
    animation: rise 0.25s var(--ease-out) both;
  }
  .toast.done {
    border-color: var(--sage);
  }
  .toast.error {
    border-color: var(--oxblood);
  }
  .spin {
    width: 12px;
    height: 12px;
    margin-top: 2px;
    flex: none;
    border-radius: 50%;
    border: 2px solid var(--line-2);
    border-top-color: var(--gold);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .mark {
    flex: none;
    font-size: 0.8rem;
    line-height: 1.2;
  }
  .mark.ok {
    color: var(--sage);
  }
  .mark.err {
    color: var(--oxblood);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    min-width: 0;
  }
  .label {
    font-size: 0.76rem;
    color: var(--bone);
  }
  .msg {
    font-size: 0.68rem;
    color: var(--ash);
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: spin 0.9s linear infinite !important;
    }
  }
</style>
