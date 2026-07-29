<script lang="ts">
  /**
   * Motion loop: turn an asset into a short looping spritesheet — the "rigging is overkill"
   * animation technique. Two sources feed the SAME `ai_sheet/` sheet: SpriteCook animating the
   * approved still, or a bring-your-own video clip baked to a transparent strip (the license-safe
   * lane for motion a rig can't author). Self-contained so it renders both in the bench dock and
   * in the Animate hub. `seedPrompt` pre-fills the motion prompt (e.g. the Blueprint's note).
   */
  import { commands, unwrap, type AiSheet, type VideoBg, type VideoLoop } from "$lib/ipc";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { runJob, runningTag } from "$lib/stores/jobs.svelte";

  let {
    gameId,
    assetKey,
    seedPrompt = "",
  }: { gameId: string; assetKey: string; seedPrompt?: string } = $props();

  let spritecookReady = $state(false);
  let sheetPrompt = $state("");
  let sheetFrames = $state(8);
  const sheetBusy = $derived(runningTag(assetKey, "sheet"));
  let sheetProgress = $state("");
  let aiSheet = $state<AiSheet | null>(null);
  let sheetCanvas = $state<HTMLCanvasElement | null>(null);
  let error = $state("");
  // The loop can come from SpriteCook (animate the approved still) or from a bring-your-own video
  // clip baked to the same sheet.
  let sheetSource = $state<"ai" | "video">("ai");
  let videoPath = $state("");
  let videoBg = $state<VideoBg>("magenta");
  let videoLoop = $state<VideoLoop>("pingPong");

  $effect(() => {
    commands.spritecookKeyPresent().then((ok) => (spritecookReady = ok));
    commands.getAiSheet(gameId, assetKey).then((r) => {
      if (r.status === "ok" && r.data) {
        aiSheet = r.data;
        sheetPrompt = r.data.prompt ?? "";
        sheetFrames = r.data.frames ?? 8;
      } else if (!sheetPrompt && seedPrompt.trim()) {
        sheetPrompt = seedPrompt.trim();
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

  async function runAiSheet() {
    error = "";
    const prompt = sheetPrompt;
    const frames = sheetFrames;
    const res = await runJob({
      gameId,
      assetKey,
      kind: "sheet",
      label: `AI sheet · ${assetKey}`,
      exec: async () => {
        const sheet = await unwrap(commands.generateAiSheet(gameId, assetKey, prompt, frames));
        // Not an AssetRecord — deliver directly; a remounted bench reloads it from disk.
        aiSheet = sheet;
        return null;
      },
    });
    if (res.error) error = res.error;
  }

  async function pickVideo() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Video", extensions: ["mp4", "mov", "webm", "gif", "m4v", "mkv"] }],
    });
    if (typeof picked === "string") videoPath = picked;
  }

  async function runVideoSheet() {
    error = "";
    const [path, bg, frames, loopMode] = [videoPath, videoBg, sheetFrames, videoLoop];
    const res = await runJob({
      gameId,
      assetKey,
      kind: "sheet",
      label: `Video loop · ${assetKey}`,
      exec: async () => {
        const sheet = await unwrap(
          commands.generateVideoSheet(gameId, assetKey, path, bg, frames, loopMode),
        );
        aiSheet = sheet; // a remounted bench reloads it from disk
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
      ctx.drawImage(img, frame * fw, 0, fw, sheet.height, 0, 0, canvas.width, canvas.height);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });
</script>

<div class="motion-loop">
  <div class="src-toggle">
    <button class="seg" class:on={sheetSource === "ai"} onclick={() => (sheetSource = "ai")}>AI · animate still</button>
    <button class="seg" class:on={sheetSource === "video"} onclick={() => (sheetSource = "video")}>Video file</button>
  </div>
  {#if sheetSource === "ai"}
    {#if !spritecookReady}
      <p class="muted tiny">
        Needs a SpriteCook key — add it in Settings. Animates the approved image into a
        looping spritesheet (good for FX and ambient motion; use the Spine rig for real
        articulation).
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
          {sheetBusy ? `Animating… ${sheetProgress}` : aiSheet ? "Redo loop" : "Generate loop"}
        </button>
      </div>
    {/if}
  {:else}
    <p class="muted tiny">
      Bake a short clip into a transparent looping sheet — for motion a rig can't author
      (flames, bursts, transformations). The clip stays local; only the baked sprite ships.
    </p>
    <button class="file-pick mono" onclick={pickVideo} title={videoPath}>
      {videoPath ? (videoPath.split("/").pop() ?? videoPath) : "Choose a video…"}
    </button>
    <div class="row2">
      <select bind:value={videoBg} title="how the clip's background is removed">
        <option value="magenta">magenta bg → key</option>
        <option value="glow">glow on black</option>
      </select>
      <select bind:value={videoLoop} title="how the clip is made seamless">
        <option value="pingPong">ping-pong loop</option>
        <option value="seam">seam-match loop</option>
      </select>
    </div>
    <div class="row2">
      <select bind:value={sheetFrames}>
        <option value={8}>8 frames</option>
        <option value={12}>12 frames</option>
        <option value={16}>16 frames</option>
        <option value={24}>24 frames</option>
      </select>
      <button onclick={runVideoSheet} disabled={sheetBusy || !videoPath}>
        {sheetBusy ? "Baking…" : aiSheet ? "Redo bake" : "Bake loop"}
      </button>
    </div>
    <p class="muted tiny">
      Generate the clip on a flat <strong>magenta</strong> background (or pure black for
      glows/sparks) so the cutout is clean.
    </p>
  {/if}
  {#if aiSheet}
    <canvas class="sheet-preview" bind:this={sheetCanvas}></canvas>
    <p class="muted tiny">
      {aiSheet.frames} frames · {aiSheet.width}×{aiSheet.height} — saved beside the asset as
      <span class="mono">ai_sheet/sheet.png</span>.
      {#if sheetSeamPct !== null}
        <br />
        <span class:seam-bad={sheetSeamPct > 8}>
          loop seam: {sheetSeamPct.toFixed(1)}% first↔last diff
          {sheetSeamPct > 8 ? "— visible pop likely, consider redoing" : "— loops cleanly"}
        </span>
      {/if}
    </p>
  {/if}
  {#if error}<p class="err">{error}</p>{/if}
</div>

<style>
  .motion-loop {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .tiny {
    font-size: 0.72rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .field textarea {
    width: 100%;
    font-size: 0.74rem;
  }
  .row2 {
    display: flex;
    gap: 0.4rem;
  }
  .row2 select {
    font-size: 0.74rem;
  }
  .row2 button {
    flex: 1;
  }
  /* Source picker: animate the still (SpriteCook) vs bake a video clip. */
  .src-toggle {
    display: flex;
    gap: 2px;
    padding: 2px;
    background: var(--wash-soft);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
  }
  .src-toggle .seg {
    flex: 1;
    padding: var(--space-1) var(--space-2);
    border: 1px solid transparent;
    border-radius: calc(var(--radius-sm) - 2px);
    background: transparent;
    color: var(--bone-dim);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .src-toggle .seg.on {
    background: var(--gold-glow);
    border-color: var(--gold-deep);
    color: var(--gold);
  }
  .file-pick {
    width: 100%;
    text-align: left;
    padding: var(--space-1) var(--space-2);
    border: 1px dashed var(--line);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--ink);
    font-size: 0.78rem;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file-pick:hover {
    border-color: var(--gold);
  }
  .sheet-preview {
    align-self: center;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background:
      repeating-conic-gradient(var(--wash-soft) 0% 25%, transparent 0% 50%) 0 0 / 12px 12px;
  }
  .seam-bad {
    color: var(--gold);
  }
  .err {
    color: var(--oxblood);
    font-size: 0.75rem;
  }
</style>
