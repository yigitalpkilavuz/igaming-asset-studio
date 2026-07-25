<script lang="ts">
  // Symbol-set composer: review the auto-built sheet prompt (override at will), see
  // the generated OR imported sheet WITH its detected cut lines before anything is
  // split, reroll until it's right, then commit — only Commit touches the symbols'
  // variations.
  import {
    commands,
    unwrap,
    type AssetRecord,
    type ProviderInfo,
    type SetPlan,
    type SetSheet,
    type SetSheetInfo,
  } from "$lib/ipc";
  import { open } from "@tauri-apps/plugin-dialog";
  import { Grid3x3, Scissors, Upload, X } from "@lucide/svelte";

  let {
    gameId,
    assetKeys,
    providers,
    initialProvider,
    onclose,
    oncommitted,
  }: {
    gameId: string;
    assetKeys: string[];
    providers: ProviderInfo[];
    initialProvider: string;
    onclose: () => void;
    oncommitted: (records: AssetRecord[]) => void;
  } = $props();

  let plan = $state<SetPlan | null>(null);
  let refKeys = $state<string[]>([]);
  let refCandidates = $state<{ key: string; varId: string }[]>([]);
  let refThumbs = $state<Record<string, string>>({});
  const refThumbRequested = new Set<string>();
  const MAX_REFS = 4;
  let prompt = $state("");
  let autoPrompt = "";
  // svelte-ignore state_referenced_locally — deliberate initial capture; the modal
  // owns its provider choice after opening.
  let providerId = $state(initialProvider);
  let sheet = $state<SetSheet | null>(null);
  let history = $state<SetSheetInfo[]>([]);
  let sheetSource = $state<"" | "generated" | "imported">("");
  let showCuts = $state(true);
  let busy = $state<"" | "gen" | "import" | "cut">("");
  let error = $state("");

  const overridden = $derived(plan !== null && prompt.trim() !== autoPrompt.trim());
  const supportsRefs = $derived(providers.find((p) => p.id === providerId)?.supportsRefs ?? false);

  // Any asset with a take can serve as a style reference (same rule as the bench).
  $effect(() => {
    const gid = gameId;
    (async () => {
      try {
        const recs = await unwrap(commands.listAssetRecords(gid));
        const cands: { key: string; varId: string }[] = [];
        for (const rec of recs) {
          const act = rec.activeVariation;
          if (!act) continue;
          const v = rec.variations?.find((x) => x.id === act);
          if (!v) continue;
          cands.push({ key: rec.key, varId: act });
          if (!refThumbRequested.has(rec.key)) {
            refThumbRequested.add(rec.key);
            const hasWebp = v.stages?.some((st) => st.name === "webp");
            const pr = hasWebp
              ? commands.getVariationStageImage(gid, rec.key, act, "webp")
              : commands.getVariationImage(gid, rec.key, act);
            pr.then((r) => {
              if (r.status === "ok") refThumbs = { ...refThumbs, [rec.key]: r.data };
            });
          }
        }
        refCandidates = cands;
      } catch {
        /* ignore — the picker just stays empty */
      }
    })();
  });

  function toggleRef(key: string) {
    if (refKeys.includes(key)) refKeys = refKeys.filter((k) => k !== key);
    else if (refKeys.length < MAX_REFS) refKeys = [...refKeys, key];
  }

  async function loadHistory() {
    const r = await commands.listSymbolSetSheets(gameId);
    if (r.status === "ok") history = r.data;
  }

  async function selectSheet(id: string) {
    if (busy || sheet?.id === id) return;
    error = "";
    try {
      sheet = await unwrap(commands.selectSymbolSetSheet(gameId, id));
      sheetSource = "";
      await loadHistory();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  $effect(() => {
    loadHistory();
    commands.planSymbolSet(gameId, assetKeys).then((r) => {
      if (r.status === "ok") {
        plan = r.data;
        autoPrompt = r.data.positive;
        prompt = r.data.positive;
      } else {
        error = r.error;
      }
    });
  });

  async function generate() {
    busy = "gen";
    error = "";
    try {
      sheet = await unwrap(
        commands.generateSymbolSetSheet(
          gameId,
          assetKeys,
          providerId,
          overridden ? prompt : "",
          supportsRefs ? [...refKeys] : [],
        ),
      );
      sheetSource = "generated";
      await loadHistory();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = "";
    }
  }

  async function importSheet() {
    const path = await open({
      multiple: false,
      filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (typeof path !== "string") return;
    busy = "import";
    error = "";
    try {
      sheet = await unwrap(commands.importSymbolSetSheet(gameId, assetKeys, path));
      sheetSource = "imported";
      await loadHistory();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = "";
    }
  }

  async function commit() {
    busy = "cut";
    error = "";
    try {
      const records = await unwrap(commands.commitSymbolSet(gameId));
      oncommitted(records);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      busy = "";
    }
  }

  // ── Manual cut override: change the grid, or drag a cut line ─────────────────
  let adjusting = $state(false);

  async function adjust(cols: number, rows: number, sx: number[], sy: number[]) {
    if (!sheet || busy) return;
    adjusting = true;
    error = "";
    try {
      sheet = await unwrap(commands.adjustSymbolSet(gameId, cols, rows, sx, sy));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      adjusting = false;
    }
  }

  function setGrid(dcols: number, drows: number) {
    if (!sheet) return;
    // Changing the shape re-detects seams for it (empty vecs = auto).
    adjust(sheet.cols + dcols, sheet.rows + drows, [], []);
  }

  // Drag state: axis + seam index; fractions update live, persist on release.
  let drag = $state<{ axis: "x" | "y"; idx: number } | null>(null);
  let sheetWrap = $state<HTMLElement | null>(null);

  function seamDown(axis: "x" | "y", idx: number, e: PointerEvent) {
    if (!sheet || busy || adjusting) return;
    drag = { axis, idx };
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }

  function seamMove(e: PointerEvent) {
    if (!drag || !sheet || !sheetWrap) return;
    const r = sheetWrap.getBoundingClientRect();
    const frac =
      drag.axis === "x" ? (e.clientX - r.left) / r.width : (e.clientY - r.top) / r.height;
    const list = drag.axis === "x" ? [...sheet.seamsX] : [...sheet.seamsY];
    // Clamp between neighbours so seams stay ordered.
    const lo = (drag.idx > 0 ? (list[drag.idx - 1] ?? 0) : 0) + 0.02;
    const hi = (drag.idx < list.length - 1 ? (list[drag.idx + 1] ?? 1) : 1) - 0.02;
    list[drag.idx] = Math.min(hi, Math.max(lo, frac));
    if (drag.axis === "x") sheet.seamsX = list;
    else sheet.seamsY = list;
  }

  function seamUp() {
    if (!drag || !sheet) return;
    drag = null;
    adjust(
      sheet.cols,
      sheet.rows,
      sheet.seamsX.map((v) => v ?? 0),
      sheet.seamsY.map((v) => v ?? 0),
    );
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && !busy) {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onkeydown} />

<div class="backdrop" onclick={() => !busy && onclose()} role="presentation">
  <!-- svelte-ignore a11y_interactive_supports_focus, a11y_click_events_have_key_events -->
  <div class="modal rise" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Symbol set">
    <header>
      <div class="head-text">
        <span class="u-label">Symbol set</span>
        <h3>One sheet, one style — cut into {assetKeys.length} symbols</h3>
      </div>
      {#if plan}
        <span class="grid-badge mono" title="sheet grid — reading order left→right, top→bottom">
          <Grid3x3 size={12} strokeWidth={1.75} />
          {plan.rows}×{plan.cols}
        </span>
      {/if}
      <button class="icon-btn" onclick={onclose} disabled={!!busy} title="Close (Esc)">
        <X size={15} strokeWidth={1.75} />
      </button>
    </header>

    <div class="body">
      <div class="left">
        {#if plan}
          <div class="cells">
            {#each plan.cells as c, i (c.assetKey)}
              <span class="cell-chip" title={c.label}>
                <b>{i + 1}</b>{c.assetKey.replace("symbol_", "")}
              </span>
            {/each}
          </div>
        {/if}

        <label class="field">
          <span class="f-label">Provider</span>
          <select bind:value={providerId} disabled={!!busy}>
            {#each providers as p (p.id)}
              <option value={p.id} disabled={!p.configured}>
                {p.displayName}{p.configured ? "" : " — set up in Settings"}
              </option>
            {/each}
          </select>
        </label>

        {#if supportsRefs && refCandidates.length}
          <div class="field">
            <span class="f-label">
              Style references
              <em class="f-note">{refKeys.length}/{MAX_REFS} — any asset you already made</em>
            </span>
            <div class="refs">
              {#each refCandidates as c (c.key)}
                <button
                  class="ref-chip"
                  class:on={refKeys.includes(c.key)}
                  disabled={!!busy || (!refKeys.includes(c.key) && refKeys.length >= MAX_REFS)}
                  title={c.key}
                  onclick={() => toggleRef(c.key)}
                >
                  {#if refThumbs[c.key]}<img src={refThumbs[c.key]} alt="" />{/if}
                </button>
              {/each}
            </div>
          </div>
        {/if}

        <div class="field grow">
          <span class="f-label">
            Sheet prompt
            <em class="f-note">{overridden ? "custom" : "auto-built from the Blueprint"}</em>
            {#if overridden}
              <button class="linkish" onclick={() => (prompt = autoPrompt)} disabled={!!busy}>
                reset to auto
              </button>
            {/if}
          </span>
          <textarea bind:value={prompt} disabled={!!busy || !plan} spellcheck="false"></textarea>
        </div>

        {#if error}<p class="error">{error}</p>{/if}
      </div>

      <div class="right">
        <div class="preview" class:empty={!sheet}>
          {#if sheet}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="sheet-wrap"
              class:dimmed={busy === "gen" || busy === "import"}
              bind:this={sheetWrap}
              onpointermove={seamMove}
              onpointerup={seamUp}
            >
              <img src={sheet.png} alt="Symbol sheet preview" draggable="false" />
              {#if showCuts}
                {#each sheet.seamsX as x, i (i)}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="seam v"
                    class:dragging={drag?.axis === "x" && drag?.idx === i}
                    style="left:{(x ?? 0) * 100}%"
                    onpointerdown={(e) => seamDown("x", i, e)}
                  ></div>
                {/each}
                {#each sheet.seamsY as y, i (i)}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="seam h"
                    class:dragging={drag?.axis === "y" && drag?.idx === i}
                    style="top:{(y ?? 0) * 100}%"
                    onpointerdown={(e) => seamDown("y", i, e)}
                  ></div>
                {/each}
              {/if}
            </div>
          {:else}
            <div class="empty-state">
              <Grid3x3 size={28} strokeWidth={1.25} />
              <p>
                {busy === "gen"
                  ? "Drawing the sheet…"
                  : busy === "import"
                    ? "Reading your image…"
                    : "Generate a sheet — or upload your own image drawn to the grid."}
              </p>
              <p class="tiny">Nothing is cut until you commit.</p>
            </div>
          {/if}
          {#if sheet && (busy === "gen" || busy === "import")}
            <div class="busy-veil mono">{busy === "gen" ? "Redrawing…" : "Reading…"}</div>
          {/if}
        </div>
        {#if history.length > 1 || (history.length === 1 && !sheet)}
          <div class="strip" title="sheet history — every roll is kept; click to go back to one">
            {#each history as hSheet (hSheet.id)}
              <button
                class="hthumb"
                class:on={sheet?.id === hSheet.id}
                disabled={!!busy}
                title="{hSheet.id} · {hSheet.rows}×{hSheet.cols} · {hSheet.symbolCount} symbols"
                onclick={() => selectSheet(hSheet.id)}
              >
                <img src={hSheet.thumb} alt={hSheet.id} />
              </button>
            {/each}
          </div>
        {/if}
        {#if sheet}
          <div class="under">
            <label class="cuts-toggle">
              <input type="checkbox" bind:checked={showCuts} />
              cut lines
            </label>
            <span class="hint">drag a line to move a cut; adjust the grid if detection got it wrong</span>
            <span class="grid-ctl mono" title="cut grid — rows × columns">
              <button onclick={() => setGrid(0, -1)} disabled={!!busy || adjusting || sheet.rows <= 1}>−</button>
              {sheet.rows}
              <button onclick={() => setGrid(0, 1)} disabled={!!busy || adjusting}>+</button>
              ×
              <button onclick={() => setGrid(-1, 0)} disabled={!!busy || adjusting || sheet.cols <= 1}>−</button>
              {sheet.cols}
              <button onclick={() => setGrid(1, 0)} disabled={!!busy || adjusting}>+</button>
            </span>
            <span class="source mono" class:differs={plan !== null && (sheet.cols !== plan.cols || sheet.rows !== plan.rows)}
              title={plan !== null && (sheet.cols !== plan.cols || sheet.rows !== plan.rows)
                ? `the model drew a ${sheet.rows}×${sheet.cols} layout instead of the requested ${plan.rows}×${plan.cols} — the cut follows the image, so this is fine`
                : "detected grid"}>
              {sheetSource || sheet.id} · {sheet.rows}×{sheet.cols}
            </span>
          </div>
        {/if}
      </div>
    </div>

    <footer>
      <button onclick={generate} disabled={!!busy || !plan || !providerId}>
        {busy === "gen" ? "Generating…" : sheet ? "✦ Reroll sheet" : "✦ Generate sheet"}
      </button>
      <button class="ghost with-icon" onclick={importSheet} disabled={!!busy || !plan} title="use your own image as the sheet — same grid, same cut">
        <Upload size={13} strokeWidth={1.75} />
        Upload sheet…
      </button>
      <span class="spacer"></span>
      <button class="gold with-icon" onclick={commit} disabled={!!busy || !sheet}>
        <Scissors size={13} strokeWidth={1.75} />
        {busy === "cut" ? "Cutting…" : "Cut into symbols"}
      </button>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--z-modal);
    background: var(--scrim);
    display: grid;
    place-items: center;
    padding: var(--space-5);
  }
  .modal {
    width: min(1120px, 94vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-3);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  .head-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 600;
  }
  .grid-badge {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: 0.7rem;
    color: var(--bone-dim);
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    padding: 2px var(--space-2);
  }
  .icon-btn {
    display: grid;
    place-items: center;
    background: none;
    border: none;
    color: var(--bone-dim);
    cursor: pointer;
    padding: var(--space-1);
    border-radius: var(--radius-1, 4px);
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--bone);
    background: var(--ink-2);
  }

  .body {
    display: grid;
    grid-template-columns: 340px 1fr;
    gap: var(--space-5);
    padding: var(--space-4) var(--space-5);
    min-height: 0;
    overflow: hidden;
  }
  .left {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-height: 0;
    overflow-y: auto;
  }

  .cells {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .cell-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-family: var(--font-mono);
    font-size: 0.68rem;
    color: var(--bone);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    padding: 2px var(--space-2) 2px var(--space-1);
  }
  .cell-chip b {
    font-weight: 600;
    color: var(--gold-bright);
    font-size: 0.62rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .refs {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    max-height: 108px;
    overflow-y: auto;
  }
  .ref-chip {
    width: 44px;
    height: 44px;
    padding: 2px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm, 4px);
    background: var(--ink-2);
    overflow: hidden;
    cursor: pointer;
  }
  .ref-chip.on {
    border-color: var(--gold-bright);
  }
  .ref-chip img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .field.grow {
    flex: 1;
    min-height: 0;
  }
  .f-label {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    font-size: 0.72rem;
    font-weight: 500;
    color: var(--bone-dim);
  }
  .f-note {
    font-style: normal;
    font-size: 0.66rem;
    color: var(--bone-faint, var(--bone-dim));
  }
  .field textarea {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    line-height: 1.55;
    resize: none;
    min-height: 12rem;
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    padding: var(--space-3);
  }
  .field textarea:focus {
    outline: none;
    border-color: var(--gold-deep);
  }
  .linkish {
    background: none;
    border: none;
    color: var(--gold-bright);
    cursor: pointer;
    font-size: 0.66rem;
    padding: 0;
  }

  .right {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-height: 0;
  }
  .preview {
    position: relative;
    flex: 1;
    min-height: 360px;
    overflow: auto;
    border-radius: var(--radius-1, 4px);
    background: var(--ink-2);
    border: 1px solid var(--line);
  }
  .preview.empty {
    display: grid;
    place-items: center;
    border-style: dashed;
    overflow: hidden;
  }
  .sheet-wrap {
    position: relative;
    line-height: 0;
    transition: opacity var(--dur-med) ease;
  }
  .sheet-wrap.dimmed {
    opacity: 0.35;
  }
  .sheet-wrap img {
    width: 100%;
    height: auto;
    display: block;
  }
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    color: var(--bone-dim);
    text-align: center;
    padding: var(--space-5);
    max-width: 34ch;
  }
  .empty-state p {
    margin: 0;
    font-size: 0.8rem;
    line-height: 1.5;
  }
  .empty-state .tiny {
    font-size: 0.68rem;
    color: var(--bone-faint, var(--bone-dim));
  }
  .busy-veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 0.75rem;
    color: var(--bone);
    letter-spacing: 0.04em;
  }
  .seam {
    position: absolute;
    touch-action: none;
  }
  .seam.v {
    top: 0;
    bottom: 0;
    width: 9px;
    margin-left: -4px;
    cursor: col-resize;
    border-left: 4px solid transparent;
    border-right: 4px solid transparent;
    background: color-mix(in srgb, var(--gold-bright) 75%, transparent);
    background-clip: padding-box;
  }
  .seam.h {
    left: 0;
    right: 0;
    height: 9px;
    margin-top: -4px;
    cursor: row-resize;
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    background: color-mix(in srgb, var(--gold-bright) 75%, transparent);
    background-clip: padding-box;
  }
  .seam:hover,
  .seam.dragging {
    background: var(--gold-bright);
    background-clip: padding-box;
  }
  .grid-ctl {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    font-size: 0.68rem;
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    padding: 1px var(--space-1);
  }
  .grid-ctl button {
    background: none;
    border: none;
    color: var(--bone-dim);
    cursor: pointer;
    padding: 0 var(--space-1);
    font-size: 0.72rem;
  }
  .grid-ctl button:hover:not(:disabled) {
    color: var(--bone);
  }

  .strip {
    display: flex;
    gap: var(--space-1);
    overflow-x: auto;
    padding-bottom: 2px;
  }
  .hthumb {
    flex: none;
    width: 64px;
    height: 44px;
    padding: 2px;
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    background: var(--ink-2);
    overflow: hidden;
    cursor: pointer;
  }
  .hthumb.on {
    border-color: var(--gold-bright);
  }
  .hthumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .under {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    font-size: 0.68rem;
    color: var(--bone-dim);
  }
  .cuts-toggle {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    cursor: pointer;
    white-space: nowrap;
  }
  .hint {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .source {
    font-size: 0.64rem;
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    padding: 1px var(--space-2);
  }
  .source.differs {
    color: var(--gold-bright);
    border-color: var(--gold-deep);
  }

  footer {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    padding: var(--space-3) var(--space-5) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .with-icon {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .error {
    color: var(--red, #e5484d);
    font-size: 0.76rem;
    line-height: 1.45;
    margin: 0;
  }
  .mono {
    font-family: var(--font-mono);
  }
</style>
