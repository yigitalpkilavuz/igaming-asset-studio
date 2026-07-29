import type { AudioCueDef, AudioKind } from "$lib/ipc";

/** Build one cue def (defaults for the fields the UI doesn't always set). */
export function ac(
  key: string,
  kind: AudioKind,
  name: string,
  looped: boolean,
  gain: number,
  targetSecs: number,
  description: string,
): AudioCueDef {
  return { key, kind, name, description, provider: "", looped, gain, targetSecs };
}

/**
 * The "Core SFX + base music" seed list (mirrors `default_audio_cues()` in game_config.rs).
 * Names follow the Stake web-sdk convention: `bgm_*` looping music, `sfx_*` effects, `jng_*` jingles.
 */
export const DEFAULT_AUDIO_CUES: AudioCueDef[] = [
  ac("bgm_main", "music", "Base-game music", true, 0.7, 30, "the main ambient background music bed, seamless loop"),
  ac("sfx_btn_spin", "sfx", "Spin button", false, 1, 0.3, "a short tactile click/whoosh when the spin button is pressed"),
  ac("sfx_reel_spin", "sfx", "Reel spin", true, 0.6, 1, "a continuous whir while the reels are spinning, seamless loop"),
  ac("sfx_reel_stop_1", "sfx", "Reel stop", false, 1, 0.2, "a tight thunk/click as a reel lands"),
  ac("sfx_anticipation", "sfx", "Anticipation", true, 0.9, 2, "a rising tension build when scatter/bonus symbols are landing"),
  ac("sfx_win_tick", "sfx", "Win rollup", true, 0.8, 0.15, "a repeating coin/counter tick during the win count-up, loops"),
  ac("sfx_winlevel_small", "sfx", "Small win", false, 1, 1.5, "a short cheerful chime for a small win"),
  ac("sfx_winlevel_big", "sfx", "Big win", false, 1, 3, "an escalating celebratory fanfare for a big win"),
  ac("sfx_winlevel_mega", "sfx", "Mega win", false, 1, 4, "an over-the-top triumphant fanfare for a mega win"),
  ac("jng_bonus", "sfx", "Scatter / bonus", false, 1, 2, "a magical celebratory sting when a bonus/scatter triggers"),
  ac("sfx_btn_general", "sfx", "UI click", false, 0.8, 0.15, "a crisp, quiet UI button click"),
  ac("sfx_coin", "sfx", "Coin / collect", false, 1, 1, "a bright coin/cash payout collect sound"),
];

/** A blank cue of the given kind, with a unique key derived from the existing keys. */
export function newCue(kind: AudioKind, existing: AudioCueDef[]): AudioCueDef {
  const prefix = kind === "music" ? "bgm" : "sfx";
  let n = existing.filter((c) => c.kind === kind).length + 1;
  let key = `${prefix}_${n}`;
  const keys = new Set(existing.map((c) => c.key));
  while (keys.has(key)) key = `${prefix}_${++n}`;
  return ac(key, kind, kind === "music" ? "New music" : "New sound", kind === "music", kind === "music" ? 0.7 : 1, kind === "music" ? 30 : 1, "");
}
