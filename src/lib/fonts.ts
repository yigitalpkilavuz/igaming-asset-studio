import type { FontDef } from "$lib/ipc";

/** Build one font def. */
export function fd(
  key: string,
  name: string,
  typeface: string,
  sizePx: number,
  fillTop: string,
  fillBottom: string,
  outlineColor: string,
  outlinePx: number,
): FontDef {
  return { key, name, typeface, sizePx, fillTop, fillBottom, outlineColor, outlinePx };
}

/**
 * The default font set seeded when a project first enables fonts (mirrors `default_fonts()` in
 * game_config.rs): a clean white numeral font + a gold gradient win font.
 */
export const DEFAULT_FONTS: FontDef[] = [
  fd("font_white", "White numerals", "titan_one", 96, "#ffffff", "#e6ecf5", "#1a2230", 6),
  fd("font_gold", "Gold win", "luckiest_guy", 108, "#ffe89a", "#e0a13a", "#4a2d0a", 7),
];

/** A blank font with a unique key derived from the existing keys. */
export function newFont(existing: FontDef[]): FontDef {
  let n = existing.length + 1;
  let key = `font_${n}`;
  const keys = new Set(existing.map((f) => f.key));
  while (keys.has(key)) key = `font_${++n}`;
  return fd(key, "New font", "titan_one", 96, "#ffffff", "#ffffff", "#000000", 6);
}
