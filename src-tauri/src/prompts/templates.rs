//! Studio-wide, hand-authored prompt scaffolding. These are constant across games —
//! the per-game aesthetic lives in the game's style master, not here.
//!
//! The composition templates encode *slot-symbol* conventions (single centered subject,
//! bold readable silhouette, flat emblem framing, clean removable background) so outputs
//! read as real game symbols, not generic AI art. The negative master strips the slop.

use crate::model::game_config::SymbolRole;

/// Universal AI-slop / quality tells — safe to apply to EVERY asset (render style, plastic
/// sheen, glow spam, watermarks, deformities, low quality).
pub const SLOP_NEGATIVE: &str = "3d render, octane render, unreal engine, cgi, photorealistic, \
hyperrealistic, photograph, airbrushed, plastic, plasticky sheen, waxy, glossy, smooth gradients, \
blurry, soft focus, lens flare, HDR, bloom, oversaturated, neon, glow spam, stock photo, generic \
fantasy art, concept art sheet, artstation, trending on artstation, deviantart, watermark, \
signature, low detail, low quality, jpeg artifacts, deformed, mutated, disfigured";

/// Extra negatives for an ISOLATED emblem/subject (symbols, chrome, panels, UI marks): it must
/// sit alone on a clean removable background — so a scene/environment here is slop.
const ISOLATED_NEGATIVE: &str = "cluttered background, busy background, scene, landscape, \
environment, ground plane, horizon, multiple subjects, duplicated subject, floating objects, \
drop shadow, cast shadow, reflection, character turnaround, sticker, caption, off-center, cropped, \
out of frame, extra limbs, extra fingers";

/// Extra negatives for a full-bleed BACKGROUND scene: it must stay behind the reels, so a
/// centred subject, a busy/bright centre, or baked-in game UI is what breaks it.
const SCENE_NEGATIVE: &str = "foreground character, creature or mascot staged in the centre, hero \
subject in the middle, competing focal point, busy cluttered centre, bright high-contrast centre, \
baked-in UI, reels, reel frame, symbols, icons, buttons, meters, baked-in text, title, logo, \
border, frame";

/// Extra negatives for SPLASH hero art: a hero subject is welcome, but no baked game UI/text.
const SPLASH_NEGATIVE: &str = "baked-in UI, reels, buttons, meters, baked-in text, title text, \
logo lockup, watermark, caption, border, frame";

/// Symbol SET sheet: several symbols generated in ONE image so the model keeps them
/// stylistically consistent, laid on a strict uniform grid that the pipeline then
/// splits back into individual assets. Functional scaffolding only — aesthetics come
/// from the game's style master.
pub fn symbol_set_composition(rows: u32, cols: u32, n: usize, cells: &str) -> String {
    let empty = if (rows * cols) as usize > n {
        "; leave every remaining cell completely empty"
    } else {
        ""
    };
    format!(
        "A specimen sheet of {n} distinct slot-game symbols arranged in a STRICT uniform \
{rows} row by {cols} column grid of equal rectangular cells with generous even empty \
spacing between them — every symbol fully inside its own cell, never touching or \
crossing a cell boundary, and ALL symbols rendered in exactly the same art style, \
palette, lighting and level of finish. Reading order left to right, top to bottom: \
{cells}. Each symbol is a single bold centered emblem with a clean readable \
silhouette{empty}. The background is one single uniform flat surface across the ENTIRE \
image, with no dividing lines, no cell frames and no labels."
    )
}

/// Negative floor for a symbol-set sheet. The usual isolated-object negatives forbid
/// "multiple subjects" — which is the whole point of a sheet — so it gets its own.
pub fn symbol_set_negative() -> String {
    format!(
        "{SLOP_NEGATIVE}, scene, landscape, environment, ground plane, horizon, cast \
shadow, drop shadow, caption, text labels, numbered cells, grid lines, cell borders, \
frames around cells, symbols overlapping cell boundaries, symbols merging together, \
uneven cell sizes, mixed art styles"
    )
}

/// Scene LAYER: a transparent element that stacks over a plate at the same canvas —
/// e.g. the storm sky seen inside a window arch. Positioned where it belongs on the
/// canvas, everything else transparent.
pub fn scene_layer_composition() -> &'static str {
    "a scene element on a fully transparent background: {subject}, drawn complete with \
clean crisp silhouette edges, positioned on the canvas exactly where it sits in the scene \
and painted at the same camera angle, perspective and lighting as the background plate it \
will stack over; everything that is not the element itself stays fully transparent — no \
backdrop, no ground plane, no vignette, no frame"
}

/// Negative floor for scene layers (the usual isolated-object negatives fight a layer's
/// intentional off-centre placement, so it gets its own).
pub fn scene_layer_negative() -> String {
    format!(
        "{SLOP_NEGATIVE}, opaque background, backdrop, background scenery behind the \
element, solid background fill, vignette, frame, border"
    )
}

/// Scene SPRITE: a standalone set-piece object with alpha — like a symbol technically,
/// but staged naturally, not as a grid emblem.
pub fn scene_sprite_composition() -> &'static str {
    "a standalone set-piece object: {subject}, one complete object presented at a natural \
scene angle with natural proportions — a piece of the game's world, not an emblem or icon; \
isolated on a plain empty removable background, no scene behind it, no ground shadow"
}

/// Scene FX still: the source frame of a short looping effect (flame, steam, arc).
pub fn scene_fx_composition() -> &'static str {
    "a single effect element: {subject}, one self-contained volumetric effect centered \
with clear margin on every side, soft natural edges, designed to animate as a short \
seamless loop; isolated on a plain empty removable background, no scene, no emitting \
object unless described, no ground"
}

/// Scene PARTICLE: a tiny single-sprite texture for the particle system.
pub fn scene_particle_composition() -> &'static str {
    "a single tiny particle texture: {subject}, one small simple element with soft \
edges, centered on a plain empty removable background; minimal detail — it will be \
drawn very small and many times; no scene, no variations, exactly one element"
}

/// Horizontally tileable textures (panning cloud bands): the left and right edges must
/// continue each other exactly.
pub fn tileable_clause() -> &'static str {
    " The image must TILE SEAMLESSLY in the horizontal direction: the left and right \
edges continue each other exactly — matching colours, lighting and shapes across the \
wrap point, no element cut off at one edge without continuing from the other, no \
vignette or lighting falloff toward the sides."
}

/// Interior cut-outs: see-through openings generated as flat chroma fills, keyed to
/// real transparency by Process (works ANYWHERE in the image, not just the border).
pub fn cutout_clause() -> &'static str {
    " Every see-through opening in the element (window glass, archways, gaps where \
deeper layers must show through) is rendered as a single perfectly flat, uniform \
magenta (#FF00FF) fill — no gradient, texture, reflection or highlight inside those \
areas, and that magenta colour appears nowhere else in the image."
}

/// Key-level composition overrides, for categories that hold structurally DIFFERENT
/// assets. §4 reel chrome is the case in point: `reel_frame` is a hollow border,
/// but `reel_background` is a fully FILLED panel that sits behind the symbols and
/// `reel_anticipation_glow` is a soft light strip — sharing the frame template made
/// all three generate frames.
pub fn composition_override(asset_key: &str) -> Option<&'static str> {
    Some(match asset_key {
        "reel_background" => {
            "a flat decorative backdrop panel: {subject}, one continuous fully-filled \
surface covering the ENTIRE canvas edge to edge — this panel sits BEHIND the spinning \
symbols, so keep it calm, even and low-contrast, with subtle large-scale texture and no \
focal point; straight-on and flat, no border, no frame, no rim, no opening, no hole, no \
window, no vignette, no scene, no objects, no baked-in symbols or text"
        }
        "reel_anticipation_glow" => {
            "a soft luminous glow overlay: {subject}, one even vertical column of light \
with softly feathered edges, brightest along the centre and fading smoothly to nothing at \
the sides, presented flat on a plain empty background; no hard edges, no border, no frame, \
no scene, no objects, no text"
        }
        _ => return None,
    })
}

/// Key-level EXTRA negatives paired with the overrides above (appended to the
/// category floor).
pub fn key_negative(asset_key: &str) -> Option<&'static str> {
    Some(match asset_key {
        "reel_background" => {
            ", frame, border, ornamental rim, hollow centre, empty opening, window, \
transparent hole, margin around the panel"
        }
        "reel_anticipation_glow" => ", frame, border, ornamental rim, hard outline",
        _ => return None,
    })
}

/// The combined negative for a category (universal slop + the category-appropriate extras).
pub fn category_negative(category: &str) -> String {
    let extra = match category {
        "backgrounds" => SCENE_NEGATIVE,
        "splash" => SPLASH_NEGATIVE,
        _ => ISOLATED_NEGATIVE,
    };
    format!("{SLOP_NEGATIVE}, {extra}")
}

/// Reels-safe background composition, aware of orientation (landscape/portrait) and whether
/// it's the heightened feature/free-spins background. Full-bleed, calm dim centre, interest
/// pushed to the edges so symbols stay readable on top.
pub fn background_composition(subject: &str, is_portrait: bool, is_feature: bool) -> String {
    let orient = if is_portrait {
        "a tall vertical portrait (9:16) full-bleed slot-game background"
    } else {
        "a wide landscape (16:9) full-bleed slot-game background"
    };
    let mood = if is_feature {
        "a heightened feature / free-spins mood — richer, more dramatic light and colour than the \
base game, yet still calm through the middle"
    } else {
        "a restrained base-game mood"
    };
    format!(
        "{orient} depicting {subject}: a cohesive, atmospheric establishing environment with real \
depth and layered detail toward the edges and corners, in {mood}. CRITICAL — keep the central \
band calm, dim, low-contrast and uncluttered: the spinning reels and symbols sit on top of it and \
must stay perfectly legible, so no busy detail, no bright highlights and no focal point in the \
middle; concentrate ornament, structure and light around the periphery with a gentle darkening \
toward the centre. Edge-to-edge, full-bleed, no border or margin. No character, creature, mascot \
or foreground subject staged in the frame, and no baked-in UI, reels, symbols, text or logo."
    )
}

/// Composition for the per-game STYLE ANCHOR: a pure style-calibration image that every
/// generation attaches as its first reference. Deliberately NOT a game asset — no symbol,
/// no scene — so it can anchor the look without ever bleeding subject matter. Framed as a
/// craftsman's specimen board, NOT an ornament collage: sparse, disciplined, restrained —
/// the anti-slop qualities the anchor is supposed to transmit to every asset.
pub fn style_anchor_composition() -> &'static str {
    "an art-style specimen board for a slot-game asset set: exactly eight separate sample \
pieces laid out in a loose, orderly grid on one flat, deep, neutral ground with calm space \
between them, together covering the range of assets the set needs — one frame corner \
fragment, one simple orb showing the material shading, one short fold of fabric, one \
small solid trinket, one carved or weathered surface-texture swatch, one small sprig of \
organic growth, one blank emblem or medallion shape, and one small square swatch showing \
how distant scenery and sky are painted. Every piece is rendered exactly the way the \
game's assets will be rendered: crisp confident silhouette, one single consistent light \
direction shared by all pieces, a disciplined palette of few hues, deliberate \
hand-crafted shading with visible decisive strokes, an even, consistent finish, at most \
one restrained accent colour across the whole board. Curated and quiet like a \
craftsman's reference sheet — samples in a grid, never a collage. It must NOT depict any \
recognizable game symbol, creature, character, object of play, scene, logo or text; it \
exists purely to demonstrate medium, linework weight, palette, lighting and surface finish"
}

/// The anchor's negative: universal anti-slop floor plus the collage/gloss failure modes
/// this particular composition invites.
pub fn style_anchor_negative() -> String {
    format!(
        "{SLOP_NEGATIVE}, kaleidoscope symmetry, ornamental collage filling every corner, \
horror vacui, busy allover pattern, overlapping jumbled objects, oversaturated rainbow \
palette, neon glow, HDR bloom, glossy plastic 3D render, airbrushed smooth gradients, \
depth of field, bokeh, drop shadows in every direction, baked-in text, logo, watermark, \
full border around the image"
    )
}

/// Appended to a background composition when the scene will be CUT into parallax depth
/// layers: the segmenter and per-layer fills need planes that separate cleanly.
pub fn parallax_layers_clause() -> &'static str {
    " Compose the scene as clearly separable parallax depth planes — a far sky or void \
backdrop, a distant background, a mid-ground, and nearer scenery at the edges — each plane \
reading as its own band with a crisp, clean silhouette against the plane behind it. Keep \
every plane in sharp focus edge to edge: no depth-of-field blur, no soft-focus foreground. \
Atmosphere (fog, haze, glow, particles) may live INSIDE one plane but must never bleed or \
gradient across a plane boundary. Favour slightly stronger tonal and value separation \
between the planes than usual, so each depth band is unmistakable."
}

/// Per-category composition template. `{subject}` is substituted with the asset's subject.
/// Encodes the shot/framing/background rules so the aesthetic (style master) stays separate
/// from the composition.
pub fn composition_template(category: &str) -> &'static str {
    match category {
        "symbols" => "{subject}, rendered as a single bold slot-machine game symbol: one \
self-contained subject, centered and facing forward, {fill}, with a strong, instantly readable \
silhouette that stays legible at small thumbnail size; chunky confident forms, crisp clean edges, \
even frontal lighting, at most one restrained accent colour; presented flat and emblem-like, isolated \
on a plain empty background with an even margin — no scene, no environment, no second subject, no \
ground, no cast shadow, no watermark or caption",

        // `backgrounds` is handled by `background_composition` (orientation/feature-aware).
        "splash" => "hero splash key art of {subject}: one bold cinematic focal composition with \
calm negative space in the upper-center reserved for a logo overlay; no on-image text",

        // NOTE: `reel_background` and `reel_anticipation_glow` do NOT use this — they have
        // key-level overrides in `composition_override` (a filled panel and a glow strip
        // are structurally different from a hollow frame).
        "reel_chrome" => "an ornamental frame / border element: {subject}, symmetrical and \
straight-on, with a completely hollow empty transparent center (only the surrounding border is \
drawn) designed to sit around the reels; flat presentation, no scene, nothing inside the opening",

        "symbol_chrome" => "a small clean UI decoration: {subject}, a single simple element \
centered on a plain empty background, flat and straight-on, minimal detail so it reads at small \
size; no scene, no cast shadow",

        "panels" => "an ornamental UI panel / plaque: {subject}, symmetrical and straight-on, with \
a simple flat center area kept clear for text and a restrained decorative trim; presented flat on a plain \
empty background, no scene, no baked-in text",

        "buy_bonus" => "a premium buy-bonus UI card: {subject}, a single bold centered emblem, \
straight-on on a plain empty background, clear and legible with at most one restrained accent colour; no \
baked-in text",

        "payline" => "a small flat UI indicator: {subject}, a single minimal centered graphic on a \
plain empty background, crisp and legible, no scene, no baked-in text",

        "mascot" => "{subject}, the game's mascot character shown FULL BODY from head to feet, \
standing in a relaxed, readable pose facing slightly toward the viewer; arms held slightly away \
from the body and any held prop kept clear of the torso so every limb reads as its own shape \
(the art will be cut into parts and rigged for skeletal animation); strong expressive \
silhouette, crisp clean edges, even lighting, at most one restrained accent colour; isolated on a \
plain empty background with generous margin — no scene, no ground, no cast shadow, no second \
character, no text",

        "branding" => "a game logo emblem / crest for {subject}: one bold iconic centered mark on a \
plain empty background, illustrated and hand-crafted (not photographic), no \
extra scene",

        _ => "{subject}, a single clean centered subject isolated on a plain empty background, flat \
and straight-on, no scene, no cast shadow",
    }
}

/// How much of the frame a symbol's subject should occupy, by tier. High-tier (and the hero
/// special) symbols read bigger than low-tier ones — the studio wants ~85% for high, ~60% for
/// low. Substituted into the `symbols` composition's `{fill}` slot. Must stay in sync with
/// the `fill_target` normalization in `commands/process.rs`, which makes this true post-hoc.
pub fn symbol_fill_clause(role: SymbolRole) -> &'static str {
    match role {
        SymbolRole::Low => "sized so the subject occupies roughly 60% of the frame — a clear, \
comfortable margin of empty space all around it",
        // High tier and every feature symbol (wild/scatter/bonus/special) are hero symbols and
        // read large.
        _ => "sized so the subject occupies roughly 85% of the frame — a large, dominant hero \
read with only a slim even margin of empty space around it",
    }
}

/// Role-specific addendum for a **special** symbol (wild / scatter / bonus / special),
/// appended to the base `symbols` composition. High/Low symbols get `None` (unchanged).
///
/// Special symbols follow slot conventions the common tiers don't: a premium ornate frame,
/// a restrained glow, and — for wild/scatter/bonus — a banner lettering the feature word
/// (WILD / SCATTER / BONUS) baked into the artwork. Returns `(clause, has_label)`; when
/// `has_label` is true the caller relaxes the "no caption" rules so the label survives.
/// The feature word a special symbol carries (WILD / SCATTER / BONUS / a short
/// special name). The word is NEVER lettered into the symbol art — localization:
/// the game typesets the translated word at runtime — so this instead drives which
/// symbols derive a separate empty `symbol_<key>_banner` asset to carry it.
pub fn symbol_label(role: SymbolRole, name: &str) -> Option<String> {
    match role {
        SymbolRole::Wild | SymbolRole::ExpandingWild => Some("WILD".into()),
        SymbolRole::Scatter => Some("SCATTER".into()),
        SymbolRole::Bonus => Some("BONUS".into()),
        SymbolRole::Special => {
            // A short, single-word-ish special (e.g. "Mystery") carries a label; a
            // long themed name does not.
            let words = name.split_whitespace().count();
            let trimmed = name.trim();
            if !trimmed.is_empty() && words <= 2 && trimmed.chars().count() <= 12 {
                Some(trimmed.to_uppercase())
            } else {
                None
            }
        }
        SymbolRole::High | SymbolRole::Low => None,
    }
}

pub fn special_symbol_clause(role: SymbolRole, name: &str) -> Option<String> {
    if matches!(role, SymbolRole::High | SymbolRole::Low) {
        return None;
    }
    let label = symbol_label(role, name);

    let role_hint = match role {
        SymbolRole::Wild | SymbolRole::ExpandingWild => "it substitutes for other symbols, so \
make it the boldest, most dominant symbol on the reels",
        SymbolRole::Scatter => "it pays anywhere and triggers the free spins, often built around a \
radiant coin, gem, star or medallion motif",
        SymbolRole::Bonus => "it opens a bonus feature, often built around a chest, coin, key or \
token motif",
        SymbolRole::Special => "it drives a special feature and should clearly stand apart from the \
common symbols",
        _ => "",
    };

    let premium = "render it as a standout feature symbol: a touch larger and more elaborate than a \
common symbol, framed in a distinct emblem border with a restrained glow so it stands \
out on the reels";

    // Localization rule: the feature word is NEVER painted into the art. The game
    // typesets the translated word at runtime onto the symbol's separate banner asset.
    let clause = match &label {
        Some(_) => format!(
            " — this is the special feature symbol; {role_hint}. {premium}. Do NOT letter \
any word onto the artwork: absolutely no text, letters, numbers, lettered banner or \
inscribed ribbon — the localized feature word is applied by the game at runtime on a \
separate banner asset"
        ),
        None => format!(" — this is the special feature symbol; {role_hint}. {premium}"),
    };
    Some(clause)
}
