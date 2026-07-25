<script lang="ts">
  /**
   * The asset ledger — ONE grouping model (symbols split into High/Low/Special tiers),
   * two densities: `list` (left rail next to the bench) and `grid` (full-width contact
   * sheet for judging the set). Multi-select lives here in both views; batch actions
   * appear in the Producer's bottom bar.
   */
  import { commands, type AssetDescriptor, type AssetRecord, type GameConfig } from "$lib/ipc";
  import { assetStatus } from "$lib/assetStatus";

  let {
    gameId,
    assets,
    records,
    config,
    view,
    selectedKey,
    checked = $bindable([]),
    onselect,
  }: {
    gameId: string | null;
    assets: AssetDescriptor[];
    records: Record<string, AssetRecord>;
    config: GameConfig;
    view: "list" | "grid";
    selectedKey: string | null;
    /** Multi-selection for batch actions (raster assets only). */
    checked?: string[];
    onselect: (key: string) => void;
  } = $props();

  const CATEGORY_LABELS: Record<string, string> = {
    symbols: "Symbols",
    mascot: "Mascot",
    backgrounds: "Backgrounds",
    reel_chrome: "Reel chrome",
    symbol_chrome: "Symbol chrome",
    panels: "Panels",
    buy_bonus: "Buy bonus",
    payline: "Paylines",
    branding: "Branding",
    splash: "Splash",
  };
  const CATEGORY_ORDER = [
    "symbols",
    "mascot",
    "backgrounds",
    "reel_chrome",
    "symbol_chrome",
    "panels",
    "buy_bonus",
    "payline",
    "branding",
    "splash",
  ];

  const symbolRole = $derived.by(() => {
    const m = new Map<string, string>();
    for (const s of config.symbols ?? []) {
      m.set(`symbol_${s.key}`, s.role);
      // Expanding wilds derive a second full-column asset — same tier group.
      if (s.role === "expandingWild") m.set(`symbol_${s.key}_expanded`, s.role);
    }
    return m;
  });

  function mkGroup(key: string, label: string, items: AssetDescriptor[]) {
    return {
      key,
      label,
      items,
      ready: items.filter((a) => a.production === "raster" && assetStatus(records[a.key]).ready)
        .length,
      rasterTotal: items.filter((a) => a.production === "raster").length,
    };
  }

  /** THE grouping — categories in canonical order, symbols split into tiers. */
  const groups = $derived.by(() => {
    const byCat = new Map<string, AssetDescriptor[]>();
    for (const a of assets) {
      const arr = byCat.get(a.category) ?? [];
      arr.push(a);
      byCat.set(a.category, arr);
    }
    const cats = [...byCat.entries()].sort((a, b) => {
      const ia = CATEGORY_ORDER.indexOf(a[0]);
      const ib = CATEGORY_ORDER.indexOf(b[0]);
      return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
    });

    const TIERS = [
      { key: "symbols_high", label: "High symbols", roles: ["high"] },
      { key: "symbols_low", label: "Low symbols", roles: ["low"] },
      { key: "symbols_special", label: "Special symbols", roles: ["wild", "expandingWild", "scatter", "bonus", "special"] },
    ];
    const out: ReturnType<typeof mkGroup>[] = [];
    for (const [key, items] of cats) {
      if (key !== "symbols") {
        out.push(mkGroup(key, CATEGORY_LABELS[key] ?? key, items));
        continue;
      }
      const seen = new Set<string>();
      for (const t of TIERS) {
        const tierItems = items.filter((a) => t.roles.includes(symbolRole.get(a.key) ?? ""));
        tierItems.forEach((a) => seen.add(a.key));
        if (tierItems.length) out.push(mkGroup(t.key, t.label, tierItems));
      }
      const rest = items.filter((a) => !seen.has(a.key));
      if (rest.length) out.push(mkGroup("symbols_other", "Other symbols", rest));
    }
    return out;
  });

  function stateOf(a: AssetDescriptor): "empty" | "briefed" | "drawn" | "ready" {
    if (a.production !== "raster") return "empty";
    const s = assetStatus(records[a.key]);
    if (s.ready) return "ready";
    if (s.generated) return "drawn";
    if (s.hasPrompt) return "briefed";
    return "empty";
  }

  /** Mass-starved: the extent clamp stopped this symbol short of its tier's mass
   *  target — the artwork's aspect ratio is the limiter (re-pose candidate). */
  function massStarved(key: string): string | null {
    const rec = records[key];
    const v = rec?.variations?.find((x) => x.id === rec.activeVariation);
    const m = v?.massReport;
    if (!m?.clamped) return null;
    const got = Math.round((m.achieved ?? 0) * 100);
    const want = Math.round((m.target ?? 0) * 100);
    return `mass-starved: reached ${got}% of the ${want}% target — the aspect ratio caps it; consider a re-pose/re-angle regeneration`;
  }

  function shortName(a: AssetDescriptor): string {
    return (
      a.key.replace(new RegExp(`^${a.category.replace(/s$/, "")}_?`), "").replace(/^_/, "") ||
      a.key
    );
  }

  function toggleCheck(key: string) {
    checked = checked.includes(key) ? checked.filter((k) => k !== key) : [...checked, key];
  }

  // Grid thumbnails: active variation's processed webp, else raw. Keyed by variation so
  // regenerated assets re-fetch. Only loads while the grid is showing.
  let thumbs = $state<Record<string, string>>({});
  const requested = new Set<string>();
  $effect(() => {
    if (view !== "grid" || !gameId) return;
    const gid = gameId;
    for (const a of assets) {
      if (a.production !== "raster") continue;
      const rec = records[a.key];
      const act = rec?.variations?.find((v) => v.id === rec?.activeVariation);
      if (!act) continue;
      const tag = `${a.key}:${act.id}`;
      if (requested.has(tag)) continue;
      requested.add(tag);
      const hasWebp = act.stages?.some((s) => s.name === "webp");
      const p = hasWebp
        ? commands.getVariationStageImage(gid, a.key, act.id, "webp")
        : commands.getVariationImage(gid, a.key, act.id);
      p.then((r) => {
        if (r.status === "ok") thumbs = { ...thumbs, [a.key]: r.data };
      });
    }
  });
</script>

{#if view === "list"}
  <nav class="ledger" aria-label="Assets">
    {#each groups as g (g.key)}
      <div class="group">
        <div class="ghead">
          <span class="glabel u-label">{g.label}</span>
          {#if g.rasterTotal}
            <span class="gcount mono"><em>{g.ready}</em>/{g.rasterTotal}</span>
          {/if}
        </div>
        {#each g.items as a (a.key)}
          <div class="lrow-wrap" class:sel={selectedKey === a.key}>
            {#if a.production === "raster"}
              <input
                type="checkbox"
                class="lcheck"
                checked={checked.includes(a.key)}
                onchange={() => toggleCheck(a.key)}
                title="select for batch actions"
              />
            {:else}
              <span class="lcheck-gap"></span>
            {/if}
            <button class="lrow" onclick={() => onselect(a.key)}>
              <span class="st st-{stateOf(a)}"></span>
              <span class="lname mono">{shortName(a)}</span>
              {#if a.production !== "raster"}<span class="ltag mono">code</span>{/if}
              {#if records[a.key]?.templateOnly}<span class="ltag mono" title="template only — never shipped">template</span>{/if}
              {#if massStarved(a.key)}<span class="ltag mono starved" title={massStarved(a.key)}>△ mass</span>{/if}
            </button>
          </div>
        {/each}
      </div>
    {/each}
    {#if !groups.length}
      <p class="muted small pad">No assets yet — set up the Blueprint first.</p>
    {/if}
  </nav>
{:else}
  <div class="sheet-wrap">
    {#each groups as g (g.key)}
      {@const rasterItems = g.items.filter((a) => a.production === "raster")}
      {#if rasterItems.length}
        <div class="srow" style="--cols: {Math.min(rasterItems.length, 6)}">
          <div class="shead">
            <span class="u-label">{g.label}</span>
            <span class="gcount mono"><em>{g.ready}</em>/{g.rasterTotal}</span>
          </div>
          <div class="tiles">
            {#each rasterItems as a (a.key)}
              <div class="tile" class:checked={checked.includes(a.key)}>
                <button class="timg" onclick={() => onselect(a.key)} title={a.key}>
                  {#if thumbs[a.key]}
                    <img src={thumbs[a.key]} alt={a.key} />
                  {:else}
                    <span class="muted tiny">—</span>
                  {/if}
                </button>
                <input
                  type="checkbox"
                  class="tcheck"
                  checked={checked.includes(a.key)}
                  onchange={() => toggleCheck(a.key)}
                />
                <div class="tmeta">
                  <span class="st st-{stateOf(a)}"></span>
                  <span class="tname mono">{shortName(a)}</span>
                  {#if massStarved(a.key)}<span class="ltag mono starved" title={massStarved(a.key)}>△</span>{/if}
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  /* ── Shared status dot (same meanings everywhere; legend on the gauge) ── */
  .st {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: none;
    border: 1.5px solid var(--ash-deep);
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
    box-shadow: 0 0 6px var(--gold-glow);
  }

  /* ── List ── */
  .ledger {
    overflow-y: auto;
    min-height: 0;
    padding: 0.6rem 0.4rem 1rem 0.6rem;
  }
  .group {
    margin-bottom: 0.9rem;
  }
  .ghead {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding: 0 0.5rem 0.25rem;
  }
  .gcount {
    font-size: 0.62rem;
    color: var(--ash-deep);
  }
  .gcount em {
    font-style: normal;
    color: var(--gold);
  }
  .lrow-wrap {
    display: flex;
    align-items: center;
    border-radius: var(--radius-sm);
  }
  .lrow-wrap.sel {
    background: var(--wash);
  }
  .lcheck {
    margin: 0 0 0 0.35rem;
    opacity: 0;
    transition: opacity 100ms var(--ease);
  }
  .lrow-wrap:hover .lcheck,
  .lcheck:checked {
    opacity: 1;
  }
  .lcheck-gap {
    width: 13px;
    margin-left: 0.35rem;
    flex: none;
  }
  .lrow {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: none;
    padding: 0.3rem 0.5rem;
    text-align: left;
  }
  .lname {
    font-size: 0.74rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lrow-wrap.sel .lname {
    color: var(--bone);
  }
  .ltag {
    font-size: 0.58rem;
    color: var(--ash-deep);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0 0.35rem;
  }
  .ltag.starved {
    color: var(--gold);
    border-color: var(--gold-deep);
  }
  .pad {
    padding: 0.8rem;
  }

  /* ── Grid (contact sheet) ── */
  .sheet-wrap {
    overflow-y: auto;
    min-height: 0;
    padding: 1rem 1.4rem 5rem;
    /* Sections are fit-content cards that FLOW — small groups share a row instead
       of each one hogging a full-width band of mostly empty tracks. */
    display: flex;
    flex-wrap: wrap;
    align-content: flex-start;
    align-items: flex-start;
    gap: 1rem;
  }
  .srow {
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--ink);
    padding: 0.7rem 0.8rem 0.8rem;
    max-width: 100%;
  }
  .shead {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1.2rem;
    margin-bottom: 0.5rem;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(var(--cols, 5), 140px);
    gap: 0.7rem;
  }
  .tile {
    position: relative;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.4rem;
    transition: border-color 120ms var(--ease);
  }
  .tile.checked {
    border-color: var(--gold-deep);
  }
  .timg {
    width: 100%;
    aspect-ratio: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    background:
      repeating-conic-gradient(var(--wash-soft) 0% 25%, transparent 0% 50%)
      0 0 / 16px 16px;
    border: none;
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .timg img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .tcheck {
    position: absolute;
    top: 0.55rem;
    right: 0.55rem;
    opacity: 0;
    transition: opacity 100ms var(--ease);
  }
  .tile:hover .tcheck,
  .tcheck:checked {
    opacity: 1;
  }
  .tmeta {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 0.1rem 0;
  }
  .tname {
    font-size: 0.66rem;
    color: var(--bone-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
