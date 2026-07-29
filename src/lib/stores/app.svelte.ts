import type { GameConfig } from "$lib/ipc";

/** Top-level navigation: the masthead sections.
 *  - `library`: the game catalogue (create / open).
 *  - `produce`: one game's Producer — everything about MAKING images.
 *  - `animate`: the Animate section — asset rail + studio/layers for MOVING them.
 *  - `preview`: the Game preview — the real assets composed into a slot screen.
 *  The open game is STICKY: visiting the Library keeps it loaded in the other sections
 *  until another game is opened. */
export type Section = "library" | "produce" | "animate" | "sound" | "fonts" | "preview";

export const appState = $state<{
  section: Section;
  /** The open game (sticky). `null` = none open — or a brand-new unsaved game while
   *  `newGame` is set (Produce shows the forced Blueprint). */
  gameId: string | null;
  newGame: boolean;
  /** Per-section asset selections. */
  produceAssetKey: string | null;
  animateAssetKey: string | null;
  settingsOpen: boolean;
  dataRev: number;
}>({
  section: "library",
  gameId: null,
  newGame: false,
  produceAssetKey: null,
  animateAssetKey: null,
  settingsOpen: false,
  dataRev: 0,
});

/** Bump to signal that on-disk project data changed (e.g. the projects folder moved). */
export function bumpData() {
  appState.dataRev++;
}

/** Switch masthead section. Produce/Animate need an open game (or an in-progress
 *  new-game blueprint for Produce). */
export function goSection(s: Section) {
  if (s !== "library" && !appState.gameId && !(s === "produce" && appState.newGame)) return;
  appState.section = s;
}

export function goProjects() {
  appState.section = "library";
}

/** Open a game's Producer. `gameId: null` starts a brand-new game (Blueprint forced). */
export function goProducer(gameId: string | null, assetKey: string | null = null) {
  if (gameId === null) {
    appState.gameId = null;
    appState.newGame = true;
    appState.produceAssetKey = null;
    appState.animateAssetKey = null;
  } else {
    if (appState.gameId !== gameId) {
      appState.produceAssetKey = null;
      appState.animateAssetKey = null;
    }
    appState.gameId = gameId;
    appState.newGame = false;
    if (assetKey !== null) appState.produceAssetKey = assetKey;
  }
  appState.section = "produce";
}

/** Back-compat alias used by the Library list (open / new game). */
export function goEditor(gameId: string | null) {
  goProducer(gameId, null);
}

/** A game was deleted — drop it as the open context if it was. */
export function gameDeleted(gameId: string) {
  if (appState.gameId === gameId) {
    appState.gameId = null;
    appState.newGame = false;
    appState.produceAssetKey = null;
    appState.animateAssetKey = null;
    appState.section = "library";
  }
}

/** A brand-new game was saved — it becomes the open (sticky) game. */
export function gameCreated(gameId: string) {
  appState.gameId = gameId;
  appState.newGame = false;
}

/** Select an asset in the Producer. */
export function selectAsset(assetKey: string | null) {
  appState.produceAssetKey = assetKey;
}

/** Open the Animate section on an asset (Studio for symbols/mascots, Layers for
 *  backgrounds — the section decides by kind). */
export function goStudio(gameId: string, assetKey: string) {
  appState.gameId = gameId;
  appState.newGame = false;
  appState.animateAssetKey = assetKey;
  appState.section = "animate";
}

/** Same destination — kept for existing call sites (Layers is part of Animate now). */
export function goLayers(gameId: string, assetKey: string) {
  goStudio(gameId, assetKey);
}

/** Select an asset within the Animate section. */
export function selectAnimateAsset(assetKey: string | null) {
  appState.animateAssetKey = assetKey;
}

/** Open the Sound studio — manage/generate the game's music + SFX cues. */
export function goSound(gameId: string) {
  appState.gameId = gameId;
  appState.newGame = false;
  appState.section = "sound";
}

/** Open the Fonts studio — edit/preview the game's bitmap fonts. */
export function goFonts(gameId: string) {
  appState.gameId = gameId;
  appState.newGame = false;
  appState.section = "fonts";
}

/** Open the Game preview for a game (the real assets composed into a slot screen). */
export function goPreview(gameId: string) {
  appState.gameId = gameId;
  appState.newGame = false;
  appState.section = "preview";
}

export function openSettings() {
  appState.settingsOpen = true;
}
export function closeSettings() {
  appState.settingsOpen = false;
}

/** A sensible default config for a new project (a 5×3 lines game). */
export function newConfig(): GameConfig {
  const highs = ["h1", "h2", "h3", "h4", "h5"];
  const lows = ["l1", "l2", "l3", "l4", "l5"];
  return {
    gameId: "",
    name: "",
    brief: "",
    stylePrompt: "",
    negativePrompt: "",
    winType: "lines",
    cols: 5,
    rows: 3,
    symbols: [
      ...highs.map((key) => ({ key, role: "high" as const, name: key.toUpperCase(), description: "" })),
      ...lows.map((key) => ({ key, role: "low" as const, name: key.toUpperCase(), description: "" })),
      { key: "wild", role: "wild", name: "Wild", description: "" },
      { key: "scatter", role: "scatter", name: "Scatter", description: "" },
    ],
    hasFeatureBackground: true,
    hasBuyBonus: false,
    buyBonusModes: [],
    hasMeter: false,
    meterThresholds: 0,
    hasMystery: false,
    holdAndSpin: false,
    hasMascot: false,
    mascotDescription: "",
  };
}
