//! Animation Studio: rig + animate AI-generated assets into Spine 4.2 skeletons that play
//! on the exact runtime Stake's web-sdk ships (`@esotericsoftware/spine-pixi-v8`).
//!
//! Export-first architecture: the studio preview plays the same skeleton JSON + atlas the
//! export writes, so what the user sees is what the game engine plays. The canonical
//! document is [`doc::StudioDoc`]; Spine JSON is a pure derivation of it (`spine42`).

pub mod atlas;
pub mod doc;
pub mod fx;
pub mod inpaint;
pub mod matte;
pub mod mesh;
pub mod outline;
pub mod motion;
pub mod motion_gen;
pub mod preview;
pub mod regions;
pub mod rig;
pub mod sam;
pub mod segment;
pub mod spine42;
pub mod store;
pub mod turn;

use sha2::{Digest, Sha256};

/// Hex sha256 of a byte slice (cache keys, source identity).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
