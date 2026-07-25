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
  <div class="head">
    <span class="u-label">Draft with any agent</span>
    <p class="hint">
      Decide the game elsewhere. <strong>Copy</strong> a self-contained prompt (instructions +
      JSON template + your current draft), fill it in any AI agent, then <strong>paste</strong> the
      JSON reply back and Apply.
    </p>
  </div>

  <ol class="steps">
    <li>
      <span class="stepno">1</span>
      <div class="stepbody">
        <span class="steptitle">Copy the prompt</span>
        <button class="gold copy" onclick={copyPrompt}>Copy prompt + template</button>
      </div>
    </li>
    <li>
      <span class="stepno">2</span>
      <div class="stepbody">
        <span class="steptitle">Fill it in your agent</span>
        <span class="stepnote">Paste it into ChatGPT / Claude / anything. It replies with one JSON object.</span>
      </div>
    </li>
    <li>
      <span class="stepno">3</span>
      <div class="stepbody">
        <span class="steptitle">Paste the JSON back</span>
        <textarea
          bind:value={pasted}
          rows="8"
          spellcheck="false"
          placeholder={`Paste the agent's JSON reply here…\n{ "name": "…", "symbols": [ … ], "stylePrompt": "…" }`}
        ></textarea>
        <button class="gold" onclick={applyPasted} disabled={applying || !pasted.trim()}>
          {applying ? "Applying…" : "Apply to blueprint"}
        </button>
      </div>
    </li>
  </ol>

  {#if note}<p class="msg ok">{note}</p>{/if}
  {#if error}<p class="msg err">{error}</p>{/if}
</div>

<style>
  .porter {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    min-height: 0;
    overflow: auto;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .hint {
    font-size: 0.8rem;
    color: var(--ash);
    line-height: 1.5;
    margin: 0;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  .steps li {
    display: flex;
    gap: 0.7rem;
  }
  .stepno {
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: 999px;
    border: 1px solid var(--line-2);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.66rem;
    font-family: var(--font-mono);
    color: var(--ash);
    margin-top: 1px;
  }
  .stepbody {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-width: 0;
    flex: 1;
  }
  .steptitle {
    font-size: 0.82rem;
    color: var(--bone-dim);
  }
  .stepnote {
    font-size: 0.72rem;
    color: var(--ash-deep);
    line-height: 1.45;
  }
  .copy {
    align-self: flex-start;
  }
  textarea {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    line-height: 1.5;
    resize: vertical;
  }
  .msg {
    font-size: 0.74rem;
    margin: 0;
    line-height: 1.45;
  }
  .msg.ok {
    color: var(--sage);
  }
  .msg.err {
    color: var(--oxblood);
  }
</style>
