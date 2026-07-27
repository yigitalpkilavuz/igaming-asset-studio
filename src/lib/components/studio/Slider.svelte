<script lang="ts">
  /**
   * Unified studio tuning control: micro-label + range + live mono value. The one knob
   * language shared by the Mesh (turn, density, sway) and Rig (physics) panels — replacing the
   * old raw number-input grids. `value` is display-only; `oninput` fires the new number, so it
   * works both for locally-bound state and for callbacks that fan out (e.g. sway → all bones).
   */
  let {
    label,
    value,
    min = 0,
    max = 1,
    step = 0.01,
    decimals = 2,
    suffix = "",
    amber = false,
    disabled = false,
    oninput,
  }: {
    label: string;
    value: number;
    min?: number;
    max?: number;
    step?: number;
    decimals?: number;
    suffix?: string;
    amber?: boolean;
    disabled?: boolean;
    oninput?: (v: number) => void;
  } = $props();

  const fmt = (v: number) => (decimals > 0 ? v.toFixed(decimals) : Math.round(v).toString()) + suffix;
</script>

<label class="sld" class:disabled>
  <span class="top">
    <span class="lbl">{label}</span>
    <span class="val mono">{fmt(value)}</span>
  </span>
  <input
    type="range"
    class:amber
    {min}
    {max}
    {step}
    {value}
    {disabled}
    oninput={(e) => oninput?.(+e.currentTarget.value)}
  />
</label>

<style>
  .sld {
    display: flex;
    flex-direction: column;
    gap: 0.28rem;
  }
  .sld.disabled {
    opacity: 0.5;
  }
  .top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .lbl {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ash);
    font-weight: 600;
  }
  .val {
    font-size: 0.72rem;
    color: var(--bone);
    font-variant-numeric: tabular-nums;
  }
  input[type="range"] {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 3px;
    border-radius: 999px;
    background: var(--line-2);
    cursor: pointer;
    margin: 0.2rem 0;
  }
  input[type="range"]:disabled {
    cursor: default;
  }
  input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 13px;
    height: 13px;
    border-radius: 999px;
    background: var(--bone);
    border: 2px solid var(--ink-2);
    box-shadow: 0 0 0 1px var(--line-2);
    cursor: pointer;
  }
  input[type="range"].amber::-webkit-slider-thumb {
    background: var(--gold);
    box-shadow: 0 0 0 1px var(--gold-deep);
  }
  input[type="range"]::-moz-range-thumb {
    width: 13px;
    height: 13px;
    border: 2px solid var(--ink-2);
    border-radius: 999px;
    background: var(--bone);
    cursor: pointer;
  }
  input[type="range"].amber::-moz-range-thumb {
    background: var(--gold);
  }
  input[type="range"]:focus-visible {
    outline: 2px solid var(--gold);
    outline-offset: 3px;
  }
</style>
