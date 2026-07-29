import type { SymbolSizing, SymbolTone } from "$lib/ipc";

/**
 * Frontend mirror of the Rust `SymbolSizing` / `SymbolTone` `Default` impls
 * (`src-tauri/src/model/game_config.rs`). A NEW game omits these fields and the backend fills them
 * via serde defaults; this constant only backfills an OLDER config that predates them (ConfigForm).
 *
 * KEEP IN SYNC with the Rust defaults — a Rust test (`defaults_are_coherent`) pins the canonical
 * values (the `ink / height² ≈ 0.53` sizing invariant + the tone bands) so drift on that side is
 * caught; this literal must match it.
 */
export const DEFAULT_SYMBOL_SIZING: SymbolSizing = {
  low: { ink: 0.21, height: 0.63, tolerance: 0.02 },
  high: { ink: 0.3, height: 0.75, tolerance: 0.03 },
  wild: { ink: 0.34, height: 0.8, tolerance: 0.03 },
  scatter: { ink: 0.36, height: 0.82, tolerance: 0.03 },
  areaWeight: 0.65,
  centroidBias: 0.7,
  alphaFloor: 26,
  safeW: 0.92,
  safeH: 0.88,
  canvas: 0,
};

// Bands are median perceptual luminance; lows sit a tier darker than the pay symbols.
export const DEFAULT_SYMBOL_TONE: SymbolTone = {
  high: { min: 0.36, max: 0.44 },
  low: { min: 0.26, max: 0.34 },
  wild: { min: 0.36, max: 0.44 },
  scatter: { min: 0.36, max: 0.44 },
  alphaFloor: 200,
  ceiling: 0.92,
  gammaLo: 0.55,
  gammaHi: 1.6,
};
