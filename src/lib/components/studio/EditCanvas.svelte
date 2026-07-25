<script lang="ts">
  /**
   * The Cut-mode editing canvas: source image + tinted mask overlays on a zoom/pan 2D
   * canvas. Point tool: click = positive SAM prompt, shift/right-click = negative.
   * Brush/erase tools: paint the selected part's mask directly at source resolution.
   * Deliberately a plain <canvas>, NOT pixi — playback truth lives in SpinePreview only.
   */
  import type { SamPrompt } from "$lib/ipc";

  export type Overlay = {
    id: string;
    url: string;
    color: string; // css color for the tint
    active: boolean;
  };

  /** A bone drawn/edited by the gizmo layer (world source-px coords, Spine-CCW degrees). */
  export type GizmoBone = {
    name: string;
    parent: string | null;
    x: number;
    y: number;
    rotation: number;
    length: number;
    selected: boolean;
  };

  let {
    imageUrl,
    width,
    height,
    overlays = [],
    prompts = [],
    tool = "point",
    brushSize = 24,
    bones = [],
    outline = null,
    onpoint,
    onpointremove,
    onmaskedit,
    onboneselect,
    onbonechange,
    onbonecommit,
    onoutlinechange,
  }: {
    imageUrl: string | null;
    /** Source image dims (all mask editing happens at this resolution). */
    width: number;
    height: number;
    overlays?: Overlay[];
    /** Prompts of the selected part, drawn as markers. */
    prompts?: SamPrompt[];
    tool?: "point" | "brush" | "erase" | "lasso" | "lassoErase" | "bones" | "outline";
    brushSize?: number;
    /** Bone gizmos (rig mode): drag pivot = move, drag tip = aim + length (shift = aim only). */
    bones?: GizmoBone[];
    /** Outline tool: editable closed rings in image px (outer boundaries + holes). */
    outline?: [number, number][][] | null;
    onpoint?: (x: number, y: number, positive: boolean) => void;
    /** Point mode: delete a prompt dot — `index` of the clicked dot, or -1 for the
     *  most recent one (right-click on empty space = undo last). */
    onpointremove?: (index: number) => void;
    /** Fired on brush stroke end with the edited mask as a full-size PNG data URL. */
    onmaskedit?: (dataUrl: string) => void;
    onboneselect?: (name: string) => void;
    onbonechange?: (name: string, patch: Partial<Pick<GizmoBone, "x" | "y" | "rotation" | "length">>) => void;
    /** Fired after every outline edit (vertex drag end / insert / delete). */
    onoutlinechange?: (rings: [number, number][][]) => void;
    onbonecommit?: () => void;
  } = $props();

  let host = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);

  // View transform: image px → screen px is (p * zoom + pan).
  let zoom = $state(1);
  let panX = $state(0);
  let panY = $state(0);
  let fitted = false;

  let spaceHeld = $state(false);
  let panning = $state(false);
  let panStart = { x: 0, y: 0, panX: 0, panY: 0 };
  let painting = false;
  let cursor = $state<{ x: number; y: number } | null>(null); // image coords

  // Loaded bitmaps, keyed by URL (mask URLs change on every edit → natural invalidation).
  const bitmaps = new Map<string, HTMLImageElement>();
  // Tinted overlay canvases, keyed by `url|color`.
  const tinted = new Map<string, HTMLCanvasElement>();
  let redraw = $state(0); // bumped when an async image load finishes

  // The mask being brush-edited, at source resolution. Initialized from the active
  // overlay when entering brush/erase/lasso; committed via onmaskedit on stroke end.
  let editCanvas: HTMLCanvasElement | null = null;
  let editInitUrl: string | null = null;

  // Polygon lasso: image-space vertices of the in-progress selection.
  let lasso = $state<{ x: number; y: number }[]>([]);
  const isLasso = () => tool === "lasso" || tool === "lassoErase";

  // Outline tool: local editable copy of the traced rings (synced from the prop; the
  // parent gets a deep copy back on every committed edit).
  let rings = $state<[number, number][][]>([]);
  let outlineDrag: { ring: number; idx: number } | null = null;
  const copyRings = (r: [number, number][][]) =>
    r.map((ring) => ring.map((p) => [p[0], p[1]] as [number, number]));
  $effect(() => {
    const next = outline ? copyRings(outline) : [];
    rings = next;
    // A fresh trace from the parent starts a new outline-undo timeline. Our own commits
    // echo back identical content and harmlessly reset to the same state.
    if (ringHist.length === 0 || JSON.stringify(ringHist[ringIdx]) !== JSON.stringify(next)) {
      ringHist = [copyRings(next)];
      ringIdx = 0;
    }
  });
  function commitOutline() {
    ringHist = ringHist.slice(0, ringIdx + 1);
    ringHist.push(copyRings(rings));
    if (ringHist.length > HIST_MAX) ringHist.shift();
    ringIdx = ringHist.length - 1;
    onoutlinechange?.(copyRings(rings));
  }

  // ── Undo/redo ────────────────────────────────────────────────────────────────
  // Two timelines: committed mask states (brush/erase/lasso, as data URLs) and outline
  // ring states. Both reset when the edited layer changes.
  const HIST_MAX = 30;
  let maskHist: string[] = [];
  let maskIdx = -1;
  let histKey: string | null = null;
  let ringHist: [number, number][][][] = [];
  let ringIdx = -1;

  $effect(() => {
    const key = overlays.find((o) => o.active)?.id ?? null;
    if (key !== histKey) {
      histKey = key;
      maskHist = [];
      maskIdx = -1;
    }
  });

  /** Blank (all-empty) mask as a data URL — the baseline when editing started from nothing. */
  function blankMaskUrl(): string {
    const c = document.createElement("canvas");
    c.width = width;
    c.height = height;
    const ctx = c.getContext("2d")!;
    ctx.fillStyle = "#000000";
    ctx.fillRect(0, 0, width, height);
    return c.toDataURL("image/png");
  }

  /** Record a committed mask state and notify the parent. */
  function commitMask(url: string) {
    if (maskIdx < 0) {
      maskHist = [editInitUrl ?? blankMaskUrl()]; // pre-edit baseline → undo target
      maskIdx = 0;
    }
    maskHist = maskHist.slice(0, maskIdx + 1);
    maskHist.push(url);
    if (maskHist.length > HIST_MAX) maskHist.shift();
    maskIdx = maskHist.length - 1;
    onmaskedit?.(url);
  }

  function undoMask() {
    if (maskIdx <= 0) return;
    maskIdx--;
    editCanvas = null; // rebuild from the restored overlay on the next stroke
    onmaskedit?.(maskHist[maskIdx]);
  }

  function redoMask() {
    if (maskIdx < 0 || maskIdx >= maskHist.length - 1) return;
    maskIdx++;
    editCanvas = null;
    onmaskedit?.(maskHist[maskIdx]);
  }

  function undoRings() {
    if (ringIdx <= 0) return;
    ringIdx--;
    rings = copyRings(ringHist[ringIdx]);
    redraw++;
    onoutlinechange?.(copyRings(rings));
  }

  function redoRings() {
    if (ringIdx >= ringHist.length - 1) return;
    ringIdx++;
    rings = copyRings(ringHist[ringIdx]);
    redraw++;
    onoutlinechange?.(copyRings(rings));
  }

  // Leaving the lasso tools abandons the in-progress polygon.
  $effect(() => {
    void tool;
    lasso = [];
  });

  /** Rasterize the closed polygon into the mask (add or erase) and commit. */
  function closeLasso() {
    if (lasso.length < 3) return;
    const c = ensureEditCanvas();
    if (!c) return;
    const ctx = c.getContext("2d")!;
    ctx.globalCompositeOperation = tool === "lassoErase" ? "destination-out" : "source-over";
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.moveTo(lasso[0].x, lasso[0].y);
    for (const pt of lasso.slice(1)) ctx.lineTo(pt.x, pt.y);
    ctx.closePath();
    ctx.fill();
    ctx.globalCompositeOperation = "source-over";
    lasso = [];
    redraw++;
    commitMask(exportMask(c));
  }

  function img(url: string): HTMLImageElement | null {
    const hit = bitmaps.get(url);
    if (hit) return hit.complete ? hit : null;
    const el = new Image();
    el.onload = () => redraw++;
    el.src = url;
    bitmaps.set(url, el);
    return null;
  }

  function tintedMask(url: string, color: string): HTMLCanvasElement | null {
    const key = `${url}|${color}`;
    const hit = tinted.get(key);
    if (hit) return hit;
    const source = img(url);
    if (!source) return null;
    const c = document.createElement("canvas");
    c.width = width;
    c.height = height;
    const ctx = c.getContext("2d")!;
    // Mask PNGs are white-on-black: use 'screen'-style luminance as alpha via drawing the
    // mask, then colorizing with source-in.
    ctx.drawImage(source, 0, 0, width, height);
    // Black pixels are opaque in the gray PNG — knock them out by treating luminance as
    // alpha: draw onto a temp with 'multiply' won't do it; instead read pixels once.
    const data = ctx.getImageData(0, 0, width, height);
    const px = data.data;
    for (let i = 0; i < px.length; i += 4) {
      px[i + 3] = px[i]; // alpha = luminance
    }
    ctx.putImageData(data, 0, 0);
    ctx.globalCompositeOperation = "source-in";
    ctx.fillStyle = color;
    ctx.fillRect(0, 0, width, height);
    tinted.set(key, c);
    return c;
  }

  /** Ensure the brush-editing canvas mirrors the active overlay's current mask. */
  function ensureEditCanvas(): HTMLCanvasElement | null {
    const active = overlays.find((o) => o.active);
    const url = active?.url ?? null;
    if (editCanvas && editInitUrl === url) return editCanvas;
    const c = document.createElement("canvas");
    c.width = width;
    c.height = height;
    const ctx = c.getContext("2d")!;
    if (url) {
      const source = img(url);
      if (!source) return null; // not loaded yet; try again next event
      ctx.drawImage(source, 0, 0, width, height);
      const data = ctx.getImageData(0, 0, width, height);
      const px = data.data;
      for (let i = 0; i < px.length; i += 4) {
        const on = px[i] > 127;
        px[i] = px[i + 1] = px[i + 2] = 255;
        px[i + 3] = on ? 255 : 0;
      }
      ctx.putImageData(data, 0, 0);
    }
    editCanvas = c;
    editInitUrl = url;
    return c;
  }

  /** Reset brush state when the active part or its mask changes. */
  $effect(() => {
    const active = overlays.find((o) => o.active);
    if (editInitUrl !== (active?.url ?? null)) {
      editCanvas = null;
    }
  });

  // ── Rendering ────────────────────────────────────────────────────────────────
  $effect(() => {
    void redraw;
    void zoom;
    void panX;
    void panY;
    void overlays;
    void prompts;
    void imageUrl;
    void tool;
    void cursor;
    void brushSize;
    void bones;
    draw();
  });

  function draw() {
    if (!canvas || !host) return;
    const dpr = window.devicePixelRatio || 1;
    const { clientWidth: cw, clientHeight: ch } = host;
    if (cw === 0 || ch === 0) return;
    if (canvas.width !== cw * dpr || canvas.height !== ch * dpr) {
      canvas.width = cw * dpr;
      canvas.height = ch * dpr;
    }
    if (!fitted && width > 0) {
      zoom = Math.min((cw * 0.9) / width, (ch * 0.9) / height);
      panX = (cw - width * zoom) / 2;
      panY = (ch - height * zoom) / 2;
      fitted = true;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cw, ch);

    ctx.save();
    ctx.translate(panX, panY);
    ctx.scale(zoom, zoom);
    ctx.imageSmoothingEnabled = zoom < 3;

    // Art-board shadow + dark board.
    ctx.fillStyle = "rgba(255,255,255,0.03)";
    ctx.fillRect(0, 0, width, height);

    const source = imageUrl ? img(imageUrl) : null;
    if (source) ctx.drawImage(source, 0, 0, width, height);

    // Mask overlays: inactive dim, active bright; brush edits preview live.
    for (const o of overlays) {
      const isEditing = o.active && (tool === "brush" || tool === "erase" || isLasso()) && editCanvas;
      const layer = isEditing ? colorize(editCanvas!, o.color) : tintedMask(o.url, o.color);
      if (!layer) continue;
      ctx.globalAlpha = o.active ? 0.5 : 0.22;
      ctx.drawImage(layer, 0, 0, width, height);
    }
    ctx.globalAlpha = 1;

    // Prompt markers (constant screen size).
    const r = 7 / zoom;
    for (const p of prompts) {
      const x = (p.x ?? 0) * width;
      const y = (p.y ?? 0) * height;
      ctx.lineWidth = 2.5 / zoom;
      ctx.strokeStyle = "#0b0b0c";
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.stroke();
      ctx.lineWidth = 1.5 / zoom;
      ctx.strokeStyle = p.label === "positive" ? "#7fc584" : "#e0716a";
      ctx.fillStyle = p.label === "positive" ? "rgba(127,197,132,0.9)" : "rgba(224,113,106,0.9)";
      ctx.beginPath();
      ctx.arc(x, y, r * 0.7, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }

    // Bone gizmos.
    if (tool === "bones" && bones.length) {
      const byName = new Map(bones.map((b) => [b.name, b]));
      // Parent connectors first (under the bones).
      ctx.lineWidth = 1 / zoom;
      ctx.strokeStyle = "rgba(116,116,126,0.55)";
      ctx.setLineDash([4 / zoom, 3 / zoom]);
      for (const b of bones) {
        const par = b.parent ? byName.get(b.parent) : null;
        if (!par) continue;
        ctx.beginPath();
        ctx.moveTo(par.x, par.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      for (const b of bones) {
        drawBone(ctx, b);
      }
    }

    // Lasso polygon: dashed outline, vertex handles, rubber band to the cursor, and a
    // highlighted first vertex once the shape can close.
    if (isLasso() && lasso.length) {
      const col = tool === "lassoErase" ? "#e0716a" : "#edc76a";
      ctx.lineWidth = 1.4 / zoom;
      ctx.strokeStyle = col;
      ctx.setLineDash([5 / zoom, 4 / zoom]);
      ctx.beginPath();
      ctx.moveTo(lasso[0].x, lasso[0].y);
      for (const pt of lasso.slice(1)) ctx.lineTo(pt.x, pt.y);
      if (cursor) ctx.lineTo(cursor.x, cursor.y);
      ctx.stroke();
      ctx.setLineDash([]);
      const vr = 3 / zoom;
      ctx.fillStyle = col;
      for (const pt of lasso) {
        ctx.fillRect(pt.x - vr, pt.y - vr, vr * 2, vr * 2);
      }
      if (lasso.length >= 3) {
        ctx.beginPath();
        ctx.arc(lasso[0].x, lasso[0].y, 8 / zoom, 0, Math.PI * 2);
        ctx.strokeStyle = col;
        ctx.stroke();
      }
    }

    // Editable outline rings: closed polylines + draggable vertex handles.
    if (tool === "outline" && rings.length) {
      ctx.lineWidth = 1.4 / zoom;
      ctx.strokeStyle = "#edc76a";
      for (const ring of rings) {
        if (ring.length < 2) continue;
        ctx.beginPath();
        ctx.moveTo(ring[0][0], ring[0][1]);
        for (const p of ring.slice(1)) ctx.lineTo(p[0], p[1]);
        ctx.closePath();
        ctx.stroke();
      }
      const vr = 3.5 / zoom;
      for (const ring of rings) {
        for (const p of ring) {
          ctx.fillStyle = "rgba(12,13,16,0.9)";
          ctx.fillRect(p[0] - vr, p[1] - vr, vr * 2, vr * 2);
          ctx.strokeStyle = "#edc76a";
          ctx.lineWidth = 1.2 / zoom;
          ctx.strokeRect(p[0] - vr, p[1] - vr, vr * 2, vr * 2);
        }
      }
    }

    // Brush cursor.
    if (cursor && (tool === "brush" || tool === "erase")) {
      ctx.lineWidth = 1.5 / zoom;
      ctx.strokeStyle = tool === "brush" ? "rgba(217,169,68,0.9)" : "rgba(224,113,106,0.9)";
      ctx.beginPath();
      ctx.arc(cursor.x, cursor.y, brushSize / 2, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.restore();
  }

  /** Tip position of a bone (screen y-down: Spine-CCW rotation flips y). */
  function boneTip(b: GizmoBone): { x: number; y: number } {
    const len = b.length > 0 ? b.length : 0;
    const r = (b.rotation * Math.PI) / 180;
    return { x: b.x + Math.cos(r) * len, y: b.y - Math.sin(r) * len };
  }

  function drawBone(ctx: CanvasRenderingContext2D, b: GizmoBone) {
    const main = b.selected ? "#edc76a" : b.parent ? "#a6aab5" : "#757b88";
    const rPivot = (b.selected ? 7 : 5.5) / zoom;
    if (b.length > 0) {
      // Spine-style kite from pivot to tip.
      const tip = boneTip(b);
      const r = (b.rotation * Math.PI) / 180;
      const wHalf = Math.min(b.length * 0.12, 10 / zoom + b.length * 0.04);
      const nx = Math.sin(r) * wHalf; // normal (y-down)
      const ny = Math.cos(r) * wHalf;
      const bx = b.x + Math.cos(r) * b.length * 0.14;
      const by = b.y - Math.sin(r) * b.length * 0.14;
      ctx.beginPath();
      ctx.moveTo(b.x, b.y);
      ctx.lineTo(bx + nx, by + ny);
      ctx.lineTo(tip.x, tip.y);
      ctx.lineTo(bx - nx, by - ny);
      ctx.closePath();
      ctx.fillStyle = b.selected ? "rgba(237,199,106,0.28)" : "rgba(166,170,181,0.16)";
      ctx.fill();
      ctx.lineWidth = 1.5 / zoom;
      ctx.strokeStyle = main;
      ctx.stroke();
      // Tip handle.
      ctx.beginPath();
      ctx.arc(tip.x, tip.y, 4.5 / zoom, 0, Math.PI * 2);
      ctx.fillStyle = b.selected ? "#edc76a" : "#757b88";
      ctx.fill();
    }
    // Pivot handle.
    ctx.beginPath();
    ctx.arc(b.x, b.y, rPivot, 0, Math.PI * 2);
    ctx.fillStyle = "rgba(12,13,16,0.85)";
    ctx.fill();
    ctx.lineWidth = 2 / zoom;
    ctx.strokeStyle = main;
    ctx.stroke();
    if (!b.parent) {
      // Root: crosshair.
      ctx.lineWidth = 1.2 / zoom;
      ctx.beginPath();
      ctx.moveTo(b.x - rPivot * 1.8, b.y);
      ctx.lineTo(b.x + rPivot * 1.8, b.y);
      ctx.moveTo(b.x, b.y - rPivot * 1.8);
      ctx.lineTo(b.x, b.y + rPivot * 1.8);
      ctx.stroke();
    }
    // Label.
    ctx.font = `${11 / zoom}px ui-monospace, monospace`;
    ctx.fillStyle = b.selected ? "#edc76a" : "rgba(166,170,181,0.75)";
    ctx.fillText(b.name, b.x + rPivot * 1.6, b.y - rPivot * 1.6);
  }

  // Bone drag state.
  let boneDrag: { name: string; kind: "pivot" | "tip" } | null = null;

  function hitBone(p: { x: number; y: number }): { name: string; kind: "pivot" | "tip" } | null {
    const rHit = 12 / zoom;
    // Tips first (they sit "on top" conceptually), topmost-drawn last → iterate reversed.
    for (const b of [...bones].reverse()) {
      if (b.length > 0) {
        const t = boneTip(b);
        if (Math.hypot(p.x - t.x, p.y - t.y) < rHit) return { name: b.name, kind: "tip" };
      }
    }
    for (const b of [...bones].reverse()) {
      if (Math.hypot(p.x - b.x, p.y - b.y) < rHit) return { name: b.name, kind: "pivot" };
    }
    return null;
  }

  const colorized = new Map<HTMLCanvasElement, { color: string; canvas: HTMLCanvasElement }>();
  /** Colorize a white-on-transparent mask canvas (cheap enough per frame at edit time). */
  function colorize(mask: HTMLCanvasElement, color: string): HTMLCanvasElement {
    const hit = colorized.get(mask);
    const c = hit?.canvas ?? document.createElement("canvas");
    c.width = mask.width;
    c.height = mask.height;
    const ctx = c.getContext("2d")!;
    ctx.clearRect(0, 0, c.width, c.height);
    ctx.drawImage(mask, 0, 0);
    ctx.globalCompositeOperation = "source-in";
    ctx.fillStyle = color;
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.globalCompositeOperation = "source-over";
    colorized.set(mask, { color, canvas: c });
    return c;
  }

  // ── Pointer interaction ──────────────────────────────────────────────────────
  function toImage(e: PointerEvent | WheelEvent): { x: number; y: number } {
    const rect = canvas!.getBoundingClientRect();
    return {
      x: (e.clientX - rect.left - panX) / zoom,
      y: (e.clientY - rect.top - panY) / zoom,
    };
  }

  function onwheel(e: WheelEvent) {
    e.preventDefault();
    const p = toImage(e);
    const factor = Math.exp(-e.deltaY * 0.0015);
    const next = Math.min(Math.max(zoom * factor, 0.05), 40);
    // Keep the point under the cursor fixed.
    panX -= p.x * (next - zoom);
    panY -= p.y * (next - zoom);
    zoom = next;
  }

  function onpointerdown(e: PointerEvent) {
    canvas!.setPointerCapture(e.pointerId);
    if (e.button === 1 || spaceHeld) {
      panning = true;
      panStart = { x: e.clientX, y: e.clientY, panX, panY };
      return;
    }
    const p = toImage(e);
    if (isLasso()) {
      if (e.button === 2) {
        lasso = lasso.slice(0, -1); // right-click = undo the last point (esc cancels all)
        return;
      }
      if (e.button !== 0) return;
      if (
        lasso.length >= 3 &&
        Math.hypot(p.x - lasso[0].x, p.y - lasso[0].y) < 10 / zoom
      ) {
        closeLasso();
        return;
      }
      lasso = [...lasso, { x: p.x, y: p.y }];
      return;
    }
    if (tool === "bones") {
      if (e.button !== 0) return;
      const hit = hitBone(p);
      if (hit) {
        onboneselect?.(hit.name);
        boneDrag = hit;
      }
      return;
    }
    if (tool === "outline") {
      const vHit = hitOutlineVertex(p);
      if (vHit) {
        if (e.button === 2 || e.altKey) {
          // Delete the vertex; a ring that would drop under 3 points is removed whole.
          const ring = rings[vHit.ring];
          if (ring.length > 3) {
            ring.splice(vHit.idx, 1);
          } else {
            rings.splice(vHit.ring, 1);
          }
          redraw++;
          commitOutline();
          return;
        }
        if (e.button === 0) outlineDrag = vHit;
        return;
      }
      if (e.button === 0) {
        // Click on an edge segment inserts a vertex there and starts dragging it.
        const sHit = hitOutlineSegment(p);
        if (sHit) {
          rings[sHit.ring].splice(sHit.after + 1, 0, [p.x, p.y]);
          outlineDrag = { ring: sHit.ring, idx: sHit.after + 1 };
          redraw++;
        }
      }
      return;
    }
    if (tool === "point") {
      if (p.x < 0 || p.y < 0 || p.x >= width || p.y >= height) return;
      // Clicking an existing dot deletes it (either button) — the way to fix a
      // misplaced node without starting over.
      const hitR = 10 / zoom;
      const hit = prompts.findIndex(
        (pr) => Math.hypot((pr.x ?? 0) * width - p.x, (pr.y ?? 0) * height - p.y) < hitR,
      );
      if (hit >= 0 && (e.button === 0 || e.button === 2)) {
        onpointremove?.(hit);
        return;
      }
      if (e.button === 2) {
        // Right-click on empty space = undo the last dot (same convention as lasso).
        onpointremove?.(-1);
        return;
      }
      if (e.shiftKey) {
        onpoint?.(p.x / width, p.y / height, false);
      } else if (e.button === 0) {
        onpoint?.(p.x / width, p.y / height, true);
      }
    } else if (e.button === 0) {
      const c = ensureEditCanvas();
      if (!c) return;
      painting = true;
      paint(c, p.x, p.y);
    }
  }

  function onpointermove(e: PointerEvent) {
    const p = toImage(e);
    cursor = p;
    if (panning) {
      panX = panStart.panX + (e.clientX - panStart.x);
      panY = panStart.panY + (e.clientY - panStart.y);
      return;
    }
    if (outlineDrag) {
      const ring = rings[outlineDrag.ring];
      if (ring?.[outlineDrag.idx]) {
        ring[outlineDrag.idx] = [
          Math.max(0, Math.min(width, p.x)),
          Math.max(0, Math.min(height, p.y)),
        ];
        redraw++;
      }
      return;
    }
    if (boneDrag) {
      const b = bones.find((x) => x.name === boneDrag!.name);
      if (!b) return;
      if (boneDrag.kind === "pivot") {
        onbonechange?.(b.name, { x: p.x, y: p.y });
      } else {
        // Tip drag: aim (Spine-CCW: flip y) + length; shift preserves length.
        const dx = p.x - b.x;
        const dy = p.y - b.y;
        const rotation = (Math.atan2(-dy, dx) * 180) / Math.PI;
        if (e.shiftKey) {
          onbonechange?.(b.name, { rotation });
        } else {
          onbonechange?.(b.name, { rotation, length: Math.max(Math.hypot(dx, dy), 1) });
        }
      }
      return;
    }
    if (painting && editCanvas) {
      paint(editCanvas, p.x, p.y);
    }
  }

  function onpointerup() {
    panning = false;
    if (outlineDrag) {
      outlineDrag = null;
      commitOutline();
    }
    if (boneDrag) {
      boneDrag = null;
      onbonecommit?.();
    }
    if (painting && editCanvas) {
      painting = false;
      commitMask(exportMask(editCanvas));
    }
  }

  function hitOutlineVertex(p: { x: number; y: number }): { ring: number; idx: number } | null {
    const r = 9 / zoom;
    for (let ri = 0; ri < rings.length; ri++) {
      for (let i = 0; i < rings[ri].length; i++) {
        const v = rings[ri][i];
        if (Math.hypot(p.x - v[0], p.y - v[1]) < r) return { ring: ri, idx: i };
      }
    }
    return null;
  }

  function hitOutlineSegment(p: { x: number; y: number }): { ring: number; after: number } | null {
    const maxD = 6 / zoom;
    for (let ri = 0; ri < rings.length; ri++) {
      const ring = rings[ri];
      for (let i = 0; i < ring.length; i++) {
        const a = ring[i];
        const b = ring[(i + 1) % ring.length];
        const vx = b[0] - a[0];
        const vy = b[1] - a[1];
        const len2 = vx * vx + vy * vy;
        if (len2 <= 0) continue;
        const t = ((p.x - a[0]) * vx + (p.y - a[1]) * vy) / len2;
        if (t < 0 || t > 1) continue;
        const d = Math.hypot(p.x - (a[0] + t * vx), p.y - (a[1] + t * vy));
        if (d < maxD) return { ring: ri, after: i };
      }
    }
    return null;
  }

  function paint(c: HTMLCanvasElement, x: number, y: number) {
    const ctx = c.getContext("2d")!;
    ctx.globalCompositeOperation = tool === "erase" ? "destination-out" : "source-over";
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(x, y, brushSize / 2, 0, Math.PI * 2);
    ctx.fill();
    redraw++;
  }

  /** White-on-transparent editing canvas → white-on-black gray PNG data URL. */
  function exportMask(c: HTMLCanvasElement): string {
    const out = document.createElement("canvas");
    out.width = width;
    out.height = height;
    const ctx = out.getContext("2d")!;
    ctx.fillStyle = "#000000";
    ctx.fillRect(0, 0, width, height);
    ctx.drawImage(c, 0, 0);
    return out.toDataURL("image/png");
  }

  function onkeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT") {
      return;
    }
    if (e.key === " " && !e.repeat) {
      spaceHeld = true;
      e.preventDefault();
    } else if (isLasso() && e.key === "Enter" && lasso.length >= 3) {
      e.preventDefault();
      closeLasso();
    } else if (isLasso() && e.key === "Escape" && lasso.length) {
      e.preventDefault();
      lasso = [];
    } else if (isLasso() && e.key === "Backspace" && lasso.length) {
      e.preventDefault();
      lasso = lasso.slice(0, -1); // drop the last placed point
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (isLasso() && lasso.length && !e.shiftKey) {
        lasso = lasso.slice(0, -1); // mid-lasso: undo the last point first
      } else if (tool === "outline") {
        if (e.shiftKey) redoRings();
        else undoRings();
      } else if (e.shiftKey) {
        redoMask();
      } else {
        undoMask();
      }
    }
  }
  function onkeyup(e: KeyboardEvent) {
    if (e.key === " ") spaceHeld = false;
  }

  // Redraw on container resize.
  $effect(() => {
    if (!host) return;
    const ro = new ResizeObserver(() => redraw++);
    ro.observe(host);
    return () => ro.disconnect();
  });
</script>

<svelte:window {onkeydown} {onkeyup} />

<div
  class="wrap"
  class:panning={spaceHeld || panning}
  bind:this={host}
  role="application"
  aria-label="Mask editor"
>
  <canvas
    bind:this={canvas}
    class:crosshair={isLasso()}
    {onwheel}
    {onpointerdown}
    {onpointermove}
    {onpointerup}
    onpointerleave={() => (cursor = null)}
    ondblclick={() => isLasso() && closeLasso()}
    oncontextmenu={(e) => e.preventDefault()}
  ></canvas>
  <span class="hint muted tiny">
    scroll = zoom · space-drag = pan ·
    {tool === "point"
      ? "click = include, shift-click = exclude"
      : tool === "bones"
        ? "drag ring = move joint · drag tip = aim/length (shift = aim only)"
        : tool === "outline"
          ? "drag vertex = move · click edge = insert · ⌥-click vertex = delete · ⌘Z = undo"
          : isLasso()
            ? "click = drop point · right-click/⌫ = undo point · enter = close · esc = cancel"
            : "drag = paint · ⌘Z = undo stroke"}
  </span>
</div>

<style>
  .wrap {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }
  .wrap.panning {
    cursor: grab;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    touch-action: none;
  }
  canvas.crosshair {
    cursor: crosshair;
  }
  .hint {
    position: absolute;
    left: 0.9rem;
    bottom: 0.6rem;
    pointer-events: none;
    opacity: 0.7;
  }
</style>
