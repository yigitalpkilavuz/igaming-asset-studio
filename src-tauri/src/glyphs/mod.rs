//! Glyph-on-base composition: symbol sets that share one pixel-identical base template
//! where only a central mark differs (five brass plates, five engraved glyphs). The
//! base is generated ONCE as its own asset (e.g. `symbol_l_base`); each symbol picks
//! that base, describes its mark, and the mark is AI-generated as a clean flat shape
//! and embossed on with the house lighting (single light from the lower-left).
//!
//! Layout:
//! - `<projects_root>/_glyphs/<id>/{shape.png, glyph.json}` — cross-game shape library
//!   (the underscore folder has no project.json, so the game list never picks it up).
//! - `<project>/assets/<key>/glyph.json` — that asset's base choice + prompt + placement.

pub mod doc;
pub mod emboss;
pub mod shape;
pub mod store;
