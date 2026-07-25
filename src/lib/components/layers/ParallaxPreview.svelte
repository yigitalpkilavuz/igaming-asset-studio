<script lang="ts">
  /**
   * Live parallax check: the cut/filled layers stacked back→front on a plain 2D canvas,
   * shifted per-layer by `speed × offset`. Offset follows the pointer (and an optional
   * sine auto-drift), capped at the doc's max shift. The stack renders at 1.1× — the
   * documented game-side scale that keeps canvas edges hidden while layers move.
   * "Show gaps" paints magenta underneath, so any unfilled reveal screams.
   */
  import type { Rect } from "$lib/ipc";

  export type PreviewLayer = {
    id: string;
    /** Texture data URL (fresh fill when available, else the raw cut). */
    url: string;
    /** Texture placement in source coords. */
    bbox: Rect;
    speed: number;
    stale: boolean;
  };

  let {
    width,
    height,
    layers,
    maxShift,
  }: {
    /** Source scene dims. */
    width: number;
    height: number;
    /** Back → front. */
    layers: PreviewLayer[];
    /** Max camera offset in source px. */
    maxShift: number;
  } = $props();

  const GAME_SCALE = 1.1;

  let host = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let drift = $state(true);
  let showGaps = $state(false);
  let pointer = $state<{ x: number; y: number } | null>(null); // -1..1

  const staleCount = $derived(layers.filter((l) => l.stale).length);

  // Texture cache keyed by URL (URLs change when a fill/cut regenerates).
  const bitmaps = new Map<string, HTMLImageElement>();
  function bitmap(url: string): HTMLImageElement | null {
    const hit = bitmaps.get(url);
    if (hit) return hit.complete ? hit : null;
    const img = new Image();
    img.src = url;
    bitmaps.set(url, img);
    return null;
  }

  $effect(() => {
    if (!canvas || !host) return;
    let raf = 0;
    const start = performance.now();
    const tick = () => {
      draw((performance.now() - start) / 1000);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });

  function draw(t: number) {
    if (!canvas || !host) return;
    const dpr = window.devicePixelRatio || 1;
    const { clientWidth: cw, clientHeight: ch } = host;
    if (canvas.width !== cw * dpr || canvas.height !== ch * dpr) {
      canvas.width = cw * dpr;
      canvas.height = ch * dpr;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cw, ch);

    // Camera offset in source px: pointer position, plus a slow figure-eight drift.
    let ox = (pointer?.x ?? 0) * maxShift;
    let oy = (pointer?.y ?? 0) * maxShift * 0.4;
    if (drift) {
      ox += Math.sin(t * 0.5) * maxShift * (pointer ? 0.25 : 1);
      oy += Math.sin(t * 0.31) * maxShift * 0.25 * (pointer ? 0.25 : 1);
    }

    // Fit the scene (at game scale) into the canvas, centered.
    const fit = Math.min(cw / width, ch / height) * GAME_SCALE;
    const originX = (cw - width * fit) / 2;
    const originY = (ch - height * fit) / 2;

    ctx.save();
    ctx.beginPath();
    ctx.rect(0, 0, cw, ch);
    ctx.clip();
    if (showGaps) {
      ctx.fillStyle = "#ff00ff";
      ctx.fillRect(originX, originY, width * fit, height * fit);
    }
    for (const layer of layers) {
      const img = bitmap(layer.url);
      if (!img) continue;
      const lx = originX + (layer.bbox.x - ox * layer.speed) * fit;
      const ly = originY + (layer.bbox.y - oy * layer.speed) * fit;
      ctx.drawImage(img, lx, ly, layer.bbox.w * fit, layer.bbox.h * fit);
    }
    ctx.restore();
  }

  function onpointermove(e: PointerEvent) {
    const rect = canvas!.getBoundingClientRect();
    pointer = {
      x: ((e.clientX - rect.left) / rect.width) * 2 - 1,
      y: ((e.clientY - rect.top) / rect.height) * 2 - 1,
    };
  }
</script>

<div class="preview" bind:this={host}>
  <canvas bind:this={canvas} {onpointermove} onpointerleave={() => (pointer = null)}></canvas>
  <div class="bar">
    <label class="opt"><input type="checkbox" bind:checked={drift} /><span class="tiny">auto drift</span></label>
    <label class="opt" title="paint magenta behind the stack — any gap that shows needs Fill hidden">
      <input type="checkbox" bind:checked={showGaps} /><span class="tiny">show gaps</span>
    </label>
    <span class="hint muted tiny">move the mouse over the stage to steer the camera</span>
    {#if staleCount}
      <span class="stale tiny">{staleCount} layer{staleCount === 1 ? "" : "s"} need re-fill</span>
    {/if}
  </div>
</div>

<style>
  .preview {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  canvas {
    flex: 1;
    min-height: 0;
    width: 100%;
    touch-action: none;
    background:
      repeating-conic-gradient(var(--wash-faint) 0% 25%, transparent 0% 50%)
      0 0 / 24px 24px;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.45rem 0.9rem;
    border-top: 1px solid var(--line);
    flex: none;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }
  .hint {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .stale {
    color: var(--gold);
  }
</style>
