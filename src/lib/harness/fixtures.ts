/**
 * Headless-render fixtures. Canned Tauri IPC responses so app components render in a REAL browser
 * engine (Playwright's WebKit — the same family as the Tauri WKWebView, so it reproduces WebKit
 * layout bugs that jsdom/Chrome would miss) with NO Rust backend.
 *
 * `installMock` replaces `window.__TAURI_INTERNALS__.invoke` — the single hook every generated
 * command in `$lib/ipc/bindings.ts` funnels through (`@tauri-apps/api/core`'s `invoke`). Commands
 * wrapped in `typedError` resolve to the plain value (it wraps into `{status:"ok",data}`);
 * `deriveAssets` returns its array directly — so the mock just returns plain values by command name.
 *
 * All imagery is inline SVG data URLs (pure strings, no binary fixtures) so this module also runs
 * under plain node for data-shape checks.
 */

// ── SVG image generators (deterministic, alpha-correct) ────────────────────────────────────────
const svg = (w: number, h: number, body: string): string =>
  "data:image/svg+xml," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${body}</svg>`,
  );

const PALETTE = ["#7b4a2f", "#3f6d52", "#5a4b86", "#8a6d2f", "#4a6d8a", "#8a3f52", "#546b3a", "#2f6d6d"];
const hashIdx = (s: string): number => {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % PALETTE.length;
};

const symbolImg = (label: string, key: string): string =>
  svg(
    256,
    256,
    `<rect x="34" y="34" width="188" height="188" rx="26" fill="${PALETTE[hashIdx(key)]}"/>` +
      `<text x="128" y="170" font-size="92" font-family="sans-serif" font-weight="700" text-anchor="middle" fill="#fff">${label}</text>`,
  );
// Opaque plate: two bands (top lighter → bottom darker), stands in for a sky/base background.
const plateImg = (top: string, bottom: string): string =>
  svg(1024, 576, `<rect width="1024" height="576" fill="${bottom}"/><rect width="1024" height="330" fill="${top}"/>`);
// Transparent layer: soft cloud band (alpha), stacks over a plate.
const cloudsImg = (): string =>
  svg(
    1024,
    576,
    `<g fill="#c9d2dd" opacity="0.5"><ellipse cx="300" cy="180" rx="240" ry="70"/><ellipse cx="720" cy="140" rx="300" ry="80"/><ellipse cx="520" cy="250" rx="260" ry="60"/></g>`,
  );
// Anchored set-piece sprite (alpha).
const boothImg = (): string =>
  svg(
    512,
    640,
    `<rect x="120" y="200" width="272" height="420" rx="16" fill="#3a3f4a"/><rect x="150" y="120" width="212" height="110" rx="10" fill="#4a5060"/><rect x="180" y="250" width="152" height="120" fill="#8bd3e6" opacity="0.5"/>`,
  );
const mascotImg = (): string =>
  svg(
    1200,
    1800,
    `<ellipse cx="600" cy="520" rx="300" ry="340" fill="#6a4b8a"/><rect x="340" y="800" width="520" height="860" rx="120" fill="#543a6d"/><circle cx="500" cy="500" r="42" fill="#fff"/><circle cx="700" cy="500" r="42" fill="#fff"/>`,
  );
// Ornamental frame (transparent centre) — border-image-slice 128 in the preview.
const frameImg = (): string =>
  svg(
    1024,
    1024,
    `<rect x="60" y="60" width="904" height="904" rx="48" fill="none" stroke="#c8a24a" stroke-width="96"/><rect x="120" y="120" width="784" height="784" rx="28" fill="none" stroke="#8a6a2a" stroke-width="10"/>`,
  );
// Reel background panel — border-image-slice 96 fill in the preview.
const panelImg = (): string => svg(1024, 1024, `<rect width="1024" height="1024" rx="40" fill="#101722" opacity="0.86"/>`);

function imageFor(key: string): string {
  if (key.startsWith("symbol_")) return symbolImg(key.replace("symbol_", "").toUpperCase().slice(0, 3), key);
  if (key.includes("sky")) return plateImg("#3a4a63", "#141b28");
  if (key.includes("cloud")) return cloudsImg();
  if (key.includes("booth")) return boothImg();
  if (key.includes("mascot")) return mascotImg();
  if (key === "reel_frame") return frameImg();
  if (key === "reel_background") return panelImg();
  if (key.startsWith("bg_base")) return plateImg("#33405a", "#10151f");
  return symbolImg("?", key);
}

// ── Fixture data ───────────────────────────────────────────────────────────────────────────────
const SYMBOL_KEYS = ["h1", "h2", "h3", "h4", "h5", "l1", "l2", "l3", "l4", "l5", "wild", "scatter"];
// Records that exist (have generated art). `bg_sea_*` is deliberately ABSENT → the preview's
// Layers panel shows "Churning Sea" as a greyed, not-generated row.
const RECORD_KEYS = [
  ...SYMBOL_KEYS.map((k) => `symbol_${k}`),
  "bg_base_landscape",
  "bg_base_portrait",
  "mascot_hero",
  "reel_frame",
  "reel_background",
  "bg_sky_landscape",
  "bg_sky_portrait",
  "bg_clouds_landscape",
  "bg_clouds_portrait",
  "bg_booth",
];

const role = (k: string): string =>
  k === "wild" ? "wild" : k === "scatter" ? "scatter" : k.startsWith("h") ? "high" : "low";

const lightningConfig = {
  gameId: "mock",
  name: "Lightning in a Bottle",
  brief: "",
  stylePrompt: "",
  negativePrompt: "",
  winType: "lines",
  cols: 5,
  rows: 5,
  symbols: SYMBOL_KEYS.map((k) => ({ key: k, role: role(k), name: k.toUpperCase(), description: "" })),
  hasFeatureBackground: false,
  hasBuyBonus: false,
  buyBonusModes: [],
  hasMeter: false,
  meterThresholds: 0,
  hasMystery: false,
  holdAndSpin: false,
  hasMascot: true,
  mascotDescription: "a mad scientist in a leather apron",
  scene: {
    presets: [
      { key: "landscape", width: 2048, height: 1152 },
      { key: "portrait", width: 1242, height: 2208 },
    ],
    assets: [
      { key: "sky", kind: "plate", name: "Storm Sky", placement: { fit: "cover" }, variants: [{ key: "landscape" }, { key: "portrait" }] },
      { key: "clouds", kind: "layer", name: "Cloud Band", placement: { fit: "cover", blend: "screen" }, variants: [{ key: "landscape" }, { key: "portrait" }] },
      { key: "sea", kind: "layer", name: "Churning Sea", placement: { fit: "cover" }, variants: [{ key: "landscape" }, { key: "portrait" }] },
      { key: "booth", kind: "sprite", name: "Observation Booth", placement: { fit: "anchored", pos: [0.13, 0.92], anchor: [0.5, 1], height: 0.42 } },
    ],
  },
};

const descriptors = [
  { key: "bg_base_landscape", kind: "background", category: "backgrounds" },
  { key: "bg_base_portrait", kind: "background", category: "backgrounds" },
  { key: "mascot_hero", kind: "mascot", category: "mascot" },
];

const record = (key: string) => ({
  key,
  prompt: { subject: key, overridePrompt: "" },
  variations: [{ id: "v1", rawFile: `${key}/raw.png`, stages: [{ name: "webp", file: "stages/final.webp" }] }],
  activeVariation: "v1",
  templateOnly: false,
});

type Fixture = { project: unknown; descriptors: unknown; records: unknown[]; imageFor: (k: string) => string };
const FIXTURES: Record<string, Fixture> = {
  lightning: {
    project: { schemaVersion: 1, config: lightningConfig, createdAt: 0, updatedAt: 0 },
    descriptors,
    records: RECORD_KEYS.map(record),
    imageFor,
  },
};

export const FIXTURE_NAMES = Object.keys(FIXTURES);

/** Resolve one command against a fixture — the plain value the real invoke would resolve to. */
export function mockInvoke(fixtureName: string, cmd: string, args: Record<string, unknown> = {}): unknown {
  const fx = FIXTURES[fixtureName] ?? FIXTURES.lightning;
  switch (cmd) {
    case "get_project":
      return fx.project;
    case "derive_assets":
      return fx.descriptors;
    case "list_asset_records":
      return fx.records;
    case "get_variation_stage_image":
    case "get_variation_image":
      return fx.imageFor(String(args.assetKey ?? ""));
    default:
      return null; // any unmocked command resolves to null (typedError → {status:"ok",data:null})
  }
}

/** Install the mock IPC on `window` — call BEFORE the component under test mounts. */
export function installMock(fixtureName = "lightning"): void {
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    invoke: (cmd: string, a: Record<string, unknown> = {}) => Promise.resolve(mockInvoke(fixtureName, cmd, a)),
    transformCallback: (cb: unknown) => cb,
    unregisterCallback: () => {},
    convertFileSrc: (p: string) => p,
  };
}
