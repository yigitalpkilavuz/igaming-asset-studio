/**
 * Shared stage-space bone-pose interaction: hit-testing, rotate/translate drag math,
 * and gizmo drawing. Used by AnimateMode (auto-key on release) and PoseOverlay
 * (setup-mode pose testing) — the only difference between them is what happens on
 * pointer-up and the accent color.
 *
 * EditCanvas's rig-mode bone editing is intentionally NOT this: it edits pivot/aim/length
 * in doc source-px (y-down) coordinates, a different model from posing a live skeleton.
 */
import type { StageBone } from "./SpinePreview.svelte";

export type PoseDragKind = "rotate" | "translate";

export type PoseDrag = {
  bone: string;
  kind: PoseDragKind;
  startAngle: number;
  startAnim: { rotation: number; x: number; y: number };
  setup: { rotation: number; x: number; y: number };
  parentWorldRotation: number;
  scale: number;
  startX: number;
  startY: number;
  moved: boolean;
};

/** Screen position of a bone's tip handle (screen y-down; min length keeps it grabbable). */
export function tipOf(b: StageBone): { x: number; y: number } {
  const len = Math.max(b.length * b.scale, 46);
  const r = (b.worldRotation * Math.PI) / 180;
  return { x: b.sx + Math.cos(r) * len, y: b.sy - Math.sin(r) * len };
}

export type PoseHit =
  /** Start a drag; `select` is set when the grab also changes the selection (pivot grabs). */
  | { action: "drag"; bone: StageBone; kind: PoseDragKind; select?: string }
  | { action: "select"; name: string }
  | { action: "clear" };

/**
 * Pointer-down resolution: selected bone's tip → rotate; any pivot → select
 * (⌘/ctrl = translate drag, second grab on the already-selected bone = rotate);
 * empty space → clear selection.
 */
export function poseHit(
  bones: StageBone[],
  selectedName: string | null,
  mx: number,
  my: number,
  translateModifier: boolean,
): PoseHit {
  const sel = bones.find((b) => b.name === selectedName);
  if (sel) {
    const tip = tipOf(sel);
    if (Math.hypot(mx - tip.x, my - tip.y) < 12) {
      return { action: "drag", bone: sel, kind: "rotate" };
    }
  }
  for (const b of bones) {
    if (Math.hypot(mx - b.sx, my - b.sy) < 12) {
      if (translateModifier) return { action: "drag", bone: b, kind: "translate", select: b.name };
      if (b.name === sel?.name) return { action: "drag", bone: b, kind: "rotate", select: b.name };
      return { action: "select", name: b.name };
    }
  }
  return { action: "clear" };
}

export function beginDrag(
  b: StageBone,
  setup: { rotation: number; x: number; y: number },
  kind: PoseDragKind,
  mx: number,
  my: number,
): PoseDrag {
  return {
    bone: b.name,
    kind,
    startAngle: Math.atan2(-(my - b.sy), mx - b.sx),
    startAnim: { rotation: b.animRotation, x: b.animX, y: b.animY },
    setup,
    parentWorldRotation: b.parentWorldRotation,
    scale: b.scale,
    startX: mx,
    startY: my,
    moved: false,
  };
}

/** Advance a drag to the current pointer position and pose the skeleton via `setPose`. */
export function applyDrag(
  drag: PoseDrag,
  b: StageBone,
  mx: number,
  my: number,
  setPose: (bone: string, patch: { rotation?: number; x?: number; y?: number }) => void,
): void {
  drag.moved = true;
  if (drag.kind === "rotate") {
    const ang = Math.atan2(-(my - b.sy), mx - b.sx);
    let delta = ((ang - drag.startAngle) * 180) / Math.PI;
    while (delta > 180) delta -= 360;
    while (delta <= -180) delta += 360;
    setPose(drag.bone, { rotation: drag.setup.rotation + drag.startAnim.rotation + delta });
  } else {
    // Screen delta → spine world delta → parent-local delta.
    const dxs = (mx - drag.startX) / drag.scale;
    const dys = -(my - drag.startY) / drag.scale;
    const r = (-drag.parentWorldRotation * Math.PI) / 180;
    const lx = dxs * Math.cos(r) - dys * Math.sin(r);
    const ly = dxs * Math.sin(r) + dys * Math.cos(r);
    setPose(drag.bone, {
      x: drag.setup.x + drag.startAnim.x + lx,
      y: drag.setup.y + drag.startAnim.y + ly,
    });
  }
}

/**
 * Draw the pivot rings + selected bone's tip handle onto an overlay canvas,
 * handling DPR resize and clearing. Pass `bones: []` to just clear (e.g. while playing).
 */
export function drawGizmos(
  overlay: HTMLCanvasElement,
  host: HTMLElement,
  bones: StageBone[],
  selectedName: string | null,
  accent: string,
): void {
  const dpr = window.devicePixelRatio || 1;
  const { clientWidth: w, clientHeight: h } = host;
  if (overlay.width !== w * dpr || overlay.height !== h * dpr) {
    overlay.width = w * dpr;
    overlay.height = h * dpr;
  }
  const ctx = overlay.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  for (const b of bones) {
    const sel = b.name === selectedName;
    ctx.beginPath();
    ctx.arc(b.sx, b.sy, sel ? 7 : 5, 0, Math.PI * 2);
    ctx.fillStyle = "rgba(12,13,16,0.8)";
    ctx.fill();
    ctx.lineWidth = sel ? 2 : 1.4;
    ctx.strokeStyle = sel ? accent : "rgba(166,170,181,0.8)";
    ctx.stroke();
    if (sel) {
      const tip = tipOf(b);
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(b.sx, b.sy);
      ctx.lineTo(tip.x, tip.y);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.beginPath();
      ctx.arc(tip.x, tip.y, 6, 0, Math.PI * 2);
      ctx.fillStyle = accent;
      ctx.fill();
      ctx.font = "10px ui-monospace, monospace";
      ctx.fillStyle = accent;
      ctx.fillText(`${b.name}  ${b.animRotation.toFixed(1)}°`, b.sx + 10, b.sy - 10);
    }
  }
}
