<script lang="ts">
  /**
   * Bring-your-own-agent blueprint porter. No in-app chat: copy a self-contained prompt
   * (instructions + JSON template + current draft), fill it in whatever external agent you
   * like, paste the JSON back, Apply. The applied config feeds the deterministic prompt
   * assembly directly (style master + symbol descriptions), so generated prompts stay correct.
   */
  import { commands, unwrap, type GameConfig } from "$lib/ipc";
  import { copyText } from "$lib/copy";

  let { config = $bindable() }: { config: GameConfig } = $props();

  let pasted = $state("");
  let applying = $state(false);
  let note = $state("");
  let error = $state("");

  async function copyPrompt() {
    error = "";
    note = "";
    try {
      const prompt = await commands.configDraftPrompt($state.snapshot(config));
      if (await copyText(prompt)) {
        note = "Copied. Paste it into any AI agent, then paste its JSON reply below and Apply.";
      } else {
        error = "Couldn't reach the clipboard — copy the text manually.";
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function applyPasted() {
    if (!pasted.trim() || applying) return;
    applying = true;
    error = "";
    note = "";
    try {
      config = await unwrap(commands.applyConfigJson($state.snapshot(config), pasted));
      note = "Applied — the form on the left is updated. Review, then Create / Save.";
      pasted = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      applying = false;
    }
  }
</script>

<div class="porter">
  <p class="hint">
    Design the game in whatever agent you like. <strong>Copy</strong> a self-contained prompt
    (instructions + JSON template + your current draft), fill it in, then <strong>paste</strong> the
    JSON reply back and Apply — no in-app chat, no lock-in.
  </p>

  <section class="card">
    <div class="card-head">
      <span class="step">1</span>
      <span class="card-title">Copy the brief</span>
    </div>
    <p class="card-note">A self-contained prompt: instructions, the JSON template, and your current draft.</p>
    <button class="gold copy" onclick={copyPrompt}>⧉&nbsp; Copy prompt + template</button>
  </section>

  <div class="bridge">
    <span class="line"></span>
    <span class="bridge-text">paste into ChatGPT · Claude · any agent → it replies with one JSON object</span>
    <span class="line"></span>
  </div>

  <section class="card">
    <div class="card-head">
      <span class="step">2</span>
      <span class="card-title">Paste the reply &amp; apply</span>
    </div>
    <textarea
      bind:value={pasted}
      rows="9"
      spellcheck="false"
      placeholder={`Paste the agent's JSON reply here…\n{ "name": "…", "symbols": [ … ], "stylePrompt": "…" }`}
    ></textarea>
    <div class="card-foot">
      {#if note}<span class="msg ok">{note}</span>{:else if error}<span class="msg err">{error}</span>{/if}
      <button class="gold apply" onclick={applyPasted} disabled={applying || !pasted.trim()}>
        {applying ? "Applying…" : "Apply to Blueprint"}
      </button>
    </div>
  </section>
</div>

<style>
  .porter {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-height: 0;
    max-width: 680px;
  }
  .hint {
    font-size: 0.82rem;
    color: var(--ash);
    line-height: 1.55;
    margin: 0;
  }
  .hint strong {
    color: var(--bone-dim);
    font-weight: 600;
  }
  .card {
    background: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: var(--space-4) var(--space-5) var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .step {
    flex: none;
    width: 22px;
    height: 22px;
    border-radius: 999px;
    display: grid;
    place-content: center;
    font-size: 0.7rem;
    font-family: var(--font-mono);
    color: var(--ash);
    background: var(--ink-2);
    border: 1px solid var(--line-2);
  }
  .card-title {
    font-size: 0.92rem;
    font-weight: 600;
    color: var(--bone);
  }
  .card-note {
    margin: 0;
    font-size: 0.76rem;
    color: var(--ash);
    line-height: 1.45;
  }
  .copy {
    align-self: flex-start;
  }
  .bridge {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-2);
  }
  .bridge .line {
    flex: 1;
    height: 1px;
    background: var(--line);
  }
  .bridge-text {
    font-size: 0.68rem;
    color: var(--ash-deep);
    white-space: nowrap;
  }
  textarea {
    width: 100%;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    line-height: 1.55;
    resize: vertical;
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--bone);
    padding: var(--space-3);
  }
  textarea:focus {
    outline: none;
    border-color: var(--gold-deep);
  }
  .card-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 1.5rem;
  }
  .msg {
    font-size: 0.74rem;
    line-height: 1.4;
  }
  .msg.ok {
    color: var(--sage);
  }
  .msg.err {
    color: var(--oxblood);
  }
  .apply {
    flex: none;
    margin-left: auto;
  }
</style>
