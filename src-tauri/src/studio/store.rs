//! On-disk layout for the Animation Studio, under `assets/<key>/studio/`:
//!
//! ```text
//! studio.json                 canonical StudioDoc (atomic writes)
//! source.png                  frozen snapshot of the processed variation
//! sam/embedding.st(+json)     cached SAM image-encoder output
//! parts/<id>/mask.png         8-bit gray, full source dimensions
//! parts/<id>/cut.png          RGBA original pixels lifted by mask, cropped to bbox
//! parts/<id>/completed.<h>.png  inpaint-completed texture (h = 8-hex cache key)
//! export/skeleton.json + <key>.atlas + <key>.webp/.png   Spine 4.2 export set
//! ```
//!
//! All functions take an explicit `base` (projects root) so they are testable without
//! Tauri, mirroring `crate::storage`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::storage;

use super::doc::{PartTexture, StudioDoc};

pub fn studio_dir(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    storage::asset_dir(base, game_id, asset_key).join("studio")
}

pub fn part_dir(base: &Path, game_id: &str, asset_key: &str, part_id: &str) -> PathBuf {
    studio_dir(base, game_id, asset_key).join("parts").join(part_id)
}

pub fn export_dir(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    studio_dir(base, game_id, asset_key).join("export")
}

pub fn source_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    studio_dir(base, game_id, asset_key).join("source.png")
}

pub fn doc_path(base: &Path, game_id: &str, asset_key: &str) -> PathBuf {
    studio_dir(base, game_id, asset_key).join("studio.json")
}

/// Load the studio doc, or `None` if this asset has no studio yet.
pub fn read_doc(base: &Path, game_id: &str, asset_key: &str) -> Result<Option<StudioDoc>, String> {
    let path = doc_path(base, game_id, asset_key);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read studio doc failed: {e}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("parse studio doc failed: {e}"))
}

pub fn write_doc(base: &Path, game_id: &str, asset_key: &str, doc: &StudioDoc) -> Result<(), String> {
    storage::atomic_write_json(&doc_path(base, game_id, asset_key), doc)
}

/// The image file that currently feeds the atlas for a part (cut vs inpaint-completed).
pub fn part_texture_path(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    part: &super::doc::Part,
) -> PathBuf {
    let dir = part_dir(base, game_id, asset_key, &part.id);
    match (&part.texture, &part.completed_hash) {
        (PartTexture::Completed, Some(h)) => dir.join(format!("completed.{h}.png")),
        _ => dir.join("cut.png"),
    }
}

/// Write a file under the studio dir, creating parent dirs.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create dir failed: {e}"))?;
    }
    fs::write(path, bytes).map_err(|e| format!("write {} failed: {e}", path.display()))
}

/// Self-heal a doc against drift (crashed sessions, hand edits, version skew) so the
/// studio always opens into a workable state instead of erroring. Returns true if the
/// doc was changed. Repairs: missing root bone; slots whose part/texture is missing or
/// not cut (dropped); slots bound to missing bones (rebound to root); physics and clip
/// timelines aimed at missing targets (pruned); zero clips (reseeded root idle).
pub fn sanitize(base: &Path, game_id: &str, asset_key: &str, doc: &mut StudioDoc) -> bool {
    let mut changed = false;

    if !doc.bones.iter().any(|b| b.name == "root") {
        let (cx, cy) = (doc.source.width as f64 / 2.0, doc.source.height as f64 / 2.0);
        doc.bones.insert(0, crate::studio::doc::Bone::new("root", None, cx, cy));
        changed = true;
    }

    // Slots must point at an existing, cut part whose texture file exists.
    let before = doc.slots.len();
    let parts = doc.parts.clone();
    doc.slots.retain(|s| {
        parts.iter().any(|p| {
            p.id == s.part_id
                && p.effective_bbox().is_some()
                && part_texture_path(base, game_id, asset_key, p).is_file()
        })
    });
    changed |= doc.slots.len() != before;

    // Alternate attachments must reference a real, cut part (else skin emission errors).
    let valid_parts: std::collections::HashSet<&str> = parts
        .iter()
        .filter(|p| {
            p.effective_bbox().is_some() && part_texture_path(base, game_id, asset_key, p).is_file()
        })
        .map(|p| p.id.as_str())
        .collect();
    for s in &mut doc.slots {
        let before = s.alternates.len();
        s.alternates.retain(|a| valid_parts.contains(a.as_str()));
        changed |= s.alternates.len() != before;
    }

    // Slots bound to vanished bones fall back to root (part stays visible, unrigged).
    let bone_names: Vec<String> = doc.bones.iter().map(|b| b.name.clone()).collect();
    for s in &mut doc.slots {
        if !bone_names.contains(&s.bone) {
            s.bone = "root".into();
            changed = true;
        }
    }

    let before = doc.physics.len();
    doc.physics.retain(|p| bone_names.contains(&p.bone));
    changed |= doc.physics.len() != before;

    let slot_names: Vec<String> = doc.slots.iter().map(|s| s.name.clone()).collect();
    for clip in &mut doc.clips {
        let before = clip.timelines.len();
        clip.timelines.retain(|tl| match &tl.target {
            crate::studio::doc::TimelineTarget::BoneRotate(b)
            | crate::studio::doc::TimelineTarget::BoneTranslate(b)
            | crate::studio::doc::TimelineTarget::BoneScale(b) => bone_names.contains(b),
            crate::studio::doc::TimelineTarget::SlotAlpha(s)
            | crate::studio::doc::TimelineTarget::SlotColor(s)
            | crate::studio::doc::TimelineTarget::SlotDeform(s)
            | crate::studio::doc::TimelineTarget::SlotAttachment(s) => slot_names.contains(s),
        });
        changed |= clip.timelines.len() != before;
    }

    // Deform/attachment timelines go stale when a mesh's vertex count or a slot's alternate list
    // changes; drop the ones that would break emit() and clamp out-of-range swap indices.
    let mesh_len: std::collections::HashMap<String, Option<usize>> = doc
        .slots
        .iter()
        .map(|s| {
            let len = match &s.attachment {
                crate::studio::doc::Attachment::Mesh(m) => Some(m.vertices.len()),
                _ => None,
            };
            (s.name.clone(), len)
        })
        .collect();
    let att_count: std::collections::HashMap<String, usize> = doc
        .slots
        .iter()
        .map(|s| (s.name.clone(), 1 + s.alternates.len()))
        .collect();
    for clip in &mut doc.clips {
        let before = clip.timelines.len();
        clip.timelines.retain(|tl| match &tl.target {
            crate::studio::doc::TimelineTarget::SlotDeform(s) => match mesh_len.get(s) {
                Some(Some(len)) => tl.keys.iter().all(|k| k.v.len() == *len),
                _ => false, // slot vanished or no longer a mesh
            },
            _ => true,
        });
        changed |= clip.timelines.len() != before;
        for tl in &mut clip.timelines {
            if let crate::studio::doc::TimelineTarget::SlotAttachment(s) = &tl.target {
                let n = *att_count.get(s).unwrap_or(&1) as f64;
                for k in &mut tl.keys {
                    if let Some(v0) = k.v.first_mut() {
                        if *v0 >= n {
                            *v0 = 0.0;
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    let before = doc.clips.len();
    doc.clips.retain(|c| !c.timelines.is_empty());
    changed |= doc.clips.len() != before;
    if doc.clips.is_empty() {
        doc.clips.push(StudioDoc::breathe_idle("root"));
        changed = true;
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{SourceRef, StudioDoc};

    #[test]
    fn sanitize_repairs_drifted_docs() {
        let base = std::env::temp_dir().join(format!("wf_studio_sanitize_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (gid, key) = ("g", "sym");

        // Drifted doc modeled on a real crash artifact: parts never cut (no bbox), slots
        // referencing missing bones/parts, physics + clips aimed at ghosts, no root.
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 100, height: 80, sha256: "s".into() },
            0.0,
        );
        doc.parts[0].bbox = None; // "all" was never cut
        doc.bones.retain(|b| b.name != "root");
        doc.slots.push(crate::studio::doc::Slot {
            name: "ghost".into(),
            bone: "nope".into(),
            part_id: "ghost".into(),
            attachment: Default::default(),
            blend: Default::default(),
            alternates: Default::default(),
        });
        doc.physics = vec![crate::studio::doc::PhysicsSpec::sway("nope")];

        assert!(sanitize(&base, gid, key, &mut doc));
        assert_eq!(doc.bones[0].name, "root");
        assert!(doc.slots.is_empty(), "uncut/ghost slots dropped");
        assert!(doc.physics.is_empty());
        // Clips retargeted safely: body bone still exists so idle survives…
        assert!(!doc.clips.is_empty());
        // …and the result emits without error despite having no drawable slots.
        assert!(crate::studio::spine42::emit(&doc).is_ok());

        // A healthy doc is left untouched (needs its texture on disk to keep the slot).
        let mut healthy = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 4, height: 4, sha256: "s".into() },
            0.0,
        );
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        write_file(&part_dir(&base, gid, key, "all").join("cut.png"), &png.into_inner()).unwrap();
        assert!(!sanitize(&base, gid, key, &mut healthy));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn doc_round_trip() {
        let base = std::env::temp_dir().join(format!("wf_studio_store_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let doc = StudioDoc::seed(
            SourceRef { variation_id: "v001".into(), width: 800, height: 600, sha256: "ab".into() },
            1.0,
        );
        write_doc(&base, "g", "symbol_h1", &doc).unwrap();
        let back = read_doc(&base, "g", "symbol_h1").unwrap().unwrap();
        assert_eq!(back, doc);
        assert!(read_doc(&base, "g", "nope").unwrap().is_none());
        let _ = fs::remove_dir_all(&base);
    }
}
