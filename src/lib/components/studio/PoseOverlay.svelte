<script lang="ts">
  /**
   * Setup-mode pose testing: bone gizmos over a SpinePreview that pose the live skeleton
   * through its override hook WITHOUT touching the doc or any clip. Poses accumulate
   * across drags so a full stance can be tested; reset() (or unmount) clears them.
   */
  import { onMount } from "svelte";
  import type SpinePreview from "./SpinePreview.svelte";
  import type { StageBone } from "./SpinePreview.svelte";
  import {
    poseHit,
    beginDrag,
    applyDrag,
    drawGizmos as drawPoseGizmos,
    type PoseDrag,
  } from "./poseDrag";

  let {
    preview,
  }: {
    preview: ReturnType<typeof SpinePreview> | null;
  } = $props();

  let overlay = $state<HTMLCanvasElement | null>(null);
  let selected = $state<string | null>(null);
  let posed = $state(false);
  let drag: PoseDrag | null = null;

  onMount(() => {
    let raf = 0;
    const tick = () => {
      draw();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      preview?.clearPose();
    };
  });

  export function reset() {
    preview?.clearPose();
    posed = false;
  }

  function bones(): StageBone[] {
    return preview?.getStageBones().filter((b) => b.parent) ?? [];
  }

  function draw() {
    if (!overlay) return;
    drawPoseGizmos(overlay, overlay.parentElement!, bones(), selected, "#6fa7dd");
  }

  function onpointerdown(e: PointerEvent) {
    const rect = overlay!.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const hit = poseHit(bones(), selected, mx, my, e.metaKey || e.ctrlKey);
    if (hit.action === "clear") {
      selected = null;
      return;
    }
    if (hit.action === "select") {
      selected = hit.name;
      return;
    }
    if (hit.select) selected = hit.select;
    const setup = preview?.getSetup(hit.bone.name);
    if (!setup) return;
    overlay!.setPointerCapture(e.pointerId);
    drag = beginDrag(hit.bone, setup, hit.kind, mx, my);
  }

  function onpointermove(e: PointerEvent) {
    if (!drag || !preview) return;
    const rect = overlay!.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const b = bones().find((x) => x.name === drag!.bone);
    if (!b) return;
    posed = true;
    applyDrag(drag, b, mx, my, (bone, patch) => preview!.setPose(bone, patch));
  }

  function onpointerup() {
    drag = null; // pose stays — that's the point
  }
</script>

<canvas
  class="pose-overlay"
  bind:this={overlay}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
></canvas>
{#if posed}
  <button class="ghost reset-pose" onclick={reset}>Reset pose</button>
{/if}

<style>
  .pose-overlay {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    touch-action: none;
  }
  .reset-pose {
    position: absolute;
    top: 0.6rem;
    right: 0.8rem;
    font-size: 0.72rem;
  }
</style>
