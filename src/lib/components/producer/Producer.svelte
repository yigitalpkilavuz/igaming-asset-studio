<script lang="ts">
  /**
   * The Producer shell: header (nav · progress · actions), the ledger (list or grid),
   * the bench for the selected asset, the batch bar, and the Blueprint/Export overlays.
   * All per-asset authoring lives in Bench; all grouping/selection in Ledger.
   */
  import {
    commands,
    unwrap,
    type AssetDescriptor,
    type AssetRecord,
    type GameConfig,
    type ProviderInfo,
  } from "$lib/ipc";
  import { gameCreated, goProjects, selectAsset, newConfig } from "$lib/stores/app.svelte";
  import { jobsState, runJob, pushRecord } from "$lib/stores/jobs.svelte";
  import { assetStatus } from "$lib/assetStatus";
  import { open } from "@tauri-apps/plugin-dialog";
  import ConfigForm from "../ConfigForm.svelte";
  import BlueprintPorter from "../BlueprintPorter.svelte";
  import ExportModal from "../ExportModal.svelte";
  import Ledger from "./Ledger.svelte";
  import Bench from "./Bench.svelte";
  import BatchBar from "./BatchBar.svelte";
  import StyleAnchor from "./StyleAnchor.svelte";
  import ContactSheetModal from "./ContactSheetModal.svelte";
  import SetComposer from "./SetComposer.svelte";

  let { gameId, assetKey }: { gameId: string | null; assetKey: string | null } = $props();

  // ── Game / config ──
  let config = $state<GameConfig>({} as GameConfig);
  let savedId = $state<string | null>(null);
  let ready = $state(false);
  let error = $state("");
  let saving = $state(false);

  let assets = $state<AssetDescriptor[]>([]);
  let deriveError = $state("");
  let records = $state<Record<string, AssetRecord>>({});

  let blueprintOpen = $state(false);
  /** Blueprint modal tab — Identity first for a new game, Symbols for editing. */
  type BpTab = "identity" | "mechanics" | "symbols" | "scenes" | "assistant";
  const BP_TABS: { key: BpTab; label: string }[] = [
    { key: "identity", label: "Identity" },
    { key: "mechanics", label: "Mechanics" },
    { key: "symbols", label: "Symbols" },
    { key: "scenes", label: "Scenes" },
    { key: "assistant", label: "AI & Porter" },
  ];
  let bpTab = $state<BpTab>("identity");
  let exportOpen = $state(false);
  let anchorOpen = $state(false);
  /** Bumped when the style anchor changes so the bench chip refreshes. */
  let anchorRev = $state(0);
  let menuOpen = $state(false);
  let contactOpen = $state(false);
  let setKeys = $state<string[] | null>(null);
  // Every symbol the set composer can include (expanded twins generate solo).
  const setCandidates = $derived(
    assets
      .filter((a) => a.category === "symbols" && a.production === "raster" && !a.key.endsWith("_expanded"))
      .map((a) => a.key),
  );
  let legendOpen = $state(false);

  let view = $state<"list" | "grid">("list");
  let checked = $state<string[]>([]);

  // Batch state derives from the global job store, so a batch keeps running (and its
  // progress stays visible) across navigation.
  const batchJob = $derived(
    jobsState.jobs.find(
      (j) => j.status === "running" && j.kind === "batch" && j.gameId === savedId,
    ),
  );
  const batchBusy = $derived(!!batchJob);
  const batchProg = $derived(batchJob?.message ?? "");
  let batchMsg = $state("");
  let publishDir = $state<string | null>(null);
  let publishing = $state(false);
  let providersList = $state<ProviderInfo[]>([]);

  $effect(() => {
    commands.listImageProviders().then((r) => {
      if (r.status === "ok") providersList = r.data;
    });
  });

  // Provider used by the batch bar's Generate / Generate as set — seeded from the
  // remembered choice, remembered again on change (shared with the bench).
  let batchProviderId = $state("");
  $effect(() => {
    if (batchProviderId || !providersList.length) return;
    const saved = localStorage.getItem("wf-provider");
    batchProviderId =
      (saved && providersList.some((p) => p.id === saved && p.configured) ? saved : null) ??
      (providersList.find((p) => p.configured)?.id ?? "");
  });
  function rememberBatchProvider() {
    try {
      if (batchProviderId) localStorage.setItem("wf-provider", batchProviderId);
    } catch {
      /* private mode etc. */
    }
  }

  // Load existing game, or start a fresh blueprint for a new one.
  $effect(() => {
    const id = gameId;
    (async () => {
      ready = false;
      if (id) {
        try {
          const project = await unwrap(commands.getProject(id));
          config = project.config;
          publishDir = project.publishDir ?? null;
          savedId = id;
          await loadRecords();
        } catch (e) {
          error = String(e);
        }
      } else {
        config = newConfig();
        savedId = null;
        blueprintOpen = true;
      }
      ready = true;
    })();
  });

  // Re-derive the asset manifest whenever the config changes.
  $effect(() => {
    JSON.stringify(config);
    const snap = $state.snapshot(config);
    if (!snap.symbols) return;
    let cancelled = false;
    commands
      .deriveAssets(snap)
      .then((a) => {
        if (!cancelled) {
          assets = a;
          deriveError = "";
        }
      })
      .catch((e) => {
        if (!cancelled) deriveError = String(e);
      });
    return () => {
      cancelled = true;
    };
  });

  async function loadRecords() {
    if (!savedId) {
      records = {};
      return;
    }
    try {
      const recs = await unwrap(commands.listAssetRecords(savedId));
      records = Object.fromEntries(recs.map((r) => [r.key, r]));
    } catch {
      /* non-fatal */
    }
  }

  function onBenchSaved(record: AssetRecord) {
    records = { ...records, [record.key]: record };
  }

  // Merge every record a background job writes — the ledger dots update no matter
  // where the job was started or where the user is now.
  let seenRev = jobsState.rev;
  $effect(() => {
    void jobsState.rev;
    const fresh = jobsState.recordUpdates.filter((u) => u.rev > seenRev);
    seenRev = jobsState.rev;
    if (fresh.length) {
      const merged = { ...records };
      for (const u of fresh) merged[u.record.key] = u.record;
      records = merged;
    }
  });

  // ── Progress ──
  const rasterAssets = $derived(assets.filter((a) => a.production === "raster"));
  const progress = $derived.by(() => {
    let briefed = 0,
      drawn = 0,
      readyN = 0;
    for (const a of rasterAssets) {
      const s = assetStatus(records[a.key]);
      if (s.hasPrompt) briefed++;
      if (s.generated) drawn++;
      if (s.ready) readyN++;
    }
    const total = rasterAssets.length || 1;
    return { total: rasterAssets.length, briefed, drawn, ready: readyN, pct: readyN / total };
  });

  const descriptor = $derived(assets.find((a) => a.key === assetKey) ?? null);
  const isRaster = $derived(descriptor?.production === "raster");

  function pickAsset(key: string) {
    selectAsset(key);
    if (view === "grid") view = "list"; // opening an asset returns to the bench
  }

  // ── Batch actions (ledger selection) ──
  function batchTargets(pred: (a: AssetDescriptor) => boolean): AssetDescriptor[] {
    return checked
      .map((k) => assets.find((a) => a.key === k))
      .filter((a): a is AssetDescriptor => !!a && a.production === "raster")
      .filter(pred);
  }

  async function batchGenerate() {
    const gid = savedId;
    if (!gid) return;
    const provider = providersList.some((p) => p.id === batchProviderId && p.configured)
      ? batchProviderId
      : providersList.find((p) => p.configured)?.id;
    if (!provider) {
      error = "No configured image provider — add a key in Settings.";
      return;
    }
    // No subject gate: prompt assembly falls back to the Blueprint's per-asset
    // description. Assets whose assembled prompt is genuinely empty fail individually.
    const targets = batchTargets(() => true);
    if (!targets.length) return;
    error = "";
    batchMsg = "";
    const keys = targets.map((a) => a.key);
    const res = await runJob({
      gameId: gid,
      kind: "batch",
      label: `Batch generate · ${keys.length} assets`,
      exec: async (update) => {
        let ok = 0;
        const fails: string[] = [];
        for (let i = 0; i < keys.length; i++) {
          update(`Generating ${i + 1}/${keys.length}: ${keys[i]}…`);
          try {
            const rec = await unwrap(commands.generateVariation(gid, keys[i], provider, [], "", 1));
            pushRecord(rec);
            ok++;
          } catch {
            fails.push(keys[i]);
          }
        }
        if (fails.length) {
          throw new Error(`Generated ${ok}/${keys.length} · failed: ${fails.join(", ")}`);
        }
        return null;
      },
    });
    batchMsg =
      res.status === "done" ? `Generated ${keys.length}/${keys.length}` : (res.error ?? "");
  }

  /// Generate the selected symbols as ONE sheet (consistent style), auto-split + upscale.
  function batchGenerateSet() {
    if (!savedId) return;
    const keys = batchTargets((a) => a.kind === "symbol").map((a) => a.key);
    if (keys.length < 2) {
      batchMsg = "Select at least two symbols for a set.";
      return;
    }
    error = "";
    batchMsg = "";
    setKeys = keys;
  }

  function setCommitted(records: AssetRecord[]) {
    for (const r of records) pushRecord(r);
    batchMsg = `Set cut — ${records.length} symbols landed as new takes. Run Process on them next.`;
    setKeys = null;
  }

  async function batchProcess() {
    const gid = savedId;
    if (!gid) return;
    const targets = batchTargets((a) => assetStatus(records[a.key]).generated);
    const skipped = checked.length - targets.length;
    if (!targets.length) return;
    error = "";
    batchMsg = "";
    const items = targets
      .map((a) => ({ key: a.key, active: records[a.key]?.activeVariation }))
      .filter((t): t is { key: string; active: string } => !!t.active);
    const res = await runJob({
      gameId: gid,
      kind: "batch",
      label: `Batch process · ${items.length} assets`,
      exec: async (update) => {
        let ok = 0;
        const fails: string[] = [];
        for (let i = 0; i < items.length; i++) {
          update(`Processing ${i + 1}/${items.length}: ${items[i].key}…`);
          try {
            const rec = await unwrap(commands.processVariation(gid, items[i].key, items[i].active));
            pushRecord(rec);
            ok++;
          } catch {
            fails.push(items[i].key);
          }
        }
        if (fails.length) {
          throw new Error(`Processed ${ok}/${items.length} · failed: ${fails.join(", ")}`);
        }
        return null;
      },
    });
    batchMsg =
      res.status === "done"
        ? `Processed ${items.length}/${items.length}` +
          (skipped ? ` · ${skipped} skipped (not drawn)` : "")
        : (res.error ?? "");
  }

  // ── Blueprint / publish / seed ──
  async function saveBlueprint() {
    saving = true;
    error = "";
    try {
      const snap = $state.snapshot(config);
      if (savedId) {
        await unwrap(commands.saveProjectConfig(snap));
      } else {
        await unwrap(commands.createProject(snap));
        savedId = snap.gameId;
        gameCreated(snap.gameId);
        await loadRecords();
      }
      blueprintOpen = false;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function publish(pickNew: boolean, keys: string[] = []) {
    if (!savedId) return;
    let dir = publishDir;
    if (pickNew || !dir) {
      const sel = await open({ directory: true, defaultPath: dir ?? undefined });
      const chosen = Array.isArray(sel) ? sel[0] : sel;
      if (!chosen) return;
      dir = chosen;
    }
    if (!dir) return;
    publishing = true;
    error = "";
    batchMsg = "";
    try {
      const rep = await unwrap(commands.publishFinals(savedId, dir, keys));
      publishDir = rep.dest;
      const scope = keys.length ? "selected " : "";
      batchMsg =
        `Published ${rep.copied.length} ${scope}asset${rep.copied.length === 1 ? "" : "s"} → ${rep.dest}` +
        (rep.skipped.length && !keys.length ? ` · ${rep.skipped.length} not ready` : "");
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      publishing = false;
    }
  }

  let seeding = $state(false);
  async function seedAll() {
    if (!savedId) return;
    seeding = true;
    batchMsg = "";
    error = "";
    try {
      const done = await unwrap(commands.seedSubjects(savedId));
      batchMsg = `${done} subject${done === 1 ? "" : "s"} seeded from the Blueprint.`;
      await loadRecords();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      seeding = false;
    }
  }

  function closeMenus() {
    menuOpen = false;
    legendOpen = false;
  }
</script>

<svelte:window onclick={closeMenus} />

<div class="producer">
  <header class="phead">
    <div class="left">
      <button class="ghost back" onclick={goProjects}>‹ Library</button>
      <h1 class="display gname">{config.name || "New game"}</h1>

      {#if savedId}
        <div class="gauge-wrap">
          <button
            class="gauge"
            title="pipeline progress — click for the legend"
            onclick={(e) => {
              e.stopPropagation();
              legendOpen = !legendOpen;
            }}
          >
            <span class="bar"><span class="fill" style:width={`${progress.pct * 100}%`}></span></span>
            <span class="mono gnums"><em>{progress.ready}</em>/{progress.total} ready</span>
          </button>
          {#if legendOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div class="legend card" onclick={(e) => e.stopPropagation()}>
              <span class="u-label">Asset states</span>
              <div class="lg-row"><span class="st"></span> not started</div>
              <div class="lg-row"><span class="st st-briefed"></span> briefed — has a subject/prompt</div>
              <div class="lg-row"><span class="st st-drawn"></span> drawn — has a generated image</div>
              <div class="lg-row"><span class="st st-ready"></span> ready — processed, shippable</div>
              <div class="lg-row"><span class="ltag mono">code</span> shipped from code, nothing to draw</div>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="actions">
      {#if savedId}
        <div class="seg">
          <button class:on={view === "list"} onclick={() => (view = "list")}>Bench</button>
          <button class:on={view === "grid"} onclick={() => (view = "grid")}>Grid</button>
        </div>
      {/if}
      <button onclick={() => (blueprintOpen = true)}>Blueprint</button>
      {#if savedId}
        <button class="gold" onclick={() => (exportOpen = true)}>Export</button>
        <div class="menu-wrap">
          <button
            class="ghost"
            onclick={(e) => {
              e.stopPropagation();
              menuOpen = !menuOpen;
            }}
          >
            ⋯
          </button>
          {#if menuOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div class="menu card" onclick={(e) => e.stopPropagation()}>
              <button class="mitem" onclick={() => { menuOpen = false; anchorOpen = true; }}>
                Style anchor…
              </button>
              <button class="mitem" onclick={() => { menuOpen = false; contactOpen = true; }}>
                Contact sheet…
              </button>
              <button class="mitem" onclick={() => { menuOpen = false; batchMsg = ""; error = ""; setKeys = []; }}>
                Generate symbol set…
              </button>
              <button class="mitem" onclick={() => { menuOpen = false; seedAll(); }} disabled={seeding || batchBusy}>
                Seed subjects from Blueprint
              </button>
              <button class="mitem" onclick={() => { menuOpen = false; publish(false); }} disabled={publishing}>
                {publishing ? "Publishing…" : publishDir ? "Publish ready assets" : "Publish ready assets…"}
              </button>
              {#if publishDir}
                <button class="mitem" onclick={() => { menuOpen = false; publish(true); }} disabled={publishing}>
                  Change publish folder…
                </button>
                <span class="mpath mono">{publishDir}</span>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </header>

  {#if error || deriveError || batchMsg || batchProg}
    <div class="banner" class:err={!!(error || deriveError)}>
      {error || deriveError || batchProg || batchMsg}
    </div>
  {/if}

  {#if ready && savedId}
    {#if view === "grid"}
      <Ledger
        gameId={savedId}
        {assets}
        {records}
        {config}
        view="grid"
        selectedKey={assetKey}
        bind:checked
        onselect={pickAsset}
        ongenerateset={(keys) => { batchMsg = ""; error = ""; setKeys = keys; }}
      />
    {:else}
      <div class="work">
        <div class="lcol">
          <Ledger
            gameId={savedId}
            {assets}
            {records}
            {config}
            view="list"
            selectedKey={assetKey}
            bind:checked
            onselect={pickAsset}
            ongenerateset={(keys) => { batchMsg = ""; error = ""; setKeys = keys; }}
          />
        </div>
        {#if descriptor && isRaster}
          {#key descriptor.key}
            <Bench gameId={savedId} asset={descriptor} {config} {anchorRev} onsaved={onBenchSaved} />
          {/key}
        {:else if descriptor}
          <div class="hollow">
            <p class="u-label">Shipped from code</p>
            <p class="muted small">
              <span class="mono">{descriptor.key}</span> is drawn by the game engine at runtime —
              there is no image to author. It's included automatically when you Export.
            </p>
          </div>
        {:else}
          <div class="hollow">
            <p class="u-label">No asset selected</p>
            <p class="muted small">
              Pick an asset from the ledger to work on it — or tick several checkboxes for
              batch actions.
            </p>
          </div>
        {/if}
      </div>
    {/if}
  {:else if ready}
    <div class="hollow tall">
      <p class="u-label">New game</p>
      <p class="muted small">Fill in the Blueprint to create the game and derive its asset list.</p>
    </div>
  {/if}

  {#if checked.length && !setKeys}
    <BatchBar
      count={checked.length}
      busy={batchBusy || publishing}
      progress={batchProg}
      symbolCount={batchTargets((a) => a.kind === "symbol").length}
      providers={providersList}
      bind:providerId={batchProviderId}
      onproviderchange={rememberBatchProvider}
      ongenerate={batchGenerate}
      ongenerateset={batchGenerateSet}
      onprocess={batchProcess}
      onpublish={() => publish(false, [...checked])}
      onclear={() => (checked = [])}
    />
  {/if}

  {#if blueprintOpen}
    <div
      class="overlay"
      role="presentation"
      onclick={(e) => e.target === e.currentTarget && savedId && (blueprintOpen = false)}
    >
      <div class="sheet rise" role="dialog" aria-modal="true" aria-label="Blueprint">
        <div class="sheet-head">
          <div>
            <span class="u-label">Blueprint</span>
            <h2 class="display sheet-title">{config.name || "New game"}</h2>
          </div>
          <span class="derived mono">{assets.length} assets derived</span>
        </div>
        <nav class="bp-tabs" aria-label="Blueprint section">
          {#each BP_TABS as t (t.key)}
            <button class="bp-tab" class:on={bpTab === t.key} onclick={() => (bpTab = t.key)}>
              {t.label}
            </button>
          {/each}
        </nav>
        <div class="sheet-body">
          {#if bpTab === "assistant"}
            <BlueprintPorter bind:config />
          {:else}
            <ConfigForm bind:config idLocked={savedId !== null} section={bpTab} />
          {/if}
        </div>
        <div class="sheet-foot">
          {#if savedId}
            <button class="ghost" onclick={() => (blueprintOpen = false)}>Close</button>
          {:else}
            <button class="ghost" onclick={goProjects}>Cancel</button>
          {/if}
          <button class="gold" onclick={saveBlueprint} disabled={saving || !config.gameId}>
            {saving ? "Saving…" : savedId ? "Save blueprint" : "Create game"}
          </button>
        </div>
      </div>
    </div>
  {/if}

  {#if exportOpen && savedId}
    <ExportModal gameId={savedId} onclose={() => (exportOpen = false)} />
  {/if}

  {#if anchorOpen && savedId}
    <StyleAnchor
      gameId={savedId}
      onclose={(changed) => {
        anchorOpen = false;
        if (changed) anchorRev++;
      }}
    />
  {/if}

{#if contactOpen && savedId}
  <ContactSheetModal gameId={savedId} {config} onclose={() => (contactOpen = false)} />
{/if}

{#if setKeys && savedId}
  <SetComposer
    gameId={savedId}
    assetKeys={setKeys}
    candidates={setCandidates}
    providers={providersList}
    initialProvider={providersList.some((p) => p.id === batchProviderId && p.configured)
      ? batchProviderId
      : (providersList.find((p) => p.configured)?.id ?? "")}
    onclose={() => (setKeys = null)}
    oncommitted={setCommitted}
  />
{/if}
</div>

<style>
  .producer {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    position: relative;
  }
  .phead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.7rem 1.2rem;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .left {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    min-width: 0;
  }
  .gname {
    font-size: 1.05rem;
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .gauge-wrap {
    position: relative;
  }
  .gauge {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: none;
    padding: 0.2rem 0.3rem;
  }
  .bar {
    width: 110px;
    height: 4px;
    background: var(--line);
    border-radius: 2px;
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--gold);
    transition: width 300ms var(--ease-out);
  }
  .gnums {
    font-size: 0.64rem;
    color: var(--ash);
  }
  .gnums em {
    font-style: normal;
    color: var(--gold);
  }
  .legend {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 40;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.7rem 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-width: 240px;
  }
  .lg-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.72rem;
    color: var(--bone-dim);
  }
  .st {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    border: 1.5px solid var(--ash-deep);
    flex: none;
  }
  .st-briefed {
    border-color: var(--ash);
    background: rgba(166, 170, 181, 0.35);
  }
  .st-drawn {
    border-color: var(--bone-dim);
    background: var(--bone-dim);
  }
  .st-ready {
    border-color: var(--gold);
    background: var(--gold);
  }
  .ltag {
    font-size: 0.58rem;
    color: var(--ash-deep);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 0.35rem;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex: none;
  }
  .seg {
    display: flex;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .seg button {
    border: none;
    border-radius: 0;
    background: transparent;
    font-size: 0.72rem;
    padding: 0.3rem 0.7rem;
    color: var(--ash);
  }
  .seg button.on {
    background: var(--wash);
    color: var(--bone);
  }
  .menu-wrap {
    position: relative;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 40;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.4rem;
    display: flex;
    flex-direction: column;
    min-width: 230px;
  }
  .mitem {
    background: transparent;
    border: none;
    text-align: left;
    padding: 0.45rem 0.6rem;
    font-size: 0.76rem;
    color: var(--bone-dim);
    border-radius: var(--radius-sm);
  }
  .mitem:hover:not(:disabled) {
    background: var(--wash);
    color: var(--bone);
  }
  .mpath {
    font-size: 0.6rem;
    color: var(--ash-deep);
    padding: 0.2rem 0.6rem 0.3rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .banner {
    padding: 0.4rem 1.2rem;
    font-size: 0.74rem;
    color: var(--lapis);
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .banner.err {
    color: var(--oxblood);
  }
  .work {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 232px 1fr;
  }
  .lcol {
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .hollow {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 2rem;
    text-align: center;
  }
  .hollow.tall {
    min-height: 40vh;
  }

  /* ── Blueprint overlay ── */
  .overlay {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .sheet {
    width: min(920px, 94vw);
    height: min(82vh, 860px);
    display: flex;
    flex-direction: column;
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-3);
  }
  .sheet-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding: var(--space-6) var(--space-6) var(--space-3);
  }
  .sheet-title {
    font-size: 1.2rem;
    margin: var(--space-1) 0 0;
  }
  .derived {
    font-size: var(--text-xs);
    color: var(--ash);
  }
  .bp-tabs {
    display: flex;
    gap: var(--space-6);
    padding: 0 var(--space-6);
    border-bottom: 1px solid var(--line);
  }
  .bp-tab {
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: transparent;
    padding: var(--space-3) var(--space-1) var(--space-4);
    color: var(--ash);
    font-size: var(--text-md);
    transition: color var(--dur-fast) var(--ease);
  }
  .bp-tab:hover {
    color: var(--bone-dim);
    background: transparent;
  }
  .bp-tab.on {
    color: var(--bone);
    border-bottom-color: var(--gold);
  }
  .sheet-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-6);
    display: flex;
    flex-direction: column;
  }
  .sheet-foot {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-6);
    border-top: 1px solid var(--line);
  }
</style>
