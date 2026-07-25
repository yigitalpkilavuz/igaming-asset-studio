//! Layered parallax backgrounds: split an APPROVED background concept into aligned depth
//! layers (sky / far / mid / foreground) that stack back into the exact source image.
//!
//! Anti-drift by construction: layers are cut from the approved pixels via the shared
//! matte machinery (`studio::matte`); only the bands hidden behind nearer layers are
//! AI-filled (`studio::inpaint` core, original pixels always win). The canonical document
//! is [`doc::LayersDoc`]; the export set under `assets/<key>/layers/export/` is what the
//! game ships (`export::build_dist` gives it precedence over the flat raster).

pub mod depth;
pub mod doc;
pub mod export;
pub mod fill;
pub mod propose;
pub mod split;
pub mod store;
