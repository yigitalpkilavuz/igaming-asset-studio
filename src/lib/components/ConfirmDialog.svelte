<script lang="ts">
  /**
   * The app's confirmation dialog — replaces native confirm(). Danger-styled by
   * default (it exists for destructive actions). Esc / backdrop / Cancel all decline;
   * only the explicit action button confirms.
   */
  let {
    title,
    message,
    confirmLabel = "Delete",
    busy = false,
    onconfirm,
    oncancel,
  }: {
    title: string;
    message: string;
    confirmLabel?: string;
    busy?: boolean;
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      oncancel();
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div class="overlay" onclick={oncancel}>
  <div
    class="dialog rise"
    role="alertdialog"
    aria-modal="true"
    tabindex="-1"
    aria-label={title}
    onclick={(e) => e.stopPropagation()}
  >
    <h2 class="display dlg-title">{title}</h2>
    <p class="dlg-message">{message}</p>
    <div class="dlg-foot">
      <button class="ghost" onclick={oncancel} disabled={busy}>Cancel</button>
      <button class="danger-solid" onclick={onconfirm} disabled={busy}>
        {busy ? "Working…" : confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    display: grid;
    place-items: center;
    z-index: var(--z-modal);
  }
  .dialog {
    width: min(440px, 92vw);
    padding: var(--space-6);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .dlg-title {
    font-size: 1.1rem;
  }
  .dlg-message {
    margin: 0;
    font-size: var(--text-body);
    line-height: 1.55;
    color: var(--bone-dim);
    white-space: pre-line;
  }
  .dlg-foot {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-3);
    margin-top: var(--space-2);
  }
  .danger-solid {
    color: #fff;
    background: var(--oxblood);
    border-color: var(--oxblood);
  }
  .danger-solid:hover:not(:disabled) {
    filter: brightness(1.1);
    background: var(--oxblood);
    color: #fff;
  }
</style>
