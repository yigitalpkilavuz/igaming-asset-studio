<script lang="ts">
  /**
   * The asset bench, canvas-first: a persistent next-step cue, the big image, the
   * variation filmstrip, and a slim right dock of collapsible sections
   * (Prompt / Generate / Refine / Finish). All authoring for ONE asset lives here.
   *
   * Absorbs every feature of the old AssetWorkbench; per-action busy flags replace the
   * old global `busy` string so unrelated controls stay live.
   */
  import {
    commands,
    unwrap,
    type AiSheet,
    type AssetDescriptor,
    type AssetGlyph,
    type AssetRecord,
    type GameConfig,
    type Preflight,
    type ProviderInfo,
    type QualityReport,
  } from "$lib/ipc";
  import { listen } from "@tauri-apps/api/event";
  import { assetStatus } from "$lib/assetStatus";
  import { goLayers, goStudio, selectAsset } from "$lib/stores/app.svelte";
  import { jobsState, runJob, runningTag } from "$lib/stores/jobs.svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { Eye } from "@lucide/svelte";
  import AlphaEdit from "./AlphaEdit.svelte";
  import DockSection from "./DockSection.svelte";

  let {
    gameId,
    asset,
    config,
    anchorRev = 0,
    onsaved,
  }: {
    gameId: string;
    asset: AssetDescriptor;
    config: GameConfig;
    /** Bumped by the Producer when the style anchor changes. */
    anchorRev?: number;
    onsaved: (record: AssetRecord) => void;
  } = $props();

  const animatable = $derived(asset.kind === "symbol" || asset.kind === "mascot");
  const layerable = $derived(asset.kind === "background");

  // ── Scene assets: variant group, composition guides, provider defaults ───────
  // A scene asset def + the variant this bench asset derives from, if any.
  /** Class prefixes auto-applied to derived scene keys (mirror of the backend rule). */
  const SCENE_PREFIX: Record<string, string> = {
    plate: "bg_",
    layer: "bg_",
    sprite: "bg_",
    fx: "fx_",
    particle: "p_",
  };
  function sceneBaseKey(def: { key: string; kind: string }): string {
    const prefix = SCENE_PREFIX[def.kind] ?? "bg_";
    return def.key.startsWith(prefix) ? def.key : `${prefix}${def.key}`;
  }
  const sceneInfo = $derived.by(() => {
    const sc = config.scene;
    if (!sc?.assets?.length) return null;
    for (const def of sc.assets) {
      const base = sceneBaseKey(def);
      const variants = def.variants?.length
        ? def.variants
        : [{ key: "", preset: "", extraPrompt: "" }];
      for (const v of variants) {
        const key = v.key?.trim() ? `${base}_${v.key.trim()}` : base;
        if (key === asset.key) {
          return { def, presetKey: v.preset?.trim() || sc.presets?.[0]?.key || "" };
        }
      }
    }
    return null;
  });
  /** Guide templates bound to this asset's preset — verification overlays + steering. */
  const guideTemplates = $derived(
    sceneInfo ? (config.scene?.guides ?? []).filter((g) => g.preset === sceneInfo.presetKey) : [],
  );
  /** Overlay toggles (template keys) + the template steering generation ("" = none). */
  let visibleGuides = $state<string[]>([]);
  let guideKey = $state("");
  /** Wrap-seam check: show the image twice side-by-side so the junction (= the wrap
   *  point) sits in the middle of the stage. */
  let seamOn = $state(false);
  /** The Scene popover (variants · guides · seam) — one button instead of chip rows. */
  let sceneViewOpen = $state(false);
  function toggleGuide(key: string) {
    visibleGuides = visibleGuides.includes(key)
      ? visibleGuides.filter((k) => k !== key)
      : [...visibleGuides, key];
  }
  /** Sibling variant keys of this asset's scene group (side-by-side management). */
  const sceneSiblings = $derived.by(() => {
    if (!sceneInfo || !sceneInfo.def.variants?.length) return [] as string[];
    const base = sceneBaseKey(sceneInfo.def);
    return sceneInfo.def.variants.map((v) => (v.key?.trim() ? `${base}_${v.key.trim()}` : base));
  });
  function siblingLabel(key: string): string {
    const base = sceneInfo ? sceneBaseKey(sceneInfo.def) : "";
    return key.startsWith(`${base}_`) ? key.slice(base.length + 1) : key;
  }

  // ── Record + prompt state ────────────────────────────────────────────────────
  let record = $state<AssetRecord | null>(null);
  let subject = $state("");
  let overridePrompt = $state("");
  let negativeOverride = $state("");
  /** Backgrounds: generate with a parallax cut in mind (separable depth planes). */
  let layered = $state(false);
  /** The game has a style anchor — auto-attached to every cloud generation. */
  let hasAnchor = $state(false);
  $effect(() => {
    void anchorRev;
    void jobsState.anchorRev;
    commands.styleAnchorPresent(gameId).then((r) => {
      if (r.status === "ok") hasAnchor = r.data;
    });
  });

  // Merge records written by background jobs for THIS asset (covers finishing while
  // the user is elsewhere, then returning; also updates the ledger via onsaved).
  let seenRev = jobsState.rev;
  $effect(() => {
    void jobsState.rev;
    const fresh = jobsState.recordUpdates.filter(
      (u) => u.rev > seenRev && u.record.key === asset.key,
    );
    seenRev = jobsState.rev;
    const latest = fresh.at(-1);
    if (latest) {
      record = latest.record;
      onsaved(latest.record);
    }
  });

  // ── AI sheet (SpriteCook) ────────────────────────────────────────────────────
  let spritecookReady = $state(false);
  let sheetPrompt = $state("");
  let sheetFrames = $state(8);
  const sheetBusy = $derived(runningTag(asset.key, "sheet"));
  let sheetProgress = $state("");
  let aiSheet = $state<AiSheet | null>(null);
  let sheetCanvas = $state<HTMLCanvasElement | null>(null);

  $effect(() => {
    commands.spritecookKeyPresent().then((ok) => (spritecookReady = ok));
    commands.getAiSheet(gameId, asset.key).then((r) => {
      if (r.status === "ok" && r.data) {
        aiSheet = r.data;
        sheetPrompt = r.data.prompt ?? "";
        sheetFrames = r.data.frames ?? 8;
      } else if (!sheetPrompt) {
        // Seed the motion prompt from the Blueprint's per-symbol animation note.
        const symKey = asset.key.startsWith("symbol_") ? asset.key.slice(7) : null;
        const planned = config.symbols?.find((s) => s.key === symKey)?.animation ?? "";
        if (planned.trim()) sheetPrompt = planned.trim();
      }
    });
    const un = listen<{ progress: number; status: string }>("sheet://progress", (e) => {
      const pct = Math.round((e.payload.progress ?? 0) * (e.payload.progress <= 1 ? 100 : 1));
      sheetProgress = e.payload.status === "queued" ? "queued…" : `${pct}%`;
    });
    return () => {
      un.then((f) => f());
    };
  });

  // ── Glyph on base ────────────────────────────────────────────────────────────
  // One empty template generated ONCE as its own asset (e.g. l_base); this asset picks
  // it, describes its mark, and the mark is AI-generated + embossed on. Same base
  // asset across symbols ⇒ pixel-identical template.
  let glyph = $state<AssetGlyph>({
    baseAsset: "",
    prompt: "",
    glyphId: null,
    transform: { x: 0.5, y: 0.5, scale: 0.42, rotation: 0 },
    params: { depth: 6, highlight: 0.9, shadow: 0.9, fillTone: 0.94 },
  });
  let glyphBaseOptions = $state<string[]>([]);
  let glyphPreviewUrl = $state<string | null>(null);
  let glyphPreviewStale = $state(false);
  const glyphGenBusy = $derived(runningTag(asset.key, "glyphgen"));
  const glyphComposeBusy = $derived(runningTag(asset.key, "glyphcompose"));

  $effect(() => {
    commands.glyphGet(gameId, asset.key).then((r) => {
      if (r.status === "ok" && r.data) {
        glyph = r.data;
        schedGlyphPreview();
      }
    });
    commands.deriveAssets(config).then((list) => {
      glyphBaseOptions = list.filter((a) => a.key !== asset.key).map((a) => a.key);
    });
  });

  let glyphSaveTimer: ReturnType<typeof setTimeout> | null = null;
  function persistGlyph() {
    if (glyphSaveTimer) clearTimeout(glyphSaveTimer);
    glyphSaveTimer = setTimeout(() => {
      commands.glyphSave(gameId, asset.key, $state.snapshot(glyph));
    }, 500);
  }

  let glyphPrevTimer: ReturnType<typeof setTimeout> | null = null;
  function schedGlyphPreview() {
    if (!glyph.glyphId || !glyph.baseAsset) return;
    glyphPreviewStale = true;
    if (glyphPrevTimer) clearTimeout(glyphPrevTimer);
    glyphPrevTimer = setTimeout(async () => {
      try {
        const p = await unwrap(commands.glyphPreview(gameId, $state.snapshot(glyph)));
        glyphPreviewUrl = p.image;
        glyphPreviewStale = false;
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }, 350);
  }

  async function generateGlyphShape() {
    error = "";
    const snap = $state.snapshot(glyph);
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "generate",
      tag: "glyphgen",
      label: `mark · ${asset.key}`,
      exec: async () => {
        const info = await unwrap(
          commands.glyphGenerate(snap.prompt.trim(), providerId, snap.glyphId ?? ""),
        );
        glyph.glyphId = info.id;
        persistGlyph();
        schedGlyphPreview();
        return null;
      },
    });
    if (res.error) error = res.error;
  }

  async function composeGlyph() {
    error = "";
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "process",
      tag: "glyphcompose",
      label: `compose · ${asset.key}`,
      exec: async () => unwrap(commands.glyphCompose(gameId, asset.key, $state.snapshot(glyph))),
    });
    if (res.error) error = res.error;
  }

  // Drag on the preview to move the mark (fractions of the base canvas).
  let glyphDragging = false;
  function glyphPointAt(e: PointerEvent) {
    const img = (e.currentTarget as HTMLElement).querySelector("img");
    if (!img) return;
    const r = img.getBoundingClientRect();
    glyph.transform.x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    glyph.transform.y = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
  }
  function glyphDragStart(e: PointerEvent) {
    glyphDragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    glyphPointAt(e);
  }
  function glyphDragMove(e: PointerEvent) {
    if (glyphDragging) glyphPointAt(e);
  }
  function glyphDragEnd() {
    if (!glyphDragging) return;
    glyphDragging = false;
    persistGlyph();
    schedGlyphPreview();
  }

  // Manual alpha erase on the selected take (interior cut-outs, backdrop remnants).
  let alphaEditOpen = $state(false);

  // ── Quality audit: standards every shipped take should meet ─────────────────
  let quality = $state<QualityReport | null>(null);
  let qualityBusy = $state(false);
  let qualityTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    const varId = activeVar?.id;
    const stageCount = activeVar?.stages?.length ?? 0;
    void stageCount; // re-audit after Process adds stages
    quality = null;
    if (!varId) return;
    if (qualityTimer) clearTimeout(qualityTimer);
    qualityTimer = setTimeout(async () => {
      qualityBusy = true;
      try {
        quality = await unwrap(commands.assetQuality(gameId, asset.key, varId));
      } catch {
        /* audit is advisory — never block the bench */
      } finally {
        qualityBusy = false;
      }
    }, 350);
  });

  /** Which take-action panel is open under the filmstrip (null = none). */
  let takeAction = $state<null | "refine" | "upscale" | "glyph" | "sheet">(null);
  function toggleTake(a: NonNullable<typeof takeAction>) {
    takeAction = takeAction === a ? null : a;
  }

  async function toggleTemplateOnly(e: Event) {
    const on = (e.currentTarget as HTMLInputElement).checked;
    try {
      const rec = await unwrap(commands.setTemplateOnly(gameId, asset.key, on));
      record = rec;
      onsaved(rec);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function runAiSheet() {
    error = "";
    const prompt = sheetPrompt;
    const frames = sheetFrames;
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "sheet",
      label: `AI sheet · ${asset.key}`,
      exec: async () => {
        const sheet = await unwrap(commands.generateAiSheet(gameId, asset.key, prompt, frames));
        // Not an AssetRecord — deliver directly; a remounted bench reloads it from disk.
        aiSheet = sheet;
        return null;
      },
    });
    if (res.error) error = res.error;
  }

  // Seamless-loop check: mean |first frame − last frame| as a percentage. A clean
  // loop's last frame flows into the first, so a large diff means a visible pop.
  let sheetSeamPct = $state<number | null>(null);
  $effect(() => {
    const sheet = aiSheet;
    sheetSeamPct = null;
    if (!sheet || (sheet.frames ?? 0) < 2) return;
    const img = new Image();
    img.onload = () => {
      const fw = Math.max(1, Math.floor(sheet.width / Math.max(1, sheet.frames)));
      const c = document.createElement("canvas");
      const s = Math.min(1, 128 / sheet.height);
      c.width = Math.max(1, Math.round(fw * s));
      c.height = Math.max(1, Math.round(sheet.height * s));
      const ctx = c.getContext("2d", { willReadFrequently: true });
      if (!ctx) return;
      const grab = (frame: number) => {
        ctx.clearRect(0, 0, c.width, c.height);
        ctx.drawImage(img, frame * fw, 0, fw, sheet.height, 0, 0, c.width, c.height);
        return ctx.getImageData(0, 0, c.width, c.height).data;
      };
      const a = grab(0);
      const b = grab(sheet.frames - 1);
      let sum = 0;
      for (let i = 0; i < a.length; i += 4) {
        sum += Math.abs(a[i] - b[i]) + Math.abs(a[i + 1] - b[i + 1]) + Math.abs(a[i + 2] - b[i + 2]);
      }
      sheetSeamPct = (sum / ((a.length / 4) * 3 * 255)) * 100;
    };
    img.src = sheet.dataUrl;
  });

  // Animated preview: step through the sheet's frames on a small canvas (~10 fps).
  $effect(() => {
    const sheet = aiSheet;
    const canvas = sheetCanvas;
    if (!sheet || !canvas) return;
    const img = new Image();
    img.src = sheet.dataUrl;
    let raf = 0;
    let last = 0;
    let frame = 0;
    const fw = Math.max(1, Math.floor(sheet.width / Math.max(1, sheet.frames)));
    const tick = (t: number) => {
      raf = requestAnimationFrame(tick);
      if (!img.complete || t - last < 100) return;
      last = t;
      frame = (frame + 1) % Math.max(1, sheet.frames);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      const scale = Math.min(1, 160 / sheet.height);
      canvas.width = Math.max(1, Math.round(fw * scale));
      canvas.height = Math.max(1, Math.round(sheet.height * scale));
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(
        img,
        frame * fw,
        0,
        fw,
        sheet.height,
        0,
        0,
        canvas.width,
        canvas.height,
      );
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });
  let loading = $state(true);
  let error = $state("");

  // Per-action busy (the old single `busy` string froze every button at once).
  let saving = $state(false);
  let importing = $state(false);
  let histBusy = $state(false); // lock/delete on the filmstrip
  // Long ops run as global background jobs (survive navigation); busy states derive
  // from the job store so a remounted bench still shows what's in flight.
  const generating = $derived(runningTag(asset.key, "generate"));
  const refining = $derived(runningTag(asset.key, "refine"));
  const finishing = $derived(
    runningTag(asset.key, "proc")
      ? "proc"
      : runningTag(asset.key, "upai")
        ? "upai"
        : runningTag(asset.key, "upfree")
          ? "upfree"
          : importing
            ? "import"
            : "",
  );

  // Per-class / per-asset provider defaults: scene assets → the asset's own provider
  // or the scene-class default; symbols → the symbol-class default. Bench selection
  // still wins for anything unconfigured.
  $effect(() => {
    void asset.key;
    const wanted = sceneInfo
      ? sceneInfo.def.provider?.trim() || config.scene?.defaultProvider?.trim() || ""
      : asset.kind === "symbol"
        ? (config.symbolProvider ?? "").trim()
        : "";
    if (wanted && providers.some((p) => p.id === wanted && p.configured)) {
      providerId = wanted;
    }
  });

  $effect(() => {
    const key = asset.key;
    loading = true;
    error = "";
    lastGuide = undefined; // re-arm the guide for the new asset
    visibleGuides = [];
    guideKey = "";
    takeAction = null;
    seamOn = false;
    sceneViewOpen = false;
    (async () => {
      try {
        const r = await unwrap(commands.getAssetRecord(gameId, key));
        record = r;
        subject = r.prompt.subject ?? "";
        overridePrompt = r.prompt.overridePrompt ?? "";
        negativeOverride = r.prompt.negativeOverride ?? "";
        layered = r.prompt.layered ?? false;
      } catch (e) {
        error = String(e);
      } finally {
        loading = false;
      }
    })();
  });

  const dirty = $derived(
    record !== null &&
      (subject !== (record.prompt.subject ?? "") ||
        overridePrompt !== (record.prompt.overridePrompt ?? "") ||
        negativeOverride !== (record.prompt.negativeOverride ?? "") ||
        layered !== (record.prompt.layered ?? false)),
  );
  const status = $derived(assetStatus(record ?? undefined));
  const activeVar = $derived(status.activeVariation);

  // ── Guided accordion ──────────────────────────────────────────────────────────
  // ONE dock section open at a time, and the section matching the asset's NEXT step
  // opens itself (no prompt → Prompt; briefed → Generate; drawn → Finish; ready →
  // everything collapsed, the ✓s tell the story). Manual toggles are respected until
  // the next state transition. Secondary tools live under "More".
  type DockSec = "prompt" | "generate" | "glyph" | "refine" | "finish" | "sheet" | "quality";
  let openSec = $state<DockSec | null>("prompt");
  function toggleSec(id: DockSec) {
    openSec = openSec === id ? null : id;
  }
  const guideStep = $derived<DockSec | null>(
    loading || !record
      ? null
      : !status.hasPrompt && !(record.variations?.length ?? 0)
        ? "prompt"
        : !status.generated
          ? "generate"
          : !status.ready
            ? "finish"
            : null,
  );
  let lastGuide: DockSec | null | undefined = undefined;
  $effect(() => {
    const g = guideStep;
    if (loading || g === lastGuide) return;
    lastGuide = g;
    openSec = g;
  });
  // Live prompt assembly result (declared here so the generate gate below can read it).
  let assembledPositive = $state("");
  let assembledNegative = $state("");
  // Generation is possible when the ASSEMBLED prompt is non-empty — assembly falls back
  // to the Blueprint's per-symbol description, so a JSON-created game can generate
  // immediately without hand-typing subjects.
  const canGenerate = $derived(
    !!(subject.trim() || overridePrompt.trim() || assembledPositive.trim()),
  );
  const hasVariations = $derived((record?.variations?.length ?? 0) > 0);

  /** The built-in cue: always name the next step. */
  const nextStep = $derived.by(() => {
    if (loading) return "";
    if (!canGenerate) return "Write a subject in Prompt, then Generate.";
    if (!hasVariations) return "Generate the first take.";
    if (!status.ready) return "Pick the best take — Reroll or Refine — then Process it in Finish.";
    return animatable ? "Ready — Animate it, or Export ships it as-is." : "Ready — Export ships it.";
  });

  // ── Live prompt assembly ─────────────────────────────────────────────────────
  $effect(() => {
    const snap = $state.snapshot(config);
    const s = subject,
      o = overridePrompt,
      n = negativeOverride,
      l = layered;
    commands.assemblePrompt(snap, asset.key, s, o, n, l).then((r) => {
      if (r.status === "ok") {
        assembledPositive = r.data.positive;
        assembledNegative = r.data.negative;
      }
    });
  });
  const promptText = $derived(overridePrompt.trim() ? overridePrompt : assembledPositive);
  const promptEdited = $derived(!!overridePrompt.trim());

  // ── Providers / preflight ────────────────────────────────────────────────────
  let providers = $state<ProviderInfo[]>([]);
  let providerId = $state("");
  let preflight = $state<Preflight | null>(null);
  /// The provider survives asset switches, view changes and app restarts — re-picking
  /// it every time was a waste of the user's time.
  const PROVIDER_LS = "wf-provider";
  const COUNT_LS = "wf-gen-count";
  let genCount = $state(Number(localStorage.getItem(COUNT_LS)) || 1);
  function rememberCount(n: number) {
    genCount = n;
    localStorage.setItem(COUNT_LS, String(n));
  }
  function rememberProvider() {
    try {
      localStorage.setItem(PROVIDER_LS, providerId);
    } catch {
      /* private mode etc. */
    }
  }
  $effect(() => {
    (async () => {
      try {
        const list = await unwrap(commands.listImageProviders());
        providers = list;
        if (!providerId) {
          const saved = localStorage.getItem(PROVIDER_LS);
          providerId =
            (saved && list.some((p) => p.id === saved && p.configured) ? saved : null) ??
            ((list.find((p) => p.configured) ?? list[0])?.id ?? "");
        }
      } catch {
        /* ignore */
      }
    })();
    commands.preflight().then((p) => (preflight = p));
  });

  // ── Style references (cloud providers; Draw Things can't take image inputs) ──
  let refKeys = $state<string[]>([]);
  let refCandidates = $state<{ key: string; varId: string }[]>([]);
  let refThumbs = $state<Record<string, string>>({});
  const refThumbRequested = new Set<string>();
  const MAX_REFS = 4;
  const supportsRefs = $derived(
    providers.find((p) => p.id === providerId)?.supportsRefs ?? false,
  );

  $effect(() => {
    const selfKey = asset.key;
    const gid = gameId;
    (async () => {
      try {
        const recs = await unwrap(commands.listAssetRecords(gid));
        const cands: { key: string; varId: string }[] = [];
        for (const rec of recs) {
          if (rec.key === selfKey) continue;
          const act = rec.activeVariation;
          if (!act) continue;
          const v = rec.variations?.find((x) => x.id === act);
          if (!v) continue;
          cands.push({ key: rec.key, varId: act });
          if (!refThumbRequested.has(rec.key)) {
            refThumbRequested.add(rec.key);
            const hasWebp = v.stages?.some((s) => s.name === "webp");
            const p = hasWebp
              ? commands.getVariationStageImage(gid, rec.key, act, "webp")
              : commands.getVariationImage(gid, rec.key, act);
            p.then((r) => {
              if (r.status === "ok") refThumbs = { ...refThumbs, [rec.key]: r.data };
            });
          }
        }
        refCandidates = cands;
      } catch {
        /* ignore */
      }
    })();
  });

  function toggleRef(key: string) {
    if (refKeys.includes(key)) refKeys = refKeys.filter((k) => k !== key);
    else if (refKeys.length < MAX_REFS) refKeys = [...refKeys, key];
  }

  // ── Images ───────────────────────────────────────────────────────────────────
  let thumbs = $state<Record<string, string>>({});
  let finalUrl = $state<string | null>(null);

  $effect(() => {
    const rec = record;
    if (!rec) return;
    for (const v of rec.variations ?? []) {
      if (!thumbs[v.id]) {
        commands.getVariationImage(gameId, asset.key, v.id).then((r) => {
          if (r.status === "ok") thumbs = { ...thumbs, [v.id]: r.data };
        });
      }
    }
  });
  $effect(() => {
    const v = activeVar;
    finalUrl = null;
    if (v && v.stages?.some((s) => s.name === "webp")) {
      commands.getVariationStageImage(gameId, asset.key, v.id, "webp").then((r) => {
        if (r.status === "ok") finalUrl = r.data;
      });
    }
  });
  const bigPreview = $derived(finalUrl ?? (activeVar ? thumbs[activeVar.id] : null) ?? null);

  // ── A/B compare ──────────────────────────────────────────────────────────────
  let compare = $state(false);
  let cmpA = $state<string | null>(null);
  let cmpB = $state<string | null>(null);
  function toggleCompare() {
    compare = !compare;
    if (compare) {
      cmpA = activeVar?.id ?? null;
      cmpB = null;
    }
  }
  function pickCompare(id: string) {
    if (cmpA === id) {
      cmpA = cmpB;
      cmpB = null;
    } else if (cmpB === id) {
      cmpB = null;
    } else if (!cmpA) {
      cmpA = id;
    } else if (!cmpB) {
      cmpB = id;
    } else {
      cmpA = cmpB;
      cmpB = id;
    }
  }
  function cmpSlot(id: string): "A" | "B" | null {
    return cmpA === id ? "A" : cmpB === id ? "B" : null;
  }

  // ── Actions ──────────────────────────────────────────────────────────────────
  function fail(e: unknown) {
    error = e instanceof Error ? e.message : String(e);
  }

  async function savePrompt() {
    saving = true;
    error = "";
    try {
      const r = await unwrap(
        commands.savePrompt(gameId, asset.key, subject, overridePrompt, negativeOverride, layered),
      );
      record = r;
      onsaved(r);
    } catch (e) {
      fail(e);
    } finally {
      saving = false;
    }
  }

  async function generate() {
    error = "";
    const snap = {
      dirty,
      subject,
      overridePrompt,
      negativeOverride,
      layered,
      providerId,
      refs: supportsRefs ? [...refKeys] : [],
      guideKey,
      count: genCount,
    };
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "generate",
      label: `Generate · ${asset.key}`,
      exec: async () => {
        if (snap.dirty) {
          await unwrap(
            commands.savePrompt(
              gameId, asset.key, snap.subject, snap.overridePrompt, snap.negativeOverride, snap.layered,
            ),
          );
        }
        return await unwrap(
          commands.generateVariation(gameId, asset.key, snap.providerId, snap.refs, snap.guideKey, snap.count),
        );
      },
    });
    if (res.error) error = res.error;
  }

  async function refine() {
    if (!activeVar || !refineText.trim()) return;
    error = "";
    const instruction = refineText.trim();
    const varId = activeVar.id;
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "refine",
      label: `Refine · ${asset.key}`,
      exec: async () =>
        await unwrap(commands.refineVariation(gameId, asset.key, varId, instruction)),
    });
    if (res.status === "done") refineText = "";
    else if (res.error) error = res.error;
  }
  let refineText = $state("");

  async function selectVariation(id: string) {
    try {
      record = await unwrap(commands.setActiveVariation(gameId, asset.key, id));
      onsaved(record);
    } catch (e) {
      fail(e);
    }
  }

  async function removeVariation(id: string) {
    histBusy = true;
    error = "";
    try {
      record = await unwrap(commands.deleteVariation(gameId, asset.key, id));
      const { [id]: _drop, ...rest } = thumbs;
      thumbs = rest;
      if (cmpA === id) cmpA = null;
      if (cmpB === id) cmpB = null;
      onsaved(record);
    } catch (e) {
      fail(e);
    } finally {
      histBusy = false;
    }
  }

  async function toggleLock(id: string, locked: boolean) {
    histBusy = true;
    error = "";
    try {
      record = await unwrap(commands.setVariationLocked(gameId, asset.key, id, !locked));
      onsaved(record);
    } catch (e) {
      fail(e);
    } finally {
      histBusy = false;
    }
  }

  async function runProcess() {
    if (!activeVar) return;
    error = "";
    const varId = activeVar.id;
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "process",
      tag: "proc",
      label: `Process · ${asset.key}`,
      exec: async () => await unwrap(commands.processVariation(gameId, asset.key, varId)),
    });
    if (res.error) error = res.error;
  }

  async function runUpscale(target: number, method: "free" | "api") {
    if (!activeVar) return;
    error = "";
    const varId = activeVar.id;
    const res = await runJob({
      gameId,
      assetKey: asset.key,
      kind: "upscale",
      tag: method === "api" ? "upai" : "upfree",
      label: `${method === "api" ? "AI upscale" : "Upscale"} · ${asset.key}`,
      exec: async () =>
        await unwrap(commands.upscaleVariation(gameId, asset.key, varId, target, method)),
    });
    if (res.error) error = res.error;
  }

  async function importImage() {
    const sel = await open({
      multiple: false,
      filters: [{ name: "Images", extensions: ["png", "webp", "jpg", "jpeg"] }],
    });
    const path = Array.isArray(sel) ? sel[0] : sel;
    if (!path) return;
    importing = true;
    error = "";
    try {
      const rec = await unwrap(commands.importSourceImage(gameId, asset.key, path));
      record = rec;
      onsaved(rec);
    } catch (e) {
      fail(e);
    } finally {
      importing = false;
    }
  }

  // Section summaries (visible while collapsed).
  const promptSummary = $derived(
    subject.trim() ? `“${subject.trim().slice(0, 60)}”` : "no subject yet",
  );
  const providerName = $derived(providers.find((p) => p.id === providerId)?.displayName ?? "");
  const finishSummary = $derived(
    status.ready ? "processed — ready to ship" : hasVariations ? "not processed yet" : "—",
  );
</script>

<svelte:window onclick={() => (sceneViewOpen = false)} />
<div class="bench">
  {#if loading}
    <p class="muted pad">Loading…</p>
  {:else}
    <!-- ── Stage: cue + image + filmstrip ── -->
    <div class="stage">
      <div class="cue">
        <span class="cue-arrow">▸</span>
        <span class="cue-text">{nextStep}</span>
        <span class="spacer"></span>
        {#if sceneSiblings.length > 1 || guideTemplates.length || sceneInfo?.def.wrap}
          <div class="scene-pop-wrap">
            <button
              class="ghost tiny-link scene-eye"
              class:active={sceneViewOpen}
              title="scene view options — variants, guide overlays, seam check"
              onclick={(e) => {
                e.stopPropagation();
                sceneViewOpen = !sceneViewOpen;
              }}
            >
              <Eye size={13} strokeWidth={1.75} />
              <span>Scene</span>
            </button>
            {#if sceneViewOpen}
              <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
              <div class="scene-pop" onclick={(e) => e.stopPropagation()}>
                {#if sceneSiblings.length > 1}
                  <span class="u-label">Variants</span>
                  <div class="sp-row">
                    {#each sceneSiblings as k (k)}
                      <button class="gchip" class:on={k === asset.key} onclick={() => selectAsset(k)}>
                        {siblingLabel(k)}
                      </button>
                    {/each}
                  </div>
                {/if}
                {#if guideTemplates.length}
                  <span class="u-label">Guide overlays</span>
                  <div class="sp-row">
                    {#each guideTemplates as g (g.key)}
                      <button
                        class="gchip"
                        class:on={visibleGuides.includes(g.key)}
                        title="toggle this guide's zones over the preview"
                        onclick={() => toggleGuide(g.key)}
                      >
                        {g.key}
                      </button>
                    {/each}
                  </div>
                {/if}
                {#if sceneInfo?.def.wrap}
                  <span class="u-label">Tiling</span>
                  <div class="sp-row">
                    <button
                      class="gchip"
                      class:on={seamOn}
                      title="show the image tiled twice — the junction in the middle is the wrap seam"
                      onclick={() => (seamOn = !seamOn)}
                    >
                      seam check
                    </button>
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
        {#if (record?.variations?.length ?? 0) > 1}
          <button class="ghost tiny-link" onclick={toggleCompare}>
            {compare ? "exit compare" : "⇄ compare"}
          </button>
        {/if}
      </div>

      {#if compare}
        <div class="cmp">
          {#each [cmpA, cmpB] as slot, i (i)}
            <div class="cmp-pane">
              {#if slot && thumbs[slot]}
                <img src={thumbs[slot]} alt="" />
              {:else}
                <span class="muted small">pick {i === 0 ? "A" : "B"} below</span>
              {/if}
            </div>
          {/each}
        </div>
      {:else}
        <div class="frame" class:has-img={!!bigPreview}>
          {#if bigPreview && seamOn && sceneInfo?.def.wrap}
            <div class="seam-double" title="the middle junction is the wrap seam">
              <img src={bigPreview} alt="" />
              <img src={bigPreview} alt="wrap seam check" />
            </div>
          {:else if bigPreview}
            <div class="stage-wrap" style:--ar={`${asset.authorW ?? 1}/${asset.authorH ?? 1}`}>
              <img src={bigPreview} alt={asset.key} />
              {#each guideTemplates.filter((g) => visibleGuides.includes(g.key)) as g (g.key)}
                {#each g.zones ?? [] as z, i (i)}
                  <div
                    class="gz"
                    style:left={`${z.x ?? 0}%`}
                    style:top={`${z.y ?? 0}%`}
                    style:width={`${z.w ?? 0}%`}
                    style:height={`${z.h ?? 0}%`}
                  >
                    <span class="gz-label mono">{z.label}</span>
                  </div>
                {/each}
              {/each}
            </div>
            {#if status.ready}<span class="ready-tag mono">ready</span>{/if}
          {:else}
            <div class="empty-frame">
              <span class="mono muted">{asset.authorW ?? "?"}×{asset.authorH ?? "?"}</span>
              <span class="muted small">not yet drawn</span>
            </div>
          {/if}
        </div>
      {/if}

      {#if hasVariations}
        <div class="filmstrip">
          {#each record?.variations ?? [] as v (v.id)}
            <div
              class="fs-item"
              class:active={activeVar?.id === v.id && !compare}
              class:cmp-a={cmpSlot(v.id) === "A"}
              class:cmp-b={cmpSlot(v.id) === "B"}
            >
              <button
                class="fs-thumb"
                title={v.promptSnapshot}
                onclick={() => (compare ? pickCompare(v.id) : selectVariation(v.id))}
              >
                {#if thumbs[v.id]}<img src={thumbs[v.id]} alt="" />{/if}
                {#if cmpSlot(v.id)}<span class="fs-slot mono">{cmpSlot(v.id)}</span>{/if}
              </button>
              <div class="fs-ops">
                <button
                  class="ghost fs-op"
                  title={v.locked ? "unlock (allows delete)" : "lock — protect from deletion"}
                  disabled={histBusy}
                  onclick={() => toggleLock(v.id, !!v.locked)}
                >
                  {v.locked ? "★" : "☆"}
                </button>
                {#if !v.locked && !compare}
                  <button
                    class="ghost fs-op del"
                    title="delete this take"
                    disabled={histBusy}
                    onclick={() => removeVariation(v.id)}
                  >
                    ✕
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}

      <!-- ── Take actions: everything you do TO the selected take (✦ = uses AI) ── -->
      <div class="take-bar">
        <span class="tb-label u-label">Take</span>
        <button class="tb-btn" class:on={takeAction === "refine"} disabled={!activeVar} onclick={() => toggleTake("refine")}>✦ Refine</button>
        <button class="tb-btn" class:on={takeAction === "upscale"} disabled={!activeVar} onclick={() => toggleTake("upscale")}>Upscale</button>
        <button class="tb-btn" disabled={!activeVar || !bigPreview} title="paint areas to full transparency — cut-outs, leftover backdrop" onclick={() => (alphaEditOpen = true)}>Edit alpha</button>
        <button class="tb-btn" class:on={takeAction === "glyph"} title="compose a mark onto a shared base template — a new take, pixel-identical base" onclick={() => toggleTake("glyph")}>✦ On base</button>
        {#if status.ready}
          <button class="tb-btn" class:on={takeAction === "sheet"} title="short looping spritesheet from the approved image (SpriteCook)" onclick={() => toggleTake("sheet")}>✦ Motion loop</button>
        {/if}
        <span class="tb-spacer"></span>
        {#if animatable}
          <button class="tb-btn tb-go" disabled={!status.generated} title={status.generated ? "rig + animate in the Animate section" : "generate an image first"} onclick={() => goStudio(gameId, asset.key)}>Animate →</button>
        {/if}
        {#if layerable}
          <button class="tb-btn tb-go" disabled={!status.ready} title={status.ready ? "split into parallax depth layers" : "generate and Process first"} onclick={() => goLayers(gameId, asset.key)}>Layers →</button>
        {/if}
      </div>
      {#if takeAction}
        <div class="take-panel rise">
          {#if takeAction === "refine"}
            <form
              class="refine-form"
              onsubmit={(e) => {
                e.preventDefault();
                refine();
              }}
            >
              <input
                bind:value={refineText}
                placeholder="describe ONE change… e.g. “thinner gold trim”"
                disabled={refining}
              />
              <button type="submit" class="gold" disabled={refining || !refineText.trim() || !activeVar}>
                {refining ? "Refining…" : "Refine"}
              </button>
            </form>
            <p class="muted tiny">
              Edits the selected take, keeps its composition and style — a new take lands in the
              filmstrip. Reroll is the tool for a fresh interpretation.
            </p>
          {:else if takeAction === "upscale"}
            <div class="row2">
              <button onclick={() => runUpscale(1024, "api")} disabled={!!finishing || !activeVar} title="gpt-image-2 recreates the image sharply at high resolution — do it BEFORE Process">
                {finishing === "upai" ? "Upscaling…" : "✦ AI upscale"}
              </button>
              <button onclick={() => runUpscale(1024, "free")} disabled={!!finishing || !activeVar} title="offline upscale ({preflight?.upscale ? 'Real-ESRGAN' : 'Lanczos'})">
                {finishing === "upfree" ? "Upscaling…" : "Free upscale"}
              </button>
            </div>
            <p class="muted tiny">
              Upscale BEFORE Process. ✦ AI redraws sharply at high resolution (paid);
              free is local ({preflight?.upscale ? "Real-ESRGAN" : "Lanczos"}).
            </p>
          {:else if takeAction === "glyph"}
            <p class="muted tiny">
              Generate an empty template ONCE as its own asset (e.g.
              <span class="mono">l_base</span>), pick it here, describe the mark. Every symbol
              using the same base gets a pixel-identical template.
            </p>
            <label class="field">
              <span class="muted tiny">base asset</span>
              <select
                bind:value={glyph.baseAsset}
                onchange={() => {
                  persistGlyph();
                  schedGlyphPreview();
                }}
              >
                <option value="">— pick the shared base —</option>
                {#each glyphBaseOptions as k (k)}
                  <option value={k}>{k}</option>
                {/each}
              </select>
            </label>
            <label class="field">
              <span class="muted tiny">the mark — what to draw on it</span>
              <textarea
                rows="2"
                bind:value={glyph.prompt}
                onchange={persistGlyph}
                placeholder="e.g. the Greek letter omega / a jagged lightning bolt"
              ></textarea>
            </label>
            <button
              onclick={generateGlyphShape}
              disabled={glyphGenBusy || !glyph.prompt.trim() || !glyph.baseAsset || !providerId}
              title={glyph.baseAsset ? "" : "pick a base asset first"}
            >
              {glyphGenBusy ? "Generating…" : glyph.glyphId ? "✦ Regenerate mark" : "✦ Generate mark"}
            </button>
            {#if glyph.glyphId && glyph.baseAsset}
              {#if glyphPreviewUrl}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                  class="glyph-stage"
                  class:stale={glyphPreviewStale}
                  onpointerdown={glyphDragStart}
                  onpointermove={glyphDragMove}
                  onpointerup={glyphDragEnd}
                >
                  <img src={glyphPreviewUrl} alt="mark on base preview" draggable="false" />
                </div>
                <p class="muted tiny">drag the preview to move the mark</p>
              {/if}
              <label class="field">
                <span class="muted tiny">size {(glyph.transform.scale ?? 0.42).toFixed(2)}</span>
                <input
                  type="range"
                  min="0.05"
                  max="0.9"
                  step="0.01"
                  bind:value={glyph.transform.scale}
                  oninput={() => {
                    persistGlyph();
                    schedGlyphPreview();
                  }}
                />
              </label>
              <label class="field">
                <span class="muted tiny">emboss depth {(glyph.params.depth ?? 6).toFixed(0)}px</span>
                <input
                  type="range"
                  min="1"
                  max="24"
                  step="1"
                  bind:value={glyph.params.depth}
                  oninput={() => {
                    persistGlyph();
                    schedGlyphPreview();
                  }}
                />
              </label>
              <button class="gold" onclick={composeGlyph} disabled={glyphComposeBusy}>
                {glyphComposeBusy ? "Composing…" : "Apply as new take"}
              </button>
              <p class="muted tiny">
                Writes the composed image as this asset's new take, plus
                <span class="mono">glyph_mask.png</span> (the mark alone) for win-glow.
              </p>
            {/if}
          {:else if takeAction === "sheet"}
            {#if !spritecookReady}
              <p class="muted tiny">
                Needs a SpriteCook key — add it in Settings. Animates the approved image
                into a looping spritesheet (good for FX and ambient motion; use the
                Animation Studio for real rigs).
              </p>
            {:else}
              <label class="field">
                <span class="muted tiny">motion — what moves and how</span>
                <textarea
                  rows="2"
                  bind:value={sheetPrompt}
                  placeholder="e.g. the flame flickers and sways gently, embers drift upward"
                ></textarea>
              </label>
              <div class="row2">
                <select bind:value={sheetFrames}>
                  <option value={8}>8 frames</option>
                  <option value={12}>12 frames</option>
                  <option value={16}>16 frames</option>
                  <option value={24}>24 frames</option>
                </select>
                <button onclick={runAiSheet} disabled={sheetBusy || !sheetPrompt.trim()}>
                  {sheetBusy
                    ? `Animating… ${sheetProgress}`
                    : aiSheet
                      ? "Redo loop"
                      : "Generate loop"}
                </button>
              </div>
              {#if aiSheet}
                <canvas class="sheet-preview" bind:this={sheetCanvas}></canvas>
                <p class="muted tiny">
                  {aiSheet.frames} frames · {aiSheet.width}×{aiSheet.height} — saved beside
                  the asset as <span class="mono">ai_sheet/sheet.png</span>.
                  {#if sheetSeamPct !== null}
                    <br />
                    <span class:seam-bad={sheetSeamPct > 8}>
                      loop seam: {sheetSeamPct.toFixed(1)}% first↔last diff
                      {sheetSeamPct > 8 ? "— visible pop likely, consider redoing" : "— loops cleanly"}
                    </span>
                  {/if}
                </p>
              {/if}
            {/if}
          {/if}
        </div>
      {/if}

      {#if error}<p class="err">{error}</p>{/if}
    </div>

    <!-- ── Dock ── -->
    <aside class="dock">
      <div class="dock-spec mono">
        {asset.category} · {asset.authorW}×{asset.authorH} · {asset.specRef}
      </div>

      <DockSection title="Prompt" open={openSec === "prompt"} done={status.hasPrompt} ontoggle={() => toggleSec("prompt")} summary={promptSummary} badge={dirty ? "unsaved" : ""}>
        <label class="field">
          <span class="muted tiny">subject — what this asset is</span>
          <textarea rows="2" bind:value={subject} placeholder="e.g. a bronze gorgon medallion"></textarea>
        </label>
        {#if layerable}
          <label class="layered-toggle" title="the prompt asks for cleanly separable depth planes — crisp silhouettes, no blur or haze across planes">
            <input type="checkbox" bind:checked={layered} />
            <span class="tiny">will be cut into parallax layers</span>
          </label>
        {/if}
        <label class="field">
          <span class="muted tiny">
            final prompt {promptEdited ? "(custom)" : "(auto)"}
            {#if promptEdited}
              · <button class="link" onclick={() => (overridePrompt = "")}>reset to auto</button>
            {/if}
          </span>
          <textarea rows="5" value={promptText} oninput={(e) => (overridePrompt = e.currentTarget.value)}></textarea>
        </label>
        <details>
          <summary class="muted tiny">Advanced · negative prompt</summary>
          <textarea rows="2" bind:value={negativeOverride} placeholder="per-asset negatives (added to the master)"></textarea>
          <p class="muted tiny neg-preview">{assembledNegative}</p>
        </details>
        <button onclick={savePrompt} disabled={saving || !dirty}>
          {saving ? "Saving…" : dirty ? "Save prompt" : "Saved"}
        </button>
      </DockSection>

      <DockSection title="Generate" open={openSec === "generate"} done={status.generated} ontoggle={() => toggleSec("generate")} summary={providerName} badge={generating ? "busy" : ""}>
        <label class="field">
          <span class="muted tiny">provider</span>
          <select bind:value={providerId} onchange={rememberProvider}>
            {#each providers as p (p.id)}
              <option value={p.id} disabled={!p.configured}>
                {p.displayName}{p.configured ? "" : " — set up in Settings"}
              </option>
            {/each}
          </select>
        </label>
        {#if hasAnchor && supportsRefs}
          <p class="anchor-chip tiny" title="the game's style anchor rides as the first reference of every generation — manage it via ⋯ → Style anchor">
            ⚓ style anchor attached
          </p>
        {/if}
        {#if guideTemplates.length && supportsRefs}
          <label class="field">
            <span class="muted tiny">layout guide — steers composition around blocked zones</span>
            <select bind:value={guideKey}>
              <option value="">— none —</option>
              {#each guideTemplates as g (g.key)}
                <option value={g.key}>{g.key}</option>
              {/each}
            </select>
          </label>
        {/if}
        {#if supportsRefs && refCandidates.length}
          <div class="field">
            <span class="muted tiny">style references ({refKeys.length}/{MAX_REFS}) — keep the set coherent</span>
            <div class="refs">
              {#each refCandidates as c (c.key)}
                <button
                  class="ref-chip"
                  class:on={refKeys.includes(c.key)}
                  disabled={!refKeys.includes(c.key) && refKeys.length >= MAX_REFS}
                  title={c.key}
                  onclick={() => toggleRef(c.key)}
                >
                  {#if refThumbs[c.key]}<img src={refThumbs[c.key]} alt="" />{/if}
                </button>
              {/each}
            </div>
          </div>
        {/if}
        <div class="gen-row">
          <div class="count-seg" title="how many takes to generate at once — they run in parallel, pick the best from the filmstrip">
            {#each [1, 2, 3, 4] as n (n)}
              <button class:on={genCount === n} onclick={() => rememberCount(n)}>×{n}</button>
            {/each}
          </div>
          <button
            class="gold grow"
            onclick={generate}
            disabled={generating || !canGenerate || !providerId}
            title={canGenerate ? "" : "write a subject first"}
          >
            {generating
              ? "Generating…"
              : (hasVariations ? "Reroll" : "Generate") + (genCount > 1 ? ` ×${genCount}` : "")}
          </button>
        </div>
      </DockSection>

      {#if hasVariations}
        <DockSection title="Finish" open={openSec === "finish"} done={status.ready} ontoggle={() => toggleSec("finish")} summary={finishSummary} badge={finishing ? "busy" : ""}>
          <button onclick={runProcess} disabled={!!finishing || !activeVar}>
            {finishing === "proc" ? "Processing…" : activeVar?.stages?.length ? "Reprocess" : "Process"}
          </button>
          {#if activeVar?.stages?.length}
            <div class="chips">
              {#each activeVar.stages as s (s.name)}<span class="chip mono">{s.name}</span>{/each}
            </div>
          {/if}
          <p class="muted tiny">
            Process removes the background and converts to shippable WebP/PNG — it makes the
            asset <em>ready</em>.
          </p>
        </DockSection>

        <DockSection
          title="Quality"
          open={openSec === "quality"}
          done={!!quality && quality.issues === 0}
          ontoggle={() => toggleSec("quality")}
          summary={quality
            ? quality.issues === 0
              ? "all checks pass"
              : `${quality.issues} issue${quality.issues === 1 ? "" : "s"}`
            : qualityBusy
              ? "auditing…"
              : "audits the shipped pixels"}
          badge={quality && quality.issues > 0 ? String(quality.issues) : ""}
        >
          {#if quality}
            <ul class="qc-list">
              {#each quality.checks as c (c.id)}
                <li class="qc qc-{c.status}">
                  <span class="qc-mark">{c.status === "pass" ? "✓" : c.status === "warn" ? "△" : "✕"}</span>
                  <div class="qc-body">
                    <span class="qc-label">{c.label}</span>
                    <span class="qc-detail muted tiny">{c.detail}</span>
                  </div>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="muted tiny">
              {qualityBusy ? "Auditing the shipped pixels…" : "Runs automatically on the selected take."}
            </p>
          {/if}
        </DockSection>
      {/if}


      <div class="dock-foot">
        <label
          class="tpl-toggle"
          title="e.g. an l_base empty plate other symbols compose onto — excluded from Export and Publish"
        >
          <input
            type="checkbox"
            checked={record?.templateOnly ?? false}
            onchange={toggleTemplateOnly}
          />
          <span class="tiny">template only — never shipped</span>
        </label>
        <button class="ghost tiny-link" onclick={importImage} disabled={finishing === "import"}>
          {finishing === "import" ? "Importing…" : "Import an image instead…"}
        </button>
      </div>
    </aside>
  {/if}
</div>

{#if alphaEditOpen && activeVar && bigPreview}
  <AlphaEdit
    {gameId}
    assetKey={asset.key}
    variationId={activeVar.id}
    imageUrl={bigPreview}
    onclose={() => (alphaEditOpen = false)}
    onapplied={(rec) => {
      record = rec;
      onsaved(rec);
    }}
  />
{/if}

<style>
  .bench {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 320px;
  }
  .pad {
    padding: 1rem;
  }

  /* ── Stage ── */
  .stage {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 0.7rem 1rem 0.8rem;
    gap: 0.6rem;
  }
  .cue {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .cue-arrow {
    color: var(--gold);
  }
  .cue-text {
    font-size: 0.78rem;
    color: var(--bone-dim);
  }
  .spacer {
    flex: 1;
  }
  .tiny-link {
    font-size: 0.7rem;
    padding: 0.15rem 0.4rem;
  }

  .frame {
    flex: 1;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
    background:
      repeating-conic-gradient(var(--wash-soft) 0% 25%, transparent 0% 50%)
      0 0 / 24px 24px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .frame img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  /* Overlay host hugging the image box: aspect-ratio mirrors the author canvas, so
     percent-positioned guide zones align with the artwork. */
  .stage-wrap {
    position: relative;
    aspect-ratio: var(--ar);
    max-width: 100%;
    max-height: 100%;
  }
  .stage-wrap img {
    width: 100%;
    height: 100%;
    display: block;
  }
  .scene-pop-wrap {
    position: relative;
  }
  .scene-eye {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .scene-eye.active {
    color: var(--gold);
  }
  .scene-pop {
    position: absolute;
    top: calc(100% + var(--space-2));
    right: 0;
    z-index: var(--z-bar);
    min-width: 240px;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--elev-2);
  }
  .sp-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .gchip {
    font-size: 0.66rem;
    padding: 0.12rem 0.5rem;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: transparent;
    color: var(--ash-deep);
  }
  .gchip.on {
    color: var(--gold);
    border-color: var(--gold-deep);
  }
  .gz {
    position: absolute;
    border: 1.5px dashed var(--gold);
    background: var(--gold-glow);
    pointer-events: none;
    overflow: hidden;
  }
  .seam-double {
    display: flex;
    max-width: 100%;
    max-height: 100%;
    overflow: hidden;
  }
  .seam-double img {
    width: 50%;
    min-width: 0;
    object-fit: contain;
    display: block;
  }
  .seam-bad {
    color: var(--gold);
  }
  .gz-label {
    position: absolute;
    top: 1px;
    left: 3px;
    font-size: 0.58rem;
    color: var(--gold-bright);
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
    white-space: nowrap;
  }
  .ready-tag {
    position: absolute;
    top: 0.5rem;
    right: 0.6rem;
    font-size: 0.62rem;
    color: var(--gold-bright);
    border: 1px solid var(--gold-deep);
    border-radius: 999px;
    padding: 0.1rem 0.5rem;
    background: rgba(12, 13, 16, 0.7);
  }
  .empty-frame {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.3rem;
  }

  .cmp {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.6rem;
  }
  .cmp-pane {
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .cmp-pane img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }

  .filmstrip {
    display: flex;
    gap: 0.5rem;
    overflow-x: auto;
    padding-bottom: 0.2rem;
    flex: none;
  }
  .fs-item {
    position: relative;
    flex: none;
  }
  .fs-thumb {
    width: 74px;
    height: 74px;
    padding: 2px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: transparent;
    overflow: hidden;
  }
  .fs-item.active .fs-thumb {
    border-color: var(--gold);
  }
  .fs-item.cmp-a .fs-thumb {
    border-color: var(--lapis);
  }
  .fs-item.cmp-b .fs-thumb {
    border-color: var(--sage);
  }
  .fs-thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .fs-slot {
    position: absolute;
    top: 3px;
    left: 5px;
    font-size: 0.62rem;
    color: var(--bone);
  }
  .fs-ops {
    position: absolute;
    top: 2px;
    right: 2px;
    display: flex;
    opacity: 0;
    transition: opacity 120ms var(--ease);
  }
  .fs-item:hover .fs-ops {
    opacity: 1;
  }
  .fs-op {
    padding: 0 0.2rem;
    font-size: 0.7rem;
  }
  .fs-op.del:hover {
    color: var(--oxblood);
  }
  .err {
    color: var(--oxblood);
    font-size: 0.72rem;
    margin: 0;
    flex: none;
  }

  /* ── Dock ── */
  .dock {
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
  }
  .dock-spec {
    padding: 0.55rem 0.9rem;
    font-size: 0.62rem;
    color: var(--ash);
    border-bottom: 1px solid var(--line);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .field textarea,
  .field select {
    width: 100%;
    font-size: 0.74rem;
  }
  .layered-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--bone-dim);
  }
  .gen-row {
    display: flex;
    gap: var(--space-2);
    align-items: stretch;
  }
  .gen-row .grow {
    flex: 1;
  }
  .count-seg {
    display: flex;
    border: 1px solid var(--line);
    border-radius: var(--radius-1, 4px);
    overflow: hidden;
  }
  .count-seg button {
    border: none;
    border-radius: 0;
    background: none;
    color: var(--bone-dim);
    font-family: var(--font-mono);
    font-size: 0.7rem;
    padding: 0 var(--space-2);
    cursor: pointer;
  }
  .count-seg button + button {
    border-left: 1px solid var(--line);
  }
  .count-seg button.on {
    background: var(--ink-2);
    color: var(--bone);
  }
  .anchor-chip {
    margin: 0;
    color: var(--gold-deep);
  }
  .sheet-preview {
    align-self: center;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background:
      repeating-conic-gradient(var(--wash-soft) 0% 25%, transparent 0% 50%)
      0 0 / 12px 12px;
  }
  /* ── Take-action bar: everything you do TO the selected take ── */
  .take-bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) 0 0;
    border-top: 1px solid var(--line);
    margin-top: var(--space-3);
  }
  .tb-label {
    margin-right: var(--space-2);
  }
  .tb-btn {
    font-size: var(--text-sm);
    padding: var(--space-2) var(--space-4);
    background: transparent;
    border: 1px solid transparent;
    color: var(--bone-dim);
  }
  .tb-btn:hover:not(:disabled) {
    background: var(--wash);
    color: var(--bone);
  }
  .tb-btn.on {
    color: var(--gold);
    border-color: var(--gold-deep);
    background: var(--gold-glow);
  }
  .tb-btn:disabled {
    color: var(--ash-deep);
  }
  .tb-spacer {
    flex: 1;
  }
  .tb-go {
    color: var(--bone-dim);
    border-color: var(--line);
  }
  .take-panel {
    margin-top: var(--space-3);
    padding: var(--space-4);
    background: var(--ink-2);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--elev-1);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    max-height: 40vh;
    overflow-y: auto;
  }
  .glyph-stage {
    touch-action: none;
    cursor: crosshair;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background:
      repeating-conic-gradient(var(--wash-soft) 0% 25%, transparent 0% 50%)
      0 0 / 12px 12px;
  }
  .glyph-stage img {
    display: block;
    width: 100%;
    user-select: none;
  }
  .glyph-stage.stale {
    opacity: 0.65;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--lapis);
    font-size: inherit;
    cursor: pointer;
  }
  .neg-preview {
    max-height: 4.5em;
    overflow: hidden;
  }
  details textarea {
    width: 100%;
    font-size: 0.72rem;
    margin-top: 0.3rem;
  }
  .refs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .ref-chip {
    width: 44px;
    height: 44px;
    padding: 2px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: transparent;
    overflow: hidden;
  }
  .ref-chip.on {
    border-color: var(--gold);
  }
  .ref-chip img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .refine-form {
    display: flex;
    gap: 0.4rem;
  }
  .refine-form input {
    flex: 1;
    min-width: 0;
    font-size: 0.72rem;
  }
  .row2 {
    display: flex;
    gap: 0.4rem;
  }
  .row2 button {
    flex: 1;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
  }
  .chip {
    font-size: 0.6rem;
    color: var(--sage);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 0.05rem 0.45rem;
  }
  .dock-foot {
    margin-top: auto;
    padding: 0.7rem 0.9rem;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  .tpl-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--ash);
    cursor: pointer;
  }
  .qc-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .qc {
    display: flex;
    gap: var(--space-3);
    align-items: flex-start;
  }
  .qc-mark {
    flex: none;
    width: 1rem;
    text-align: center;
    font-size: var(--text-sm);
  }
  .qc-pass .qc-mark {
    color: var(--sage);
  }
  .qc-warn .qc-mark {
    color: var(--gold);
  }
  .qc-fail .qc-mark {
    color: var(--oxblood);
  }
  .qc-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }
  .qc-label {
    font-size: var(--text-sm);
    color: var(--bone-dim);
  }
  .qc-warn .qc-label,
  .qc-fail .qc-label {
    color: var(--bone);
  }
  .qc-detail {
    line-height: 1.45;
  }
</style>
