<script lang="ts">
  /**
   * Unit-square bezier ease editor for the selected key's OUT curve. Edits one handle
   * pair; the caller replicates it across value components (the emitter denormalizes to
   * Spine 4.x absolute control points).
   */
  let {
    handles,
    onchange,
  }: {
    /** [cx1, cy1, cx2, cy2] normalized 0..1 (cy may exceed 0..1 for overshoot). */
    handles: [number, number, number, number];
    onchange: (h: [number, number, number, number]) => void;
  } = $props();

  const SIZE = 148;
  const PAD = 10;
  let dragging: 0 | 1 | null = $state(null);
  let svg = $state<SVGSVGElement | null>(null);

  const px = (v: number) => PAD + v * (SIZE - 2 * PAD);
  const py = (v: number) => SIZE - PAD - v * (SIZE - 2 * PAD);

  function fromEvent(e: PointerEvent): { x: number; y: number } {
    const r = svg!.getBoundingClientRect();
    return {
      x: Math.min(Math.max((e.clientX - r.left - PAD) / (SIZE - 2 * PAD), 0), 1),
      y: Math.min(Math.max((SIZE - PAD - (e.clientY - r.top)) / (SIZE - 2 * PAD), -0.5), 1.5),
    };
  }

  function down(which: 0 | 1, e: PointerEvent) {
    dragging = which;
    (e.target as Element).setPointerCapture(e.pointerId);
  }
  function move(e: PointerEvent) {
    if (dragging === null) return;
    const p = fromEvent(e);
    const h: [number, number, number, number] = [...handles];
    h[dragging * 2] = p.x;
    h[dragging * 2 + 1] = p.y;
    onchange(h);
  }
</script>

<svg
  bind:this={svg}
  width={SIZE}
  height={SIZE}
  viewBox={`0 0 ${SIZE} ${SIZE}`}
  onpointermove={move}
  onpointerup={() => (dragging = null)}
  role="application"
  aria-label="Easing curve editor"
>
  <rect x={PAD} y={PAD} width={SIZE - 2 * PAD} height={SIZE - 2 * PAD} class="frame" />
  <path
    class="curve"
    d={`M ${px(0)} ${py(0)} C ${px(handles[0])} ${py(handles[1])}, ${px(handles[2])} ${py(handles[3])}, ${px(1)} ${py(1)}`}
  />
  <line class="arm" x1={px(0)} y1={py(0)} x2={px(handles[0])} y2={py(handles[1])} />
  <line class="arm" x1={px(1)} y1={py(1)} x2={px(handles[2])} y2={py(handles[3])} />
  <circle class="end" cx={px(0)} cy={py(0)} r="3" />
  <circle class="end" cx={px(1)} cy={py(1)} r="3" />
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <circle
    class="handle"
    cx={px(handles[0])}
    cy={py(handles[1])}
    r="6"
    onpointerdown={(e) => down(0, e)}
  />
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <circle
    class="handle"
    cx={px(handles[2])}
    cy={py(handles[3])}
    r="6"
    onpointerdown={(e) => down(1, e)}
  />
</svg>

<style>
  svg {
    display: block;
    touch-action: none;
    user-select: none;
  }
  .frame {
    fill: var(--wash-faint);
    stroke: var(--line);
  }
  .curve {
    fill: none;
    stroke: var(--gold);
    stroke-width: 1.6;
  }
  .arm {
    stroke: var(--ash-deep);
    stroke-width: 1;
  }
  .end {
    fill: var(--ash);
  }
  .handle {
    fill: var(--gold-bright);
    cursor: grab;
  }
  .handle:active {
    cursor: grabbing;
  }
</style>
