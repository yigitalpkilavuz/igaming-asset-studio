//! Build the studio's playable artifacts. `bundle()` (for the in-app preview) and
//! `export()` (for dist) run the SAME `spine42::emit` + `atlas::pack` path — the
//! export-first guarantee: what the preview plays is byte-for-byte what ships.

use std::fs;
use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::doc::StudioDoc;
use super::{atlas, spine42, store};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PageImage {
    /// File name the atlas text references, e.g. `symbol_h1.webp`.
    pub name: String,
    /// `data:image/webp;base64,…` for direct loading in the webview.
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBundle {
    pub skeleton_json: String,
    pub atlas_text: String,
    pub pages: Vec<PageImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StudioExportReport {
    pub dir: String,
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

/// Emit skeleton JSON + pack the atlas from the parts' current textures on disk.
fn build_artifacts(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
) -> Result<(String, atlas::PackResult), String> {
    let skeleton = spine42::emit(doc)?;
    let skeleton_json = serde_json::to_string_pretty(&skeleton)
        .map_err(|e| format!("serialize skeleton failed: {e}"))?;

    let mut inputs: Vec<atlas::PackInput> = Vec::new();
    for part in &doc.parts {
        if part.bbox.is_none() {
            continue; // not cut yet — spine42::emit already errored if a slot needs it
        }
        let path = store::part_texture_path(base, game_id, asset_key, part);
        let img = image::open(&path)
            .map_err(|e| format!("open part texture {}: {e}", path.display()))?
            .to_rgba8();
        inputs.push(atlas::PackInput { name: part.id.clone(), image: img });
    }
    if inputs.is_empty() {
        return Err("no cut parts yet — segment and cut parts in the Cut tab first".into());
    }
    let pack = atlas::pack(&inputs, asset_key)?;
    Ok((skeleton_json, pack))
}

/// Build the in-memory preview bundle for the given (possibly unsaved) doc.
pub fn bundle(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
) -> Result<PreviewBundle, String> {
    let (skeleton_json, pack) = build_artifacts(base, game_id, asset_key, doc)?;
    let pages = pack
        .pages
        .into_iter()
        .map(|p| PageImage {
            name: p.name,
            data_url: format!(
                "data:image/webp;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&p.webp)
            ),
        })
        .collect();
    Ok(PreviewBundle { skeleton_json, atlas_text: pack.atlas_text, pages })
}

/// Write the export set to `assets/<key>/studio/export/`.
pub fn export(
    base: &Path,
    game_id: &str,
    asset_key: &str,
    doc: &StudioDoc,
) -> Result<StudioExportReport, String> {
    let (skeleton_json, pack) = build_artifacts(base, game_id, asset_key, doc)?;
    let dir = store::export_dir(base, game_id, asset_key);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("create export dir failed: {e}"))?;

    let mut files = Vec::new();
    let mut write = |name: &str, bytes: &[u8]| -> Result<(), String> {
        store::write_file(&dir.join(name), bytes)?;
        files.push(name.to_string());
        Ok(())
    };
    write("skeleton.json", skeleton_json.as_bytes())?;
    write(&format!("{asset_key}.atlas"), pack.atlas_text.as_bytes())?;
    for page in &pack.pages {
        write(&page.name, &page.webp)?;
        // Debug/tooling copy alongside the shipped webp.
        write(&format!("{}.png", page.name.trim_end_matches(".webp")), &page.png)?;
    }

    let mut warnings = Vec::new();
    if doc.clips.is_empty() {
        warnings.push("no animation clips — skeleton exports with none".into());
    }
    if !doc.clips.iter().any(|c| c.name == "idle") {
        warnings.push("no \"idle\" clip — games usually expect one".into());
    }

    Ok(StudioExportReport {
        dir: dir.to_string_lossy().into_owned(),
        files,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{SourceRef, StudioDoc};

    /// End-to-end: seed doc + real PNG on disk → bundle + export set.
    #[test]
    fn seed_doc_bundles_and_exports() {
        let base = std::env::temp_dir().join(format!("wf_studio_prev_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (gid, key) = ("g", "symbol_h1");

        // A tiny real source + its degenerate "all" cut.
        let img = image::RgbaImage::from_pixel(64, 48, image::Rgba([10, 200, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        store::write_file(&store::source_path(&base, gid, key), &png).unwrap();
        store::write_file(&store::part_dir(&base, gid, key, "all").join("cut.png"), &png).unwrap();

        let doc = StudioDoc::seed(
            SourceRef { variation_id: "v001".into(), width: 64, height: 48, sha256: "x".into() },
            0.0,
        );

        let b = bundle(&base, gid, key, &doc).unwrap();
        assert!(b.skeleton_json.contains("\"spine\": \"4.2"));
        assert!(b.atlas_text.starts_with("symbol_h1.webp"));
        assert_eq!(b.pages.len(), 1);
        assert!(b.pages[0].data_url.starts_with("data:image/webp;base64,"));

        let report = export(&base, gid, key, &doc).unwrap();
        assert!(report.files.contains(&"skeleton.json".to_string()));
        assert!(report.files.contains(&"symbol_h1.atlas".to_string()));
        assert!(report.files.contains(&"symbol_h1.webp".to_string()));
        assert!(store::export_dir(&base, gid, key).join("skeleton.json").is_file());
        // Idle clip exists, so no idle warning.
        assert!(report.warnings.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    /// Dev harness, not a test of behaviour: writes a multi-part sample export to
    /// `$WF_SAMPLE_EXPORT_DIR` so the emitted skeleton/atlas can be validated against the
    /// real spine-ts runtime in Node (`spine-core` parses exactly what spine-pixi-v8 loads).
    /// No-op unless the env var is set.
    #[test]
    fn write_sample_export_for_runtime_validation() {
        let Some(dir) = std::env::var_os("WF_SAMPLE_EXPORT_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(dir);
        let base = out.join("proj");
        let (gid, key) = ("g", "sample");

        let write_png = |path: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]| {
            let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            store::write_file(path, &buf.into_inner()).unwrap();
        };

        // Two parts: torso (300×400 at 100,80) + head (200×160 at 150,0), 500×480 source.
        write_png(&store::source_path(&base, gid, key), 500, 480, [80, 80, 90, 255]);
        write_png(&store::part_dir(&base, gid, key, "torso").join("cut.png"), 300, 400, [120, 40, 40, 255]);
        write_png(&store::part_dir(&base, gid, key, "head").join("cut.png"), 200, 160, [40, 120, 60, 255]);
        // Alternate ("closed") head art, same box → a blink/swap attachment.
        write_png(&store::part_dir(&base, gid, key, "head_closed").join("cut.png"), 200, 160, [60, 40, 120, 255]);

        use crate::studio::doc::*;
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v001".into(), width: 500, height: 480, sha256: "s".into() },
            0.0,
        );
        doc.parts = vec![
            Part {
                id: "torso".into(),
                name: "Torso".into(),
                prompts: vec![],
                bbox: Some(Rect { x: 100, y: 80, w: 300, h: 400 }),
                mask_hash: None,
                completed_hash: None,
                completed_bbox: None,
                texture: PartTexture::Cut,
                deformable: false,
                attachment_only: false,
            },
            Part {
                id: "head".into(),
                name: "Head".into(),
                prompts: vec![],
                bbox: Some(Rect { x: 150, y: 0, w: 200, h: 160 }),
                mask_hash: None,
                completed_hash: None,
                completed_bbox: None,
                texture: PartTexture::Cut,
                deformable: false,
                attachment_only: false,
            },
            // Attachment-only alternate for the head (blink/swap) — no slot/bone of its own.
            Part {
                id: "head_closed".into(),
                name: "Head Closed".into(),
                prompts: vec![],
                bbox: Some(Rect { x: 150, y: 0, w: 200, h: 160 }),
                mask_hash: None,
                completed_hash: None,
                completed_bbox: None,
                texture: PartTexture::Cut,
                deformable: false,
                attachment_only: true,
            },
        ];
        doc.bones = vec![
            Bone::new("root", None, 250.0, 240.0),
            Bone::new("torso", Some("root".into()), 250.0, 300.0),
            {
                let mut b = Bone::new("head", Some("torso".into()), 250.0, 90.0);
                b.rotation = 5.0;
                b.length = 80.0;
                b
            },
        ];
        doc.slots = vec![
            Slot { name: "torso".into(), bone: "torso".into(), part_id: "torso".into(), attachment: Attachment::Region, blend: crate::studio::doc::BlendMode::Normal, alternates: Vec::new() },
            Slot { name: "head".into(), bone: "head".into(), part_id: "head".into(), attachment: Attachment::Region, blend: crate::studio::doc::BlendMode::Additive, alternates: vec!["head_closed".into()] },
        ];
        // The head gets a physics sway (exercises the 4.2 constraint emitter).
        doc.physics = vec![crate::studio::doc::PhysicsSpec::sway("head")];

        // Torso becomes an auto-weighted deformable mesh (exercises the weighted emitter).
        let torso_tex = image::open(store::part_dir(&base, gid, key, "torso").join("cut.png"))
            .unwrap()
            .to_rgba8();
        let torso_bbox = doc.parts[0].bbox.unwrap();
        let mut torso_mesh = crate::studio::mesh::generate(&torso_tex, torso_bbox).unwrap();
        torso_mesh.weights = crate::studio::mesh::auto_weights(&doc, "torso", &torso_mesh);
        doc.slots[0].attachment = Attachment::Mesh(torso_mesh);

        // Head becomes an UNWEIGHTED mesh (deform offsets are authoritative — the case the
        // turn baker and ripple primitive target; weighted-mesh deform has different length
        // semantics in the runtime, so the baker restricts itself to unweighted meshes).
        let head_tex = image::open(store::part_dir(&base, gid, key, "head").join("cut.png"))
            .unwrap()
            .to_rgba8();
        let head_bbox = doc.parts[1].bbox.unwrap();
        let head_mesh = crate::studio::mesh::generate(&head_tex, head_bbox).unwrap();
        let head_vc = head_mesh.vertices.len() / 2;
        doc.slots[1].attachment = Attachment::Mesh(head_mesh);

        doc.clips = vec![StudioDoc::breathe_idle("torso"), {
            let mut c = StudioDoc::breathe_idle("torso");
            c.id = "win".into();
            c.name = "win".into();
            c.timelines.push(Timeline {
                target: TimelineTarget::BoneRotate("head".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0], curve: Curve::Bezier(EASE_IN_OUT.to_vec()) },
                    Key { time: 1.0, v: vec![-12.0], curve: Curve::Stepped },
                    Key { time: 2.0, v: vec![0.0], curve: Curve::Linear },
                ],
            });
            c.timelines.push(Timeline {
                target: TimelineTarget::SlotAlpha("head".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![1.0], curve: Curve::Linear },
                    Key { time: 2.0, v: vec![0.4], curve: Curve::Linear },
                ],
            });
            // Mesh-deform on the head mesh: an all-vertex bob (exercises the deform emitter +
            // runtime parse). Offsets are source-px y-down deltas; here every vertex lifts 10px.
            c.timelines.push(Timeline {
                target: TimelineTarget::SlotDeform("head".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0; head_vc * 2], curve: Curve::Linear },
                    Key {
                        time: 1.0,
                        v: (0..head_vc).flat_map(|_| [0.0, -10.0]).collect(),
                        curve: Curve::Linear,
                    },
                    Key { time: 2.0, v: vec![0.0; head_vc * 2], curve: Curve::Linear },
                ],
            });
            // Attachment swap on the head: base → head_closed → base (a blink).
            c.timelines.push(Timeline {
                target: TimelineTarget::SlotAttachment("head".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0], curve: Curve::Stepped },
                    Key { time: 0.5, v: vec![1.0], curve: Curve::Stepped },
                    Key { time: 0.7, v: vec![0.0], curve: Curve::Stepped },
                ],
            });
            c
        }];

        let report = export(&base, gid, key, &doc).unwrap();
        // Copy the export set flat into $WF_SAMPLE_EXPORT_DIR for the node validator.
        for f in &report.files {
            fs::copy(store::export_dir(&base, gid, key).join(f), out.join(f)).unwrap();
        }
        let _ = fs::remove_dir_all(&base);
    }

    /// Env-gated GEOMETRY REVIEW of generated motion on a realistic cutout mascot. Builds a
    /// full-body character (rigid region parts on an articulated bone tree — the default rig,
    /// no meshes), renders a GENERATED idle and win through `render_plan` (real amp clamps +
    /// safety nets), and exports atlas+skeleton so `scripts/review-motion.mjs` can compute the
    /// WORLD vertices of every part through the real spine runtime and measure body distortion.
    /// The classic mascot "deformity" is a child part SHEARING because a `pop` puts non-uniform
    /// scale on a parent bone; this makes it measurable. Run:
    ///   WF_MASCOT_DUMP=/…/out cargo test --lib studio::preview::tests::mascot_motion_review -- --nocapture
    ///   node scripts/review-motion.mjs /…/out
    #[test]
    fn mascot_motion_review() {
        let Some(dir) = std::env::var_os("WF_MASCOT_DUMP") else {
            return;
        };
        let out = std::path::PathBuf::from(dir);
        let base = out.join("proj");
        let (gid, key) = ("g", "mascot");

        let write_png = |path: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]| {
            let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            store::write_file(path, &buf.into_inner()).unwrap();
        };

        use crate::studio::doc::*;
        // A plausible full-body character. bbox in source px (y-down); children of the torso
        // are held at non-90° angles (arms) so a non-uniform torso scale SHEARS them visibly.
        let parts: [(&str, Rect, [u8; 4]); 6] = [
            ("torso", Rect { x: 255, y: 330, w: 90, h: 200 }, [120, 60, 60, 255]),
            ("head", Rect { x: 250, y: 205, w: 100, h: 120 }, [60, 120, 80, 255]),
            ("arm_l", Rect { x: 185, y: 340, w: 70, h: 170 }, [80, 80, 140, 255]),
            ("arm_r", Rect { x: 345, y: 340, w: 70, h: 170 }, [80, 80, 140, 255]),
            ("leg_l", Rect { x: 265, y: 520, w: 35, h: 200 }, [100, 90, 60, 255]),
            ("leg_r", Rect { x: 300, y: 520, w: 35, h: 200 }, [100, 90, 60, 255]),
        ];
        write_png(&store::source_path(&base, gid, key), 600, 760, [40, 40, 48, 255]);
        for (id, bb, col) in parts {
            write_png(&store::part_dir(&base, gid, key, id).join("cut.png"), bb.w, bb.h, col);
        }

        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 600, height: 760, sha256: "s".into() },
            0.0,
        );
        doc.parts = parts
            .iter()
            .map(|(id, bb, _)| Part {
                id: (*id).into(),
                name: (*id).into(),
                prompts: vec![],
                bbox: Some(*bb),
                mask_hash: None,
                completed_hash: None,
                completed_bbox: None,
                texture: PartTexture::Cut,
                deformable: false,
                attachment_only: false,
            })
            .collect();
        let mk = |name: &str, parent: Option<&str>, x: f64, y: f64, rot: f64, len: f64| {
            let mut b = Bone::new(name, parent.map(|s| s.to_string()), x, y);
            b.rotation = rot;
            b.length = len;
            b
        };
        doc.bones = vec![
            mk("root", None, 300.0, 600.0, 0.0, 0.0),
            mk("torso", Some("root"), 300.0, 520.0, 90.0, 190.0),
            mk("head", Some("torso"), 300.0, 330.0, 90.0, 120.0),
            mk("arm_l", Some("torso"), 255.0, 350.0, 250.0, 170.0),
            mk("arm_r", Some("torso"), 345.0, 350.0, 290.0, 170.0),
            mk("leg_l", Some("root"), 285.0, 525.0, 265.0, 195.0),
            mk("leg_r", Some("root"), 315.0, 525.0, 275.0, 195.0),
        ];
        doc.slots = parts
            .iter()
            .map(|(id, _, _)| Slot {
                name: (*id).into(),
                bone: (*id).into(),
                part_id: (*id).into(),
                attachment: Attachment::Region,
                blend: BlendMode::Normal,
                alternates: vec![],
            })
            .collect();

        use crate::studio::motion_gen::{render_plan, MoveDraft, PlanDraft};
        let osc = |target: &str, kind: &str, amp: f64| MoveDraft {
            target: target.into(),
            kind: kind.into(),
            amp: Some(amp),
            cycles: Some(2.0),
            phase: None,
            color: None,
            to: None,
            at: None,
        };
        let beat = |target: &str, kind: &str, amp: f64, at: f64| MoveDraft {
            target: target.into(),
            kind: kind.into(),
            amp: Some(amp),
            cycles: None,
            phase: None,
            color: None,
            to: None,
            at: Some(at),
        };
        let idle = render_plan(
            &doc,
            "idle",
            "idle",
            PlanDraft {
                duration: 3.0,
                looping: Some(true),
                moves: vec![
                    osc("torso", "shift", 8.0),
                    osc("torso", "breathe", 0.03),
                    osc("head", "sway", 6.0),
                    osc("arm_l", "swing", 16.0),
                    osc("arm_r", "swing", 16.0),
                    osc("leg_l", "swing", 8.0),
                    osc("leg_r", "swing", 8.0),
                ],
            },
        );
        let win = render_plan(
            &doc,
            "win",
            "win",
            PlanDraft {
                duration: 1.6,
                looping: Some(false),
                moves: vec![
                    beat("torso", "pop", 0.18, 0.35), // non-uniform scale on a parent → shears children
                    beat("arm_r", "anticipate", 16.0, 0.35),
                    osc("torso", "glint", 0.0), // colour only, harmless
                ],
            },
        );
        doc.clips = vec![idle, win];

        let report = export(&base, gid, key, &doc).unwrap();
        for f in &report.files {
            fs::copy(store::export_dir(&base, gid, key).join(f), out.join(f)).unwrap();
        }
        eprintln!(
            "wrote mascot export to {} — now run: node scripts/review-motion.mjs {}",
            out.display(),
            out.display()
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// Dump the REAL preview bundle for an on-disk asset — the exact artifact every studio
    /// preview renders. Confirms whether the skeleton/atlas/textures are valid (→ frontend
    /// bug) or the bundle build fails/empties (→ backend bug). Env-gated. Run:
    ///   WF_BASE=/…/projects WF_GAME=game WF_ASSET=key \
    ///     cargo test --lib studio::preview::tests::bundle_real_asset_dump -- --nocapture
    #[test]
    fn bundle_real_asset_dump() {
        let (Some(base), Ok(game), Ok(asset)) =
            (std::env::var_os("WF_BASE"), std::env::var("WF_GAME"), std::env::var("WF_ASSET"))
        else {
            return;
        };
        let base = std::path::PathBuf::from(base);
        let doc = store::read_doc(&base, &game, &asset).unwrap().unwrap();
        eprintln!(
            "doc: {} bones, {} slots, {} parts ({} cut), {} clips",
            doc.bones.len(),
            doc.slots.len(),
            doc.parts.len(),
            doc.parts.iter().filter(|p| p.bbox.is_some()).count(),
            doc.clips.len()
        );
        for s in &doc.slots {
            let att = match &s.attachment {
                crate::studio::doc::Attachment::Region => "region",
                crate::studio::doc::Attachment::Mesh(_) => "mesh",
            };
            let cut = store::part_texture_path(&base, &game, &asset, doc.part(&s.part_id).unwrap());
            eprintln!("  slot {:<16} bone={:<16} att={att:<6} tex_exists={}", s.name, s.bone, cut.is_file());
        }
        match super::bundle(&base, &game, &asset, &doc) {
            Ok(b) => {
                eprintln!(
                    "BUNDLE OK: skeleton {} B · atlas {} B · {} page(s)",
                    b.skeleton_json.len(),
                    b.atlas_text.len(),
                    b.pages.len()
                );
                for p in &b.pages {
                    eprintln!("  page {} dataUrl {} B", p.name, p.data_url.len());
                }
                let v: serde_json::Value = serde_json::from_str(&b.skeleton_json).unwrap();
                let slots = v["slots"].as_array().map(|a| a.len()).unwrap_or(0);
                let att_count = v["skins"]
                    .as_array()
                    .and_then(|s| s.first())
                    .and_then(|s| s["attachments"].as_object())
                    .map(|o| o.len())
                    .unwrap_or(0);
                eprintln!("  skeleton: {slots} slots, {att_count} skin-attachment groups");
                if slots == 0 || b.pages.is_empty() {
                    eprintln!("  ⚠ EMPTY — nothing to draw (this is why the preview is blank)");
                }
            }
            Err(e) => eprintln!("BUNDLE FAILED: {e}  ← this is why the preview is blank"),
        }
    }
}
