import type { AssetRecord, Variation } from "$lib/ipc";

export type AssetStatus = {
  hasPrompt: boolean;
  generated: boolean;
  processed: boolean;
  ready: boolean;
  /** 0..3 — how many pipeline stages are complete (prompt, gen, proc). */
  step: number;
  activeVariation: Variation | null;
};

/** Derive the pipeline status for an asset from its record. */
export function assetStatus(rec?: AssetRecord): AssetStatus {
  const hasPrompt = !!(rec?.prompt.subject?.trim() || rec?.prompt.overridePrompt?.trim());
  const generated = (rec?.variations?.length ?? 0) > 0;
  const active =
    rec?.variations?.find((v) => v.id === rec?.activeVariation) ?? null;
  // "processed" = a shippable final exists: webp for images, the normalized wav for audio
  // (audio ships as one baked Howler audiosprite at export, not per-asset files).
  const processed = !!active?.stages?.some((s) => s.name === "webp" || s.name === "wav");
  const step = (hasPrompt ? 1 : 0) + (generated ? 1 : 0) + (processed ? 1 : 0);
  return {
    hasPrompt,
    generated,
    processed,
    ready: processed,
    step,
    activeVariation: active,
  };
}
