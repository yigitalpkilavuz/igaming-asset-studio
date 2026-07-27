//! Spine 4.2 skeleton JSON emitter — a pure function of the [`StudioDoc`].
//!
//! This is the single place where studio coordinates (source pixels, y-down, world-space
//! pivots) become Spine coordinates (y-up, origin at image centre, bone-local transforms).
//! 1 Spine unit = 1 source pixel.
//!
//! The 4.x curve trap: per-key `"curve"` is either `"stepped"` or a flat array of
//! ABSOLUTE control points — 4 numbers per value component `[cx1, cy1, cx2, cy2]` where
//! `cx` are seconds between this key and the next and `cy` are values. NOT the 3.x
//! normalized percentages. Doc curves store normalized unit-square handles; this module
//! denormalizes them.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use super::doc::{Attachment, Curve, Key, StudioDoc, TimelineTarget};

/// Version stamped into `skeleton.spine`. Must be 4.2.x for spine-pixi-v8 4.2.
pub const SPINE_VERSION: &str = "4.2.43";

/// World transform of a bone as resolved during emission.
#[derive(Clone, Copy)]
struct World {
    x: f64, // spine coords (y-up, centre origin)
    y: f64,
    rot: f64,
    sx: f64, // accumulated scale product down the chain
    sy: f64,
}

pub fn emit(doc: &StudioDoc) -> Result<Value, String> {
    let (w, h) = (doc.source.width as f64, doc.source.height as f64);
    let to_spine = |px: f64, py: f64| (px - w / 2.0, h / 2.0 - py);

    // ── Bones, parents before children ─────────────────────────────────────────
    let ordered = bone_emit_order(doc)?;
    let mut worlds: HashMap<String, World> = HashMap::new();
    let mut bone_index: HashMap<String, usize> = HashMap::new();
    let mut bones_json: Vec<Value> = Vec::new();

    for b in &ordered {
        let (wx, wy) = to_spine(b.x as f64, b.y as f64);
        let parent_world = b.parent.as_deref().map(|p| {
            worlds
                .get(p)
                .copied()
                .ok_or_else(|| format!("bone \"{}\" parent \"{p}\" not found", b.name))
        });
        let mut obj = Map::new();
        obj.insert("name".into(), json!(b.name));
        match parent_world {
            None => {
                // root: world == local
                worlds.insert(
                    b.name.clone(),
                    World { x: wx, y: wy, rot: b.rotation as f64, sx: b.scale_x as f64, sy: b.scale_y as f64 },
                );
                if wx.abs() > 1e-6 {
                    obj.insert("x".into(), json!(round(wx)));
                }
                if wy.abs() > 1e-6 {
                    obj.insert("y".into(), json!(round(wy)));
                }
            }
            Some(pw) => {
                let pw = pw?;
                obj.insert("parent".into(), json!(b.parent));
                // Local translation: world delta rotated into the parent frame, divided by
                // the parent's accumulated scale. Exact for the setup norm (scales == 1);
                // non-uniform parent scale + rotation would need a full affine inverse.
                let (dx, dy) = (wx - pw.x, wy - pw.y);
                let (lx, ly) = rotate(dx, dy, -pw.rot);
                let (lx, ly) = (lx / pw.sx.max(1e-9), ly / pw.sy.max(1e-9));
                let lrot = norm_angle(b.rotation as f64 - pw.rot);
                if lx.abs() > 1e-6 {
                    obj.insert("x".into(), json!(round(lx)));
                }
                if ly.abs() > 1e-6 {
                    obj.insert("y".into(), json!(round(ly)));
                }
                if lrot.abs() > 1e-6 {
                    obj.insert("rotation".into(), json!(round(lrot)));
                }
                worlds.insert(
                    b.name.clone(),
                    World {
                        x: wx,
                        y: wy,
                        rot: b.rotation as f64,
                        sx: pw.sx * b.scale_x as f64,
                        sy: pw.sy * b.scale_y as f64,
                    },
                );
            }
        }
        if b.length > 0.0 {
            obj.insert("length".into(), json!(round(b.length as f64)));
        }
        if (b.scale_x - 1.0).abs() > 1e-6 {
            obj.insert("scaleX".into(), json!(round4(b.scale_x as f64)));
        }
        if (b.scale_y - 1.0).abs() > 1e-6 {
            obj.insert("scaleY".into(), json!(round4(b.scale_y as f64)));
        }
        bone_index.insert(b.name.clone(), bones_json.len());
        bones_json.push(Value::Object(obj));
    }

    // ── Slots + attachments ────────────────────────────────────────────────────
    let mut slots_json: Vec<Value> = Vec::new();
    let mut attachments = Map::new();
    for s in &doc.slots {
        let bone_world = *worlds
            .get(&s.bone)
            .ok_or_else(|| format!("slot \"{}\" references unknown bone \"{}\"", s.name, s.bone))?;
        let part = doc
            .part(&s.part_id)
            .ok_or_else(|| format!("slot \"{}\" references unknown part \"{}\"", s.name, s.part_id))?;
        let bbox = part
            .effective_bbox()
            .ok_or_else(|| format!("part \"{}\" has no cut texture yet", part.id))?;

        let mut slot_obj = Map::new();
        slot_obj.insert("name".into(), json!(s.name));
        slot_obj.insert("bone".into(), json!(s.bone));
        slot_obj.insert("attachment".into(), json!(part.id));
        // Blend mode (lowercase — the runtime uppercases the first letter for its enum).
        match s.blend {
            super::doc::BlendMode::Normal => {}
            super::doc::BlendMode::Additive => {
                slot_obj.insert("blend".into(), json!("additive"));
            }
            super::doc::BlendMode::Multiply => {
                slot_obj.insert("blend".into(), json!("multiply"));
            }
            super::doc::BlendMode::Screen => {
                slot_obj.insert("blend".into(), json!("screen"));
            }
        }
        slots_json.push(Value::Object(slot_obj));

        let att_val = match &s.attachment {
            Attachment::Region => region_attachment(bbox, &bone_world, to_spine),
            Attachment::Mesh(m) => mesh_attachment(doc, m, &bone_world, &bone_index, &worlds, to_spine, bbox.w, bbox.h)?,
        };
        let mut by_name = Map::new();
        by_name.insert(part.id.clone(), att_val);
        // Alternate attachments (blink / mouth shapes / swap) — region-only quads on the SAME
        // host bone, so a swapped image overlays exactly where the base sits.
        for alt_id in &s.alternates {
            let alt = doc
                .part(alt_id)
                .ok_or_else(|| format!("slot \"{}\" references unknown alternate part \"{alt_id}\"", s.name))?;
            let alt_bbox = alt
                .effective_bbox()
                .ok_or_else(|| format!("alternate part \"{alt_id}\" has no cut texture yet"))?;
            by_name.insert(alt_id.clone(), region_attachment(alt_bbox, &bone_world, to_spine));
        }
        attachments.insert(s.name.clone(), Value::Object(by_name));
    }

    // ── Physics constraints (4.2: the runtime simulates them) ──────────────────
    // Unknown bones are skipped rather than fatal — the runtime hard-errors on them.
    let physics_json: Vec<Value> = doc
        .physics
        .iter()
        .filter(|p| worlds.contains_key(&p.bone))
        .enumerate()
        .map(|(i, p)| {
            json!({
                "name": p.bone,
                "order": i,
                "bone": p.bone,
                "rotate": round4(p.rotate.clamp(0.0, 1.0)),
                "inertia": round4(p.inertia.clamp(0.0, 1.0)),
                "strength": round4(p.strength.max(0.0)),
                "damping": round4(p.damping.clamp(0.0, 1.0)),
                "wind": round4(p.wind),
                "gravity": round4(p.gravity),
                "mix": round4(p.mix.clamp(0.0, 1.0)),
            })
        })
        .collect();

    // ── Animations ─────────────────────────────────────────────────────────────
    let mut animations = Map::new();
    for clip in &doc.clips {
        animations.insert(clip.name.clone(), emit_clip(doc, clip, &worlds)?);
    }

    Ok(json!({
        "skeleton": {
            "hash": fnv(&format!("{}:{}", doc.source.sha256, doc.parts.len())),
            "spine": SPINE_VERSION,
            "x": round(-w / 2.0),
            "y": round(-h / 2.0),
            "width": round(w),
            "height": round(h),
            "images": "",
            "audio": "",
        },
        "bones": bones_json,
        "slots": slots_json,
        "physics": physics_json,
        "skins": [ { "name": "default", "attachments": Value::Object(attachments) } ],
        "animations": Value::Object(animations),
    }))
}

/// Weighted/unweighted mesh attachment. Doc mesh vertices are source-px (y-down world).
#[allow(clippy::too_many_arguments)]
fn mesh_attachment(
    doc: &StudioDoc,
    m: &super::doc::MeshData,
    bone_world: &World,
    bone_index: &HashMap<String, usize>,
    worlds: &HashMap<String, World>,
    to_spine: impl Fn(f64, f64) -> (f64, f64),
    tex_w: u32,
    tex_h: u32,
) -> Result<Value, String> {
    let n = m.vertices.len() / 2;
    if m.uvs.len() != m.vertices.len() {
        return Err("mesh uvs/vertices length mismatch".into());
    }
    let vertices_json: Vec<Value> = match &m.weights {
        None => {
            // Unweighted: bone-local positions, flat [x, y, ...].
            let mut out = Vec::with_capacity(m.vertices.len());
            for i in 0..n {
                let (wx, wy) = to_spine(m.vertices[i * 2] as f64, m.vertices[i * 2 + 1] as f64);
                let (lx, ly) = rotate(wx - bone_world.x, wy - bone_world.y, -bone_world.rot);
                out.push(json!(round(lx / bone_world.sx.max(1e-9))));
                out.push(json!(round(ly / bone_world.sy.max(1e-9))));
            }
            out
        }
        Some(weights) => {
            if weights.len() != n {
                return Err("mesh weights/vertices length mismatch".into());
            }
            // Weighted: flat [boneCount, boneIndex, bindX, bindY, weight, ...] with bind
            // positions in each influencing bone's local setup space.
            let mut out = Vec::new();
            for (i, vw) in weights.iter().enumerate() {
                let (wx, wy) = to_spine(m.vertices[i * 2] as f64, m.vertices[i * 2 + 1] as f64);
                let influences: Vec<_> = vw
                    .influences
                    .iter()
                    .filter(|inf| inf.weight > 1e-4)
                    .collect();
                if influences.is_empty() {
                    return Err(format!("mesh vertex {i} has no bone influences"));
                }
                out.push(json!(influences.len()));
                let total: f64 = influences.iter().map(|inf| inf.weight as f64).sum();
                for inf in influences {
                    let idx = *bone_index
                        .get(&inf.bone)
                        .ok_or_else(|| format!("mesh weight references unknown bone \"{}\"", inf.bone))?;
                    let bw = worlds.get(&inf.bone).unwrap();
                    let (bx, by) = rotate(wx - bw.x, wy - bw.y, -bw.rot);
                    out.push(json!(idx));
                    out.push(json!(round(bx / bw.sx.max(1e-9))));
                    out.push(json!(round(by / bw.sy.max(1e-9))));
                    out.push(json!(round4(inf.weight as f64 / total)));
                }
            }
            out
        }
    };
    let _ = doc;
    Ok(json!({
        "type": "mesh",
        "uvs": m.uvs.iter().map(|u| json!(round4(*u as f64))).collect::<Vec<_>>(),
        "triangles": m.triangles,
        "vertices": vertices_json,
        "hull": m.hull,
        "width": tex_w,
        "height": tex_h,
    }))
}

/// A rigid region attachment: the part's texture quad offset from its bone pivot. Shared by the
/// base attachment and by each alternate (blink/swap), which sit on the same host bone.
fn region_attachment(
    bbox: super::doc::Rect,
    bone_world: &World,
    to_spine: impl Fn(f64, f64) -> (f64, f64),
) -> Value {
    let (tcx, tcy) = to_spine(
        bbox.x as f64 + bbox.w as f64 / 2.0,
        bbox.y as f64 + bbox.h as f64 / 2.0,
    );
    let (dx, dy) = (tcx - bone_world.x, tcy - bone_world.y);
    let (ax, ay) = rotate(dx, dy, -bone_world.rot);
    let (ax, ay) = (ax / bone_world.sx.max(1e-9), ay / bone_world.sy.max(1e-9));
    let mut att = Map::new();
    if ax.abs() > 1e-6 {
        att.insert("x".into(), json!(round(ax)));
    }
    if ay.abs() > 1e-6 {
        att.insert("y".into(), json!(round(ay)));
    }
    let arot = norm_angle(-bone_world.rot);
    if arot.abs() > 1e-6 {
        att.insert("rotation".into(), json!(round(arot)));
    }
    att.insert("width".into(), json!(bbox.w));
    att.insert("height".into(), json!(bbox.h));
    Value::Object(att)
}

/// One clip → the Spine animation object `{ "bones":…, "slots":…, "deform":… }`.
fn emit_clip(
    doc: &StudioDoc,
    clip: &super::doc::Clip,
    worlds: &HashMap<String, World>,
) -> Result<Value, String> {
    let mut bones: Map<String, Value> = Map::new();
    let mut slots: Map<String, Value> = Map::new();
    // deform.<skin>.<slot>.<attachment> = [keys]; the studio uses a single "default" skin.
    let mut deform_by_slot: Map<String, Value> = Map::new();

    for tl in &clip.timelines {
        if tl.keys.is_empty() {
            continue;
        }
        // Non-scalar channels (variable-length vertex arrays / string-valued swaps) take
        // dedicated emit paths — they don't go through the fixed-arity `emit_keys`.
        match &tl.target {
            TimelineTarget::SlotDeform(s) => {
                let slot = slot_named(doc, clip, s, "deform")?;
                let keys_json = emit_deform_keys(clip, slot, &tl.keys, worlds)?;
                // Spine 4.2 nests attachment timelines by type:
                // deform.<skin>.<slot>.<attachment>.deform = [keys].
                let mut typed = Map::new();
                typed.insert("deform".into(), keys_json);
                let mut by_att = Map::new();
                by_att.insert(slot.part_id.clone(), Value::Object(typed));
                deform_by_slot.insert(s.clone(), Value::Object(by_att));
                continue;
            }
            TimelineTarget::SlotAttachment(s) => {
                let slot = slot_named(doc, clip, s, "attachment")?;
                let keys_json = emit_attachment_keys(clip, slot, &tl.keys)?;
                slot_entry(&mut slots, s).insert("attachment".into(), keys_json);
                continue;
            }
            _ => {}
        }
        let comps = tl
            .target
            .components()
            .expect("scalar timeline has fixed arity");
        for k in &tl.keys {
            if k.v.len() != comps {
                return Err(format!(
                    "clip \"{}\": key at t={} has {} values, expected {comps}",
                    clip.name,
                    k.time,
                    k.v.len()
                ));
            }
        }
        let keys_json = emit_keys(&tl.keys, comps, &tl.target)?;
        match &tl.target {
            TimelineTarget::BoneRotate(b) => {
                bone_entry(&mut bones, b).insert("rotate".into(), keys_json);
            }
            TimelineTarget::BoneTranslate(b) => {
                bone_entry(&mut bones, b).insert("translate".into(), keys_json);
            }
            TimelineTarget::BoneScale(b) => {
                bone_entry(&mut bones, b).insert("scale".into(), keys_json);
            }
            TimelineTarget::SlotAlpha(s) => {
                slot_entry(&mut slots, s).insert("alpha".into(), keys_json);
            }
            TimelineTarget::SlotColor(s) => {
                slot_entry(&mut slots, s).insert("rgb".into(), keys_json);
            }
            TimelineTarget::SlotDeform(_) | TimelineTarget::SlotAttachment(_) => {
                unreachable!("deform/attachment handled before scalar emission")
            }
        }
    }

    let mut anim = Map::new();
    if !slots.is_empty() {
        anim.insert("slots".into(), Value::Object(slots));
    }
    if !bones.is_empty() {
        anim.insert("bones".into(), Value::Object(bones));
    }
    if !deform_by_slot.is_empty() {
        // 4.2 reads deform/sequence timelines from the animation's "attachments" key
        // (attachments.<skin>.<slot>.<attachment>.deform) — NOT a top-level "deform".
        let mut skin = Map::new();
        skin.insert("default".into(), Value::Object(deform_by_slot));
        anim.insert("attachments".into(), Value::Object(skin));
    }
    Ok(Value::Object(anim))
}

/// Look up a slot by name with a clip-scoped error.
fn slot_named<'a>(
    doc: &'a StudioDoc,
    clip: &super::doc::Clip,
    name: &str,
    kind: &str,
) -> Result<&'a super::doc::Slot, String> {
    doc.slots
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("clip \"{}\": {kind} timeline on unknown slot \"{name}\"", clip.name))
}

fn slot_entry<'a>(slots: &'a mut Map<String, Value>, name: &str) -> &'a mut Map<String, Value> {
    slots
        .entry(name.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .unwrap()
}

/// Emit a mesh-deform timeline. Each key's `v` holds source-px (y-down) vertex-offset deltas
/// `[dx0, dy0, …]`; they map into the slot bone's local space via the same linear transform as
/// the setup mesh MINUS the translation (offsets are deltas): scale · rot(−bone) · yflip.
fn emit_deform_keys(
    clip: &super::doc::Clip,
    slot: &super::doc::Slot,
    keys: &[Key],
    worlds: &HashMap<String, World>,
) -> Result<Value, String> {
    let mesh = match &slot.attachment {
        Attachment::Mesh(m) => m,
        _ => {
            return Err(format!(
                "clip \"{}\": deform on slot \"{}\" whose attachment is not a mesh",
                clip.name, slot.name
            ))
        }
    };
    let vcount = mesh.vertices.len() / 2;
    let bw = worlds.get(&slot.bone).ok_or_else(|| {
        format!(
            "clip \"{}\": deform slot \"{}\" references unknown bone \"{}\"",
            clip.name, slot.name, slot.bone
        )
    })?;

    let mut out: Vec<Value> = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        if k.v.len() != vcount * 2 {
            return Err(format!(
                "clip \"{}\": deform key at t={} has {} values, expected {} (2 × {vcount} verts)",
                clip.name,
                k.time,
                k.v.len(),
                vcount * 2
            ));
        }
        let mut verts: Vec<Value> = Vec::with_capacity(vcount * 2);
        for vi in 0..vcount {
            // A·d — y-flip, rotate into bone-local, divide by bone scale. No translation.
            let (fx, fy) = (k.v[vi * 2], -k.v[vi * 2 + 1]);
            let (rx, ry) = rotate(fx, fy, -bw.rot);
            verts.push(json!(round(rx / bw.sx.max(1e-9))));
            verts.push(json!(round(ry / bw.sy.max(1e-9))));
        }
        let mut obj = Map::new();
        if k.time.abs() > 1e-9 {
            obj.insert("time".into(), json!(round4(k.time)));
        }
        obj.insert("vertices".into(), Value::Array(verts));
        // A deform key carries a SINGLE interpolation curve (0..1) across all vertices — not the
        // per-component form of scalar timelines. The generator uses Linear/Stepped in practice.
        if i + 1 < keys.len() {
            match &k.curve {
                Curve::Linear => {}
                Curve::Stepped => {
                    obj.insert("curve".into(), json!("stepped"));
                }
                Curve::Bezier(h) if h.len() >= 4 => {
                    let (t0, t1) = (k.time, keys[i + 1].time);
                    obj.insert(
                        "curve".into(),
                        json!([
                            round4(t0 + h[0] * (t1 - t0)),
                            round4(h[1]),
                            round4(t0 + h[2] * (t1 - t0)),
                            round4(h[3]),
                        ]),
                    );
                }
                Curve::Bezier(_) => {}
            }
        }
        out.push(Value::Object(obj));
    }
    Ok(Value::Array(out))
}

/// Emit a slot-attachment (swap) timeline. `v[0]` indexes `[part_id, ...slot.alternates]`;
/// a negative index (`HIDE_INDEX`) emits `null` (the slot shows nothing).
fn emit_attachment_keys(
    clip: &super::doc::Clip,
    slot: &super::doc::Slot,
    keys: &[Key],
) -> Result<Value, String> {
    let names: Vec<&str> = std::iter::once(slot.part_id.as_str())
        .chain(slot.alternates.iter().map(|a| a.as_str()))
        .collect();
    let mut out: Vec<Value> = Vec::with_capacity(keys.len());
    for k in keys {
        let idx = k.v.first().copied().unwrap_or(0.0).round() as i64;
        let mut obj = Map::new();
        if k.time.abs() > 1e-9 {
            obj.insert("time".into(), json!(round4(k.time)));
        }
        if idx < 0 {
            obj.insert("name".into(), Value::Null);
        } else {
            let name = names.get(idx as usize).copied().ok_or_else(|| {
                format!(
                    "clip \"{}\": attachment key index {idx} out of range for slot \"{}\" ({} attachments)",
                    clip.name,
                    slot.name,
                    names.len()
                )
            })?;
            obj.insert("name".into(), json!(name));
        }
        out.push(Value::Object(obj));
    }
    Ok(Value::Array(out))
}

fn bone_entry<'a>(bones: &'a mut Map<String, Value>, name: &str) -> &'a mut Map<String, Value> {
    bones
        .entry(name.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .unwrap()
}

/// Emit a timeline's key array with 4.x curve encoding.
fn emit_keys(keys: &[Key], comps: usize, target: &TimelineTarget) -> Result<Value, String> {
    let mut out: Vec<Value> = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        let mut obj = Map::new();
        if k.time.abs() > 1e-9 {
            obj.insert("time".into(), json!(round4(k.time as f64)));
        }
        // Values. Translate doc offsets are y-down screen px; Spine wants y-up.
        match target {
            TimelineTarget::BoneRotate(_) => {
                obj.insert("value".into(), json!(round4(k.v[0] as f64)));
            }
            TimelineTarget::SlotAlpha(_) => {
                obj.insert("value".into(), json!(round4(k.v[0].clamp(0.0, 1.0) as f64)));
            }
            TimelineTarget::SlotColor(_) => {
                // Spine 4.2 rgb timeline key: colour as an "rrggbb" hex string; the bezier
                // curve (below) carries per-channel easing in 0..1 value space.
                let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                obj.insert(
                    "color".into(),
                    json!(format!("{:02x}{:02x}{:02x}", byte(k.v[0]), byte(k.v[1]), byte(k.v[2]))),
                );
            }
            TimelineTarget::BoneTranslate(_) => {
                obj.insert("x".into(), json!(round4(k.v[0] as f64)));
                obj.insert("y".into(), json!(round4(-k.v[1] as f64)));
            }
            TimelineTarget::BoneScale(_) => {
                obj.insert("x".into(), json!(round4(k.v[0] as f64)));
                obj.insert("y".into(), json!(round4(k.v[1] as f64)));
            }
            TimelineTarget::SlotDeform(_) | TimelineTarget::SlotAttachment(_) => {
                unreachable!("non-scalar timeline routed to emit_keys")
            }
        }
        // Curve OUT of this key (not on the last key).
        if i + 1 < keys.len() {
            let next = &keys[i + 1];
            match &k.curve {
                Curve::Linear => {}
                Curve::Stepped => {
                    obj.insert("curve".into(), json!("stepped"));
                }
                Curve::Bezier(handles) => {
                    if handles.len() != comps * 4 {
                        return Err(format!(
                            "bezier curve has {} handles, expected {}",
                            handles.len(),
                            comps * 4
                        ));
                    }
                    let (t0, t1) = (k.time as f64, next.time as f64);
                    let mut flat: Vec<Value> = Vec::with_capacity(comps * 4);
                    for c in 0..comps {
                        // Component values as emitted (translate y is flipped).
                        let flip = matches!(target, TimelineTarget::BoneTranslate(_)) && c == 1;
                        let sign = if flip { -1.0 } else { 1.0 };
                        let v0 = k.v[c] as f64 * sign;
                        let v1 = next.v[c] as f64 * sign;
                        let hx1 = handles[c * 4] as f64;
                        let hy1 = handles[c * 4 + 1] as f64;
                        let hx2 = handles[c * 4 + 2] as f64;
                        let hy2 = handles[c * 4 + 3] as f64;
                        flat.push(json!(round4(t0 + hx1 * (t1 - t0))));
                        flat.push(json!(round4(v0 + hy1 * (v1 - v0))));
                        flat.push(json!(round4(t0 + hx2 * (t1 - t0))));
                        flat.push(json!(round4(v0 + hy2 * (v1 - v0))));
                    }
                    obj.insert("curve".into(), Value::Array(flat));
                }
            }
        }
        out.push(Value::Object(obj));
    }
    Ok(Value::Array(out))
}

/// Bones ordered so every parent precedes its children. Errors on unknown parents/cycles.
fn bone_emit_order(doc: &StudioDoc) -> Result<Vec<&super::doc::Bone>, String> {
    let mut out: Vec<&super::doc::Bone> = Vec::new();
    let mut emitted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut remaining: Vec<&super::doc::Bone> = doc.bones.iter().collect();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|b| {
            let ready = match b.parent.as_deref() {
                None => true,
                Some(p) => emitted.contains(p),
            };
            if ready {
                emitted.insert(b.name.as_str());
                out.push(b);
            }
            !ready
        });
        if remaining.len() == before {
            let names: Vec<&str> = remaining.iter().map(|b| b.name.as_str()).collect();
            return Err(format!(
                "bones with missing parents or cycles: {}",
                names.join(", ")
            ));
        }
    }
    Ok(out)
}

/// Rotate a vector by `deg` degrees CCW.
fn rotate(x: f64, y: f64, deg: f64) -> (f64, f64) {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    (x * c - y * s, x * s + y * c)
}

fn norm_angle(mut a: f64) -> f64 {
    while a > 180.0 {
        a -= 360.0;
    }
    while a <= -180.0 {
        a += 360.0;
    }
    a
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::doc::*;

    fn source() -> SourceRef {
        SourceRef { variation_id: "v001".into(), width: 1000, height: 800, sha256: "ab".into() }
    }

    #[test]
    fn seed_doc_emits_valid_structure() {
        let doc = StudioDoc::seed(source(), 0.0);
        let v = emit(&doc).unwrap();
        assert_eq!(v["skeleton"]["spine"], json!(SPINE_VERSION));
        assert_eq!(v["skeleton"]["width"], json!(1000.0));
        // root at image centre → spine (0,0): x/y omitted.
        assert_eq!(v["bones"][0]["name"], json!("root"));
        assert!(v["bones"][0].get("x").is_none());
        // body is coincident with root.
        assert_eq!(v["bones"][1]["parent"], json!("root"));
        // slot + region attachment with full-image dims.
        assert_eq!(v["slots"][0]["attachment"], json!("all"));
        let att = &v["skins"][0]["attachments"]["all"]["all"];
        assert_eq!(att["width"], json!(1000));
        assert_eq!(att["height"], json!(800));
        // idle animation with a scale timeline present.
        assert!(v["animations"]["idle"]["bones"]["body"]["scale"].is_array());
    }

    #[test]
    fn child_bone_local_transform_accounts_for_parent_rotation() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        // Parent "body" rotated 90° CCW; child 100px to its screen-right (y-down px).
        doc.bones[1].rotation = 90.0;
        doc.bones.push(Bone {
            name: "arm".into(),
            parent: Some("body".into()),
            x: 600.0, // 100px right of centre (500, 400)
            y: 400.0,
            rotation: 90.0,
            length: 50.0,
            scale_x: 1.0,
            scale_y: 1.0,
        });
        let v = emit(&doc).unwrap();
        let arm = v["bones"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == json!("arm"))
            .unwrap();
        // World spine delta (100, 0) rotated into a +90° frame → local (0, -100).
        assert!((arm["x"].as_f64().unwrap_or(0.0)).abs() < 0.01);
        assert!((arm["y"].as_f64().unwrap() + 100.0).abs() < 0.01);
        // Same world rotation as parent → local rotation omitted.
        assert!(arm.get("rotation").is_none());
        assert_eq!(arm["length"], json!(50.0));
    }

    #[test]
    fn region_attachment_offsets_from_bone_pivot() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        // Part occupies the top-left 200×100 of the image; bone stays at centre.
        doc.parts[0].bbox = Some(Rect { x: 0, y: 0, w: 200, h: 100 });
        let v = emit(&doc).unwrap();
        let att = &v["skins"][0]["attachments"]["all"]["all"];
        // Texture centre (100, 50) y-down → spine (-400, 350); bone at (0,0).
        assert_eq!(att["x"], json!(-400.0));
        assert_eq!(att["y"], json!(350.0));
        assert_eq!(att["width"], json!(200));
    }

    #[test]
    fn curve_denormalization_is_absolute() {
        let doc = StudioDoc::seed(source(), 0.0);
        let v = emit(&doc).unwrap();
        let keys = v["animations"]["idle"]["bones"]["body"]["scale"]
            .as_array()
            .unwrap();
        // Key 0 (t=0, v=1) → key 1 (t=1, v=[1.015, 1.03]) with easeInOut (.42,0,.58,1):
        // x-comp: cx1=0.42, cy1=1.0, cx2=0.58, cy2=1.015.
        let curve = keys[0]["curve"].as_array().unwrap();
        assert_eq!(curve.len(), 8);
        assert!((curve[0].as_f64().unwrap() - 0.42).abs() < 1e-6);
        assert!((curve[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert!((curve[2].as_f64().unwrap() - 0.58).abs() < 1e-6);
        assert!((curve[3].as_f64().unwrap() - 1.015).abs() < 1e-6);
        // y-comp second block: cy2 = 1.03.
        assert!((curve[7].as_f64().unwrap() - 1.03).abs() < 1e-6);
        // Last key has no curve.
        assert!(keys[2].get("curve").is_none());
    }

    #[test]
    fn translate_flips_y_in_values_and_curves() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.clips = vec![Clip {
            id: "c".into(),
            name: "c".into(),
            duration: 1.0,
            looping: false,
            timelines: vec![Timeline {
                target: TimelineTarget::BoneTranslate("body".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0, 0.0], curve: Curve::Linear },
                    Key { time: 1.0, v: vec![10.0, 20.0], curve: Curve::Linear }, // 20px DOWN on screen
                ],
            }],
        }];
        let v = emit(&doc).unwrap();
        let keys = v["animations"]["c"]["bones"]["body"]["translate"]
            .as_array()
            .unwrap();
        assert_eq!(keys[1]["x"], json!(10.0));
        assert_eq!(keys[1]["y"], json!(-20.0)); // y-down doc → y-up spine
    }

    #[test]
    fn slot_color_emits_rgb_hex_timeline() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.clips = vec![Clip {
            id: "c".into(),
            name: "c".into(),
            duration: 1.0,
            looping: false,
            timelines: vec![Timeline {
                target: TimelineTarget::SlotColor("all".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![1.0, 1.0, 1.0], curve: Curve::Stepped },
                    Key { time: 0.5, v: vec![1.0, 0.5, 0.0], curve: Curve::Linear },
                ],
            }],
        }];
        let v = emit(&doc).unwrap();
        let keys = v["animations"]["c"]["slots"]["all"]["rgb"].as_array().unwrap();
        assert_eq!(keys[0]["color"], json!("ffffff"), "white = no tint");
        assert_eq!(keys[0]["curve"], json!("stepped"));
        assert_eq!(keys[1]["color"], json!("ff8000"), "255,128,0 orange"); // round(0.5*255)=128
    }

    #[test]
    fn weighted_mesh_emits_bind_poses_in_bone_space() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        // Two bones side by side; a 2-vertex "strip" weighted one vertex to each.
        doc.bones.push(Bone::new("b2", Some("root".into()), 700.0, 400.0));
        doc.parts[0].bbox = Some(Rect { x: 400, y: 300, w: 400, h: 200 });
        doc.slots[0].attachment = Attachment::Mesh(MeshData {
            vertices: vec![500.0, 400.0, 700.0, 400.0], // at body pivot; at b2 pivot
            uvs: vec![0.0, 0.5, 1.0, 0.5],
            triangles: vec![0, 1, 0],
            hull: 2,
            weights: Some(vec![
                VertexWeights { influences: vec![BoneInfluence { bone: "body".into(), weight: 1.0 }] },
                VertexWeights {
                    influences: vec![
                        BoneInfluence { bone: "body".into(), weight: 0.5 },
                        BoneInfluence { bone: "b2".into(), weight: 0.5 },
                    ],
                },
            ]),
        });
        let v = emit(&doc).unwrap();
        let att = &v["skins"][0]["attachments"]["all"]["all"];
        assert_eq!(att["type"], json!("mesh"));
        let verts = att["vertices"].as_array().unwrap();
        // Vertex 0: 1 influence, bone index of "body" (=1), bind (0,0) since it sits on the pivot.
        assert_eq!(verts[0], json!(1));
        assert_eq!(verts[1], json!(1));
        assert_eq!(verts[2], json!(0.0));
        assert_eq!(verts[3], json!(0.0));
        assert_eq!(verts[4], json!(1.0));
        // Vertex 1: 2 influences; first is "body": world spine (200, 0) from body → bind (200, 0).
        assert_eq!(verts[5], json!(2));
        assert_eq!(verts[6], json!(1));
        assert_eq!(verts[7], json!(200.0));
        assert_eq!(verts[8], json!(0.0));
        assert!((verts[9].as_f64().unwrap() - 0.5).abs() < 1e-6);
        // Second influence is b2 (index 2), bind (0,0).
        assert_eq!(verts[10], json!(2));
        assert_eq!(verts[11], json!(0.0));
        assert_eq!(verts[12], json!(0.0));
    }

    #[test]
    fn additive_blend_emits_lowercase_and_normal_is_omitted() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.slots[0].blend = crate::studio::doc::BlendMode::Additive;
        let v = emit(&doc).unwrap();
        assert_eq!(v["slots"][0]["blend"], json!("additive"));
        doc.slots[0].blend = crate::studio::doc::BlendMode::Normal;
        let v = emit(&doc).unwrap();
        assert!(v["slots"][0].get("blend").is_none());
    }

    #[test]
    fn physics_constraints_emit_with_defaults_and_skip_unknown_bones() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.physics = vec![
            crate::studio::doc::PhysicsSpec::sway("body"),
            crate::studio::doc::PhysicsSpec::sway("ghost"), // unknown → skipped
        ];
        let v = emit(&doc).unwrap();
        let ph = v["physics"].as_array().unwrap();
        assert_eq!(ph.len(), 1);
        assert_eq!(ph[0]["bone"], json!("body"));
        assert_eq!(ph[0]["rotate"], json!(1.0));
        assert_eq!(ph[0]["strength"], json!(60.0));
        assert_eq!(ph[0]["damping"], json!(0.85));
        assert_eq!(ph[0]["order"], json!(0));
    }

    #[test]
    fn missing_parent_errors() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.bones.push(Bone::new("orphan", Some("ghost".into()), 0.0, 0.0));
        assert!(emit(&doc).is_err());
    }

    /// A 2-vertex mesh on the "all" slot, whose "body" bone is unrotated/unit-scale by default.
    fn mesh_doc() -> StudioDoc {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.slots[0].attachment = Attachment::Mesh(MeshData {
            vertices: vec![400.0, 300.0, 600.0, 500.0],
            uvs: vec![0.0, 0.0, 1.0, 1.0],
            triangles: vec![0, 1, 0],
            hull: 2,
            weights: None,
        });
        doc
    }

    fn deform_clip(keys: Vec<Key>) -> Clip {
        Clip {
            id: "c".into(),
            name: "c".into(),
            duration: 1.0,
            looping: false,
            timelines: vec![Timeline { target: TimelineTarget::SlotDeform("all".into()), keys }],
        }
    }

    #[test]
    fn deform_offsets_unrotated_bone_only_flip_y() {
        let mut doc = mesh_doc();
        doc.clips = vec![deform_clip(vec![
            Key { time: 0.0, v: vec![0.0, 0.0, 0.0, 0.0], curve: Curve::Linear },
            Key { time: 0.5, v: vec![10.0, 20.0, -4.0, 6.0], curve: Curve::Linear },
        ])];
        let v = emit(&doc).unwrap();
        let keys = v["animations"]["c"]["attachments"]["default"]["all"]["all"]["deform"]
            .as_array()
            .unwrap();
        // Setup-pose key (all zero) — time omitted, full vertex array emitted.
        assert!(keys[0].get("time").is_none());
        assert_eq!(keys[0]["vertices"], json!([0.0, 0.0, 0.0, 0.0]));
        // Deltas on an unrotated unit-scale bone → just (dx, -dy).
        assert_eq!(keys[1]["time"], json!(0.5));
        assert_eq!(keys[1]["vertices"], json!([10.0, -20.0, -4.0, -6.0]));
    }

    #[test]
    fn deform_offsets_rotated_scaled_bone_apply_full_linear_map() {
        let mut doc = mesh_doc();
        // body: 90° CCW, non-uniform scale (2, 1). A = diag(1/2,1)·Rot(-90)·diag(1,-1).
        doc.bones[1].rotation = 90.0;
        doc.bones[1].scale_x = 2.0;
        doc.bones[1].scale_y = 1.0;
        doc.clips = vec![deform_clip(vec![
            Key { time: 0.0, v: vec![0.0, 0.0, 0.0, 0.0], curve: Curve::Linear },
            Key { time: 0.5, v: vec![10.0, 20.0, -4.0, 6.0], curve: Curve::Linear },
        ])];
        let v = emit(&doc).unwrap();
        let keys = v["animations"]["c"]["attachments"]["default"]["all"]["all"]["deform"]
            .as_array()
            .unwrap();
        // d=(10,20): yflip→(10,-20); rot(-90)→(-20,-10); /scale→(-10,-10).
        // d=(-4,6):  yflip→(-4,-6);  rot(-90)→(-6,4);    /scale→(-3,4).
        assert_eq!(keys[1]["vertices"], json!([-10.0, -10.0, -3.0, 4.0]));
    }

    #[test]
    fn deform_key_vertex_count_mismatch_errors() {
        let mut doc = mesh_doc();
        // Mesh has 2 verts (4 floats); a 2-float key is one vertex short.
        doc.clips = vec![deform_clip(vec![Key {
            time: 0.0,
            v: vec![1.0, 2.0],
            curve: Curve::Linear,
        }])];
        assert!(emit(&doc).is_err());
    }

    #[test]
    fn deform_on_region_slot_errors() {
        let mut doc = StudioDoc::seed(source(), 0.0); // "all" is Region by default
        doc.clips = vec![deform_clip(vec![Key {
            time: 0.0,
            v: vec![],
            curve: Curve::Linear,
        }])];
        assert!(emit(&doc).is_err());
    }

    #[test]
    fn attachment_timeline_names_alternates_and_hides() {
        let mut doc = StudioDoc::seed(source(), 0.0);
        doc.parts.push(Part {
            id: "eyes_closed".into(),
            name: "eyes_closed".into(),
            prompts: vec![],
            bbox: Some(Rect { x: 0, y: 0, w: 100, h: 100 }),
            mask_hash: None,
            completed_hash: None,
            completed_bbox: None,
            texture: PartTexture::Cut,
            deformable: false,
            attachment_only: true,
        });
        doc.slots[0].alternates = vec!["eyes_closed".into()];
        doc.clips = vec![Clip {
            id: "c".into(),
            name: "c".into(),
            duration: 1.0,
            looping: false,
            timelines: vec![Timeline {
                target: TimelineTarget::SlotAttachment("all".into()),
                keys: vec![
                    Key { time: 0.0, v: vec![0.0], curve: Curve::Stepped },
                    Key { time: 0.1, v: vec![1.0], curve: Curve::Stepped },
                    Key { time: 0.2, v: vec![HIDE_INDEX], curve: Curve::Stepped },
                ],
            }],
        }];
        let v = emit(&doc).unwrap();
        // Skin registers BOTH the base and the alternate under the slot.
        let atts = v["skins"][0]["attachments"]["all"].as_object().unwrap();
        assert!(atts.contains_key("all"), "base attachment present");
        assert!(atts.contains_key("eyes_closed"), "alternate registered in skin");
        // Timeline maps index → attachment name, negative → null (hidden).
        let keys = v["animations"]["c"]["slots"]["all"]["attachment"]
            .as_array()
            .unwrap();
        assert_eq!(keys[0]["name"], json!("all"));
        assert!(keys[0].get("time").is_none());
        assert_eq!(keys[1]["name"], json!("eyes_closed"));
        assert_eq!(keys[1]["time"], json!(0.1));
        assert_eq!(keys[2]["name"], Value::Null);
    }
}
