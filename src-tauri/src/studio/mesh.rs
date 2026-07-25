//! Deformable meshes: a grid mesh over a part's opaque pixels + distance-based
//! auto-weights. With one bone per part, the win is smooth JOINT bending — vertices near
//! the parent joint blend toward the parent bone (up to 50/50), so rotating a limb bows
//! the seam instead of cracking it. Region stays the default; meshes are opt-in per part.

use image::RgbaImage;

use super::doc::{BoneInfluence, MeshData, Rect, StudioDoc, VertexWeights};

/// Grid cell target: ~8 cells across the long side, cells clamped to 12..64 px.
fn grid_step(w: u32, h: u32) -> u32 {
    (w.max(h) / 8).clamp(12, 64)
}

/// Build a grid mesh over the texture's opaque region (dilated by one cell so the mesh
/// covers antialiased edges). Vertices are SOURCE-pixel positions (bbox offset applied);
/// UVs are 0..1 within the texture. Returns `None` for a fully transparent texture.
pub fn generate(tex: &RgbaImage, bbox: Rect) -> Option<MeshData> {
    let (tw, th) = tex.dimensions();
    if tw == 0 || th == 0 {
        return None;
    }
    let step = grid_step(tw, th);
    let cols = tw.div_ceil(step) as usize;
    let rows = th.div_ceil(step) as usize;

    // Cell occupancy (sampled every 2px for speed).
    let mut occupied = vec![false; cols * rows];
    for (x, y, p) in tex.enumerate_pixels() {
        if (x | y) & 1 == 0 && p.0[3] > 8 {
            occupied[(y / step) as usize * cols + (x / step) as usize] = true;
        }
    }
    if !occupied.iter().any(|&o| o) {
        return None;
    }
    // Dilate by one cell.
    let src = occupied.clone();
    for cy in 0..rows {
        for cx in 0..cols {
            if src[cy * cols + cx] {
                continue;
            }
            let near = (cy.saturating_sub(1)..=(cy + 1).min(rows - 1)).any(|ny| {
                (cx.saturating_sub(1)..=(cx + 1).min(cols - 1)).any(|nx| src[ny * cols + nx])
            });
            if near {
                occupied[cy * cols + cx] = true;
            }
        }
    }

    // Vertex grid: a corner is used if any adjacent cell is occupied.
    let vcols = cols + 1;
    let vrows = rows + 1;
    let mut vidx = vec![u32::MAX; vcols * vrows];
    let mut vertices: Vec<f64> = Vec::new();
    let mut uvs: Vec<f64> = Vec::new();
    let cell_at = |cx: isize, cy: isize| -> bool {
        cx >= 0
            && cy >= 0
            && (cx as usize) < cols
            && (cy as usize) < rows
            && occupied[cy as usize * cols + cx as usize]
    };
    for vy in 0..vrows {
        for vx in 0..vcols {
            let used = cell_at(vx as isize - 1, vy as isize - 1)
                || cell_at(vx as isize, vy as isize - 1)
                || cell_at(vx as isize - 1, vy as isize)
                || cell_at(vx as isize, vy as isize);
            if !used {
                continue;
            }
            // Clamp the last row/column onto the texture edge.
            let px = ((vx as u32) * step).min(tw) as f64;
            let py = ((vy as u32) * step).min(th) as f64;
            vidx[vy * vcols + vx] = (vertices.len() / 2) as u32;
            vertices.push(bbox.x as f64 + px);
            vertices.push(bbox.y as f64 + py);
            uvs.push(px / tw as f64);
            uvs.push(py / th as f64);
        }
    }

    // Two triangles per occupied cell.
    let mut triangles: Vec<u32> = Vec::new();
    for cy in 0..rows {
        for cx in 0..cols {
            if !occupied[cy * cols + cx] {
                continue;
            }
            let v00 = vidx[cy * vcols + cx];
            let v10 = vidx[cy * vcols + cx + 1];
            let v01 = vidx[(cy + 1) * vcols + cx];
            let v11 = vidx[(cy + 1) * vcols + cx + 1];
            debug_assert!(v00 != u32::MAX && v10 != u32::MAX && v01 != u32::MAX && v11 != u32::MAX);
            triangles.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
        }
    }

    Some(MeshData { vertices, uvs, triangles, hull: 0, weights: None })
}

/// Distance-based auto-weights.
///
/// Influence candidates = the part's own bone plus every descendant bone that isn't
/// another part's slot bone (i.e. a CHAIN authored down the part — tail, cape, crest).
/// Each vertex binds to its 2 nearest candidates by distance to the bone SEGMENT
/// (pivot→tip), inverse-distance blended — so the mesh bends ALONG the chain. On top,
/// vertices near the part's own pivot (the joint — auto-rig puts it on the parent seam)
/// blend up to 50% toward the PARENT bone so the seam bows instead of cracking.
pub fn auto_weights(doc: &StudioDoc, own_bone: &str, mesh: &MeshData) -> Option<Vec<VertexWeights>> {
    let own = doc.bone(own_bone)?;
    let parent = own.parent.as_deref().and_then(|p| doc.bone(p));
    let n = mesh.vertices.len() / 2;

    // Chain candidates: own + descendants not bound by other slots.
    let other_slot_bones: Vec<&str> = doc
        .slots
        .iter()
        .filter(|s| s.bone != own_bone)
        .map(|s| s.bone.as_str())
        .collect();
    let mut candidates: Vec<&crate::studio::doc::Bone> = vec![own];
    let mut grew = true;
    while grew {
        grew = false;
        for b in &doc.bones {
            if candidates.iter().any(|c| c.name == b.name) {
                continue;
            }
            let under = b
                .parent
                .as_deref()
                .is_some_and(|p| candidates.iter().any(|c| c.name == p));
            if under && !other_slot_bones.contains(&b.name.as_str()) {
                candidates.push(b);
                grew = true;
            }
        }
    }

    let reach = parent
        .map(|p| ((own.x - p.x).powi(2) + (own.y - p.y).powi(2)).sqrt().max(1.0))
        .unwrap_or(1.0);

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (vx, vy) = (mesh.vertices[i * 2], mesh.vertices[i * 2 + 1]);

        // Nearest-2 candidates by segment distance, inverse-distance blended.
        let mut ranked: Vec<(f64, &str)> = candidates
            .iter()
            .map(|b| (segment_distance(vx, vy, b), b.name.as_str()))
            .collect();
        ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let take = &ranked[..ranked.len().min(2)];
        let inv_sum: f64 = take.iter().map(|(d, _)| 1.0 / (d + 1.0)).sum();
        let mut influences: Vec<BoneInfluence> = take
            .iter()
            .map(|(d, name)| BoneInfluence {
                bone: (*name).to_string(),
                weight: (1.0 / (d + 1.0)) / inv_sum,
            })
            .filter(|inf| inf.weight > 0.02)
            .collect();

        // Joint blend toward the parent near the own pivot.
        if let Some(par) = parent {
            let d_joint = ((vx - own.x).powi(2) + (vy - own.y).powi(2)).sqrt();
            let w_par = (0.5 * (1.0 - d_joint / reach)).clamp(0.0, 0.5);
            if w_par >= 0.02 {
                for inf in &mut influences {
                    inf.weight *= 1.0 - w_par;
                }
                influences.push(BoneInfluence { bone: par.name.clone(), weight: w_par });
            }
        }
        // Normalize (filtering may have dropped a sliver).
        let total: f64 = influences.iter().map(|i| i.weight).sum();
        for inf in &mut influences {
            inf.weight /= total.max(1e-9);
        }
        out.push(VertexWeights { influences });
    }
    Some(out)
}

/// Distance from a point to a bone's segment (pivot → tip; pivot only when length = 0).
/// Doc rotations are Spine-CCW over y-down coords, so the tip is at (+cos, -sin).
fn segment_distance(px: f64, py: f64, b: &crate::studio::doc::Bone) -> f64 {
    let (ax, ay) = (b.x, b.y);
    if b.length <= 0.0 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let r = b.rotation.to_radians();
    let (bx, by) = (ax + r.cos() * b.length, ay - r.sin() * b.length);
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::{Bone, SourceRef};

    fn tex(w: u32, h: u32, opaque: impl Fn(u32, u32) -> bool) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([100, 100, 100, if opaque(x, y) { 255 } else { 0 }])
        })
    }

    #[test]
    fn grid_mesh_covers_opaque_region_with_valid_topology() {
        let t = tex(100, 60, |x, _| x < 70); // left 70px opaque
        let bbox = Rect { x: 200, y: 100, w: 100, h: 60 };
        let m = generate(&t, bbox).unwrap();
        let n = m.vertices.len() / 2;
        assert_eq!(m.uvs.len(), m.vertices.len());
        assert!(n >= 4);
        // All triangle indices valid.
        assert!(m.triangles.iter().all(|&i| (i as usize) < n));
        assert_eq!(m.triangles.len() % 3, 0);
        // Vertices offset into source coords and UVs in range.
        for i in 0..n {
            let (x, y) = (m.vertices[i * 2], m.vertices[i * 2 + 1]);
            assert!((200.0..=300.0).contains(&x) && (100.0..=160.0).contains(&y));
            assert!((0.0..=1.0).contains(&m.uvs[i * 2]) && (0.0..=1.0).contains(&m.uvs[i * 2 + 1]));
        }
        // Fully transparent → None.
        assert!(generate(&tex(50, 50, |_, _| false), bbox).is_none());
    }

    #[test]
    fn auto_weights_blend_toward_parent_near_joint() {
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 400, height: 300, sha256: "s".into() },
            0.0,
        );
        // torso at (100,150) ← arm bone pivots at (200,150); arm extends right.
        doc.bones = vec![
            Bone::new("root", None, 200.0, 150.0),
            Bone::new("torso", Some("root".into()), 100.0, 150.0),
            Bone::new("arm", Some("torso".into()), 200.0, 150.0),
        ];
        let mesh = MeshData {
            vertices: vec![
                205.0, 150.0, // at the shoulder joint → strong parent blend
                380.0, 150.0, // far down the arm → own bone only
            ],
            uvs: vec![0.0, 0.5, 1.0, 0.5],
            triangles: vec![0, 1, 0],
            hull: 0,
            weights: None,
        };
        let w = auto_weights(&doc, "arm", &mesh).unwrap();
        assert_eq!(w.len(), 2);
        // Joint vertex: two influences, parent close to 0.5, sums to 1.
        assert_eq!(w[0].influences.len(), 2);
        let par_w = w[0].influences.iter().find(|i| i.bone == "torso").unwrap().weight;
        assert!(par_w > 0.4, "parent weight {par_w}");
        let sum: f64 = w[0].influences.iter().map(|i| i.weight).sum();
        assert!((sum - 1.0).abs() < 1e-9);
        // Far vertex: own bone only.
        assert_eq!(w[1].influences.len(), 1);
        assert_eq!(w[1].influences[0].bone, "arm");
    }

    #[test]
    fn chain_bones_dominate_vertices_along_the_chain() {
        let mut doc = StudioDoc::seed(
            SourceRef { variation_id: "v".into(), width: 400, height: 100, sha256: "s".into() },
            0.0,
        );
        // A horizontal tail: part bone (100,50)→tip(200,50), chain child (200,50)→tip(300,50).
        let mut tail = Bone::new("tail", Some("root".into()), 100.0, 50.0);
        tail.length = 100.0;
        let mut seg2 = Bone::new("tail_seg2", Some("tail".into()), 200.0, 50.0);
        seg2.length = 100.0;
        doc.bones = vec![Bone::new("root", None, 200.0, 50.0), tail, seg2];
        doc.slots[0].bone = "tail".into();

        let mesh = MeshData {
            vertices: vec![
                120.0, 50.0, // on the first segment
                280.0, 50.0, // deep into the chain child
            ],
            uvs: vec![0.0, 0.5, 1.0, 0.5],
            triangles: vec![0, 1, 0],
            hull: 0,
            weights: None,
        };
        let w = auto_weights(&doc, "tail", &mesh).unwrap();
        let weight_of = |v: &VertexWeights, bone: &str| {
            v.influences.iter().find(|i| i.bone == bone).map(|i| i.weight).unwrap_or(0.0)
        };
        assert!(
            weight_of(&w[0], "tail") > weight_of(&w[0], "tail_seg2"),
            "base vertex follows the part bone: {:?}",
            w[0]
        );
        assert!(
            weight_of(&w[1], "tail_seg2") > 0.8,
            "tip vertex follows the chain bone: {:?}",
            w[1]
        );
        // Weights normalized.
        for v in &w {
            let sum: f64 = v.influences.iter().map(|i| i.weight).sum();
            assert!((sum - 1.0).abs() < 1e-6);
        }
    }
}
