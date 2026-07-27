//! 2.5D depth turn: give a flat part volume by treating a depth/relief map as a surface
//! height, rotating that surface in yaw/pitch, and projecting it through a pinhole camera.
//! The per-frame projected offsets bake into a Spine mesh-DEFORM timeline, so the effect plays
//! on the stock runtime with no custom code.
//!
//! Offsets are measured against the flat (un-turned) projection, so the setup mesh IS the rest
//! pose and — because the yaw/pitch drivers use whole cycles — the clip loops seamlessly.
//!
//! The mesh must be UNWEIGHTED: deform offsets are authoritative and 1:1 with vertices (the
//! runtime's deform length for a weighted mesh counts influences, not vertices).

use serde::{Deserialize, Serialize};
use specta::Type;

use super::doc::{Curve, Key, MeshData, Rect, Timeline, TimelineTarget};

/// Knobs for a generated turn clip. All have sane defaults; the UI exposes them as sliders.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TurnParams {
    /// Peak yaw (left↔right turn) in degrees.
    #[serde(default = "d_yaw")]
    pub yaw_amp: f64,
    /// Peak pitch (up↕down tilt) in degrees.
    #[serde(default)]
    pub pitch_amp: f64,
    /// Surface relief as a fraction of the part's long side — how far near/far pixels sit in Z.
    #[serde(default = "d_depth")]
    pub depth: f64,
    /// Pinhole focal length as a multiple of the long side; larger = weaker perspective.
    #[serde(default = "d_lens")]
    pub lens: f64,
    /// Whole cycles over the clip (kept integral so the loop closes seamlessly).
    #[serde(default = "d_cycles")]
    pub cycles: f64,
    /// Clip duration in seconds.
    #[serde(default = "d_duration")]
    pub duration: f64,
    /// Pin the silhouette (0 = off, 1 = strong): attenuates deform near transparent edges so the
    /// outline doesn't tear/shear on strong tilts. The interior keeps its full turn.
    #[serde(default = "d_edge_pin")]
    pub edge_pin: f64,
}

fn d_yaw() -> f64 {
    22.0
}
fn d_depth() -> f64 {
    0.12
}
fn d_lens() -> f64 {
    2.5
}
fn d_cycles() -> f64 {
    1.0
}
fn d_duration() -> f64 {
    2.5
}
fn d_edge_pin() -> f64 {
    0.5
}

impl Default for TurnParams {
    fn default() -> Self {
        Self {
            yaw_amp: d_yaw(),
            pitch_amp: 0.0,
            depth: d_depth(),
            lens: d_lens(),
            cycles: d_cycles(),
            duration: d_duration(),
            edge_pin: d_edge_pin(),
        }
    }
}

/// Clean a raw part depth estimate: subject-normalize over the OPAQUE pixels (so the
/// transparent background no longer compresses the range), edge-aware smooth snapped to the
/// art (guided filter — kills the DPT seam artifacts), then flatten the background to neutral
/// 0.5 so background vertices don't warp. `alpha`/`relief` are row-major `w×h`; `rgb` is the
/// same-size guide the depth model saw.
pub fn clean_relief(relief: &[f32], alpha: &[u8], rgb: &image::RgbImage, w: u32, h: u32) -> Vec<f32> {
    let n = (w * h) as usize;
    if relief.len() != n || alpha.len() != n || (rgb.width() * rgb.height()) as usize != n {
        return relief.to_vec();
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for i in 0..n {
        if alpha[i] > 8 {
            lo = lo.min(relief[i]);
            hi = hi.max(relief[i]);
        }
    }
    if lo > hi {
        return relief.to_vec(); // no opaque pixels
    }
    let range = (hi - lo).max(1e-4);
    let normed: Vec<f32> = relief.iter().map(|&r| ((r - lo) / range).clamp(0.0, 1.0)).collect();
    let mut out = crate::studio::matte::guided_filter_rgb(&normed, rgb, 8, 1e-4);
    for i in 0..n {
        if alpha[i] <= 8 {
            out[i] = 0.5;
        }
    }
    out
}

/// How much REAL depth variation the subject has — p95−p5 of the raw estimate over opaque
/// pixels, mapped to a depth gain in [0.4, 1.0]. Flat art (tiny spread) turns subtly instead of
/// amplifying noise once subject-normalized; genuinely 3-D art turns at full strength.
pub fn depth_gain(relief: &[f32], alpha: &[u8]) -> f64 {
    let mut vals: Vec<f32> =
        relief.iter().zip(alpha).filter(|(_, &a)| a > 8).map(|(&r, _)| r).collect();
    if vals.len() < 16 {
        return 1.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f64| vals[(((vals.len() - 1) as f64) * q) as usize] as f64;
    let spread = (pct(0.95) - pct(0.05)).max(0.0);
    let t = ((spread - 0.03) / 0.12).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t); // smoothstep
    0.4 + 0.6 * s
}

/// Samples per turn cycle — dense enough that Linear-interpolated deform reads as smooth.
const FRAMES_PER_CYCLE: usize = 16;

/// Bilinear sample of a row-major `w×h` relief buffer at pixel coords (px, py).
fn sample(relief: &[f32], w: u32, h: u32, px: f64, py: f64) -> f64 {
    if w == 0 || h == 0 || relief.is_empty() {
        return 0.5;
    }
    let x = px.clamp(0.0, (w - 1) as f64);
    let y = py.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |xx: u32, yy: u32| relief[(yy * w + xx) as usize] as f64;
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bot = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bot * fy
}

/// Bake a looping 2.5D turn into a mesh-deform timeline for `slot`.
///
/// `relief` is a `relief_w×relief_h` height map (0..1, larger = nearer) aligned to the part
/// texture; `mesh` is the UNWEIGHTED grid mesh whose UVs index into that texture; `bbox` is the
/// part's box in source pixels — its long side is the PHYSICAL scale (relief depth, camera
/// distance) and its centre is the turn axis, independent of the relief buffer's resolution.
pub fn bake(
    slot: &str,
    mesh: &MeshData,
    bbox: Rect,
    relief: &[f32],
    alpha: &[u8],
    relief_w: u32,
    relief_h: u32,
    params: &TurnParams,
) -> Timeline {
    let vcount = mesh.vertices.len() / 2;
    let (cx, cy) = (
        bbox.x as f64 + bbox.w as f64 / 2.0,
        bbox.y as f64 + bbox.h as f64 / 2.0,
    );
    let max_dim = bbox.w.max(bbox.h) as f64;
    let relief_scale = params.depth.max(0.0) * max_dim;
    let cam_d = (params.lens.max(0.05) * max_dim).max(1.0);
    let clamp_px = 0.25 * max_dim;

    // Per-vertex silhouette factor for edge-pin: sample alpha at the vertex UV; the interior
    // (alpha≈1) keeps its full turn, the outer antialiased band fades to pinned. `edge_pin`
    // scales how strongly the rim is held (0 = no pin).
    let ep = params.edge_pin.clamp(0.0, 1.0);
    let alpha_f: Vec<f32> = alpha.iter().map(|&a| a as f32 / 255.0).collect();
    let edge: Vec<f64> = (0..vcount)
        .map(|i| {
            if ep <= 0.0 || alpha_f.len() != (relief_w * relief_h) as usize {
                return 1.0;
            }
            let (u, v) = (mesh.uvs[i * 2], mesh.uvs[i * 2 + 1]);
            let a = sample(
                &alpha_f,
                relief_w,
                relief_h,
                u * (relief_w.max(1) - 1) as f64,
                v * (relief_h.max(1) - 1) as f64,
            );
            let t = (a / 0.6).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t); // smoothstep: keep interior full, fade the AA band
            1.0 - ep * (1.0 - s)
        })
        .collect();

    // Pinhole projection of a surface point (x, y, z) after a yaw then a pitch rotation.
    // Axes: x right, y down, z toward the camera; camera sits at +z distance `cam_d`.
    let project = |x: f64, y: f64, z: f64, yaw_deg: f64, pitch_deg: f64| -> (f64, f64) {
        let (sy, cyw) = yaw_deg.to_radians().sin_cos();
        let (x1, z1) = (x * cyw + z * sy, -x * sy + z * cyw);
        let (sp, cp) = pitch_deg.to_radians().sin_cos();
        let (y2, z2) = (y * cp - z1 * sp, y * sp + z1 * cp);
        let s = cam_d / (cam_d - z2).max(1.0);
        (x1 * s, y2 * s)
    };

    // Per-vertex setup point (centred X/Y + relief Z) and its flat-pose projection (the rest,
    // from which offsets are measured).
    let mut pts: Vec<(f64, f64, f64)> = Vec::with_capacity(vcount);
    let mut flat: Vec<(f64, f64)> = Vec::with_capacity(vcount);
    for i in 0..vcount {
        let (vx, vy) = (mesh.vertices[i * 2], mesh.vertices[i * 2 + 1]);
        let (u, v) = (mesh.uvs[i * 2], mesh.uvs[i * 2 + 1]);
        let h = sample(
            relief,
            relief_w,
            relief_h,
            u * (relief_w.max(1) - 1) as f64,
            v * (relief_h.max(1) - 1) as f64,
        );
        let (x, y, z) = (vx - cx, vy - cy, (h - 0.5) * relief_scale);
        pts.push((x, y, z));
        flat.push(project(x, y, z, 0.0, 0.0));
    }

    let cyc = params.cycles.round().max(1.0);
    let n = (cyc as usize).max(1) * FRAMES_PER_CYCLE;
    let dur = params.duration.clamp(0.4, 12.0);

    let mut keys: Vec<Key> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let f = i as f64 / n as f64;
        let theta = 2.0 * std::f64::consts::PI * cyc * f;
        let (yaw, pitch) = (params.yaw_amp * theta.sin(), params.pitch_amp * theta.sin());
        let mut v: Vec<f64> = Vec::with_capacity(vcount * 2);
        for (i, &(x, y, z)) in pts.iter().enumerate() {
            let (px, py) = project(x, y, z, yaw, pitch);
            let dx = (px - flat[i].0).clamp(-clamp_px, clamp_px) * edge[i];
            let dy = (py - flat[i].1).clamp(-clamp_px, clamp_px) * edge[i];
            v.push((dx * 100.0).round() / 100.0);
            v.push((dy * 100.0).round() / 100.0);
        }
        keys.push(Key { time: dur * f, v, curve: Curve::Linear });
    }
    Timeline { target: TimelineTarget::SlotDeform(slot.to_string()), keys }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two coincident vertices (X=Y=0) that sample opposite relief extremes, plus a flat one at
    /// mid-relief (Z=0). Under yaw the near vertex parallaxes one way, the far the other, and the
    /// flat vertex barely moves — the signature of real depth.
    fn mesh() -> MeshData {
        MeshData {
            // All three at the bbox centre (50,50) so X=Y=0 and only Z drives horizontal shift.
            vertices: vec![50.0, 50.0, 50.0, 50.0, 50.0, 50.0],
            uvs: vec![0.0, 0.5, 0.5, 0.5, 1.0, 0.5], // near / mid / far columns
            triangles: vec![0, 1, 2],
            hull: 3,
            weights: None,
        }
    }

    // 3×1 relief: near (1.0) · mid (0.5) · far (0.0).
    fn relief() -> Vec<f32> {
        vec![1.0, 0.5, 0.0]
    }

    #[test]
    fn loop_closes_and_vertex_count_matches() {
        let m = mesh();
        let tl = bake(
            "s",
            &m,
            Rect { x: 0, y: 0, w: 100, h: 100 },
            &relief(),
            &[255u8; 3],
            3,
            1,
            &TurnParams::default(),
        );
        let keys = &tl.keys;
        assert!(keys.len() > 4);
        // Every key carries 2×vertex_count offsets.
        for k in keys {
            assert_eq!(k.v.len(), 6);
        }
        // First and last frame are the flat rest pose (all ~0) and equal → seamless loop.
        let first = &keys[0].v;
        let last = &keys[keys.len() - 1].v;
        for c in first {
            assert!(c.abs() < 1e-6, "rest frame is flat");
        }
        for (a, b) in first.iter().zip(last) {
            assert!((a - b).abs() < 1e-6, "loop closes");
        }
    }

    #[test]
    fn relief_drives_opposing_parallax() {
        let m = mesh();
        let tl = bake(
            "s",
            &m,
            Rect { x: 0, y: 0, w: 100, h: 100 },
            &relief(),
            &[255u8; 3],
            3,
            1,
            &TurnParams { yaw_amp: 30.0, pitch_amp: 0.0, ..Default::default() },
        );
        // Peak yaw is a quarter through the (single) cycle.
        let peak = &tl.keys[tl.keys.len() / 4].v;
        let (near_dx, mid_dx, far_dx) = (peak[0], peak[2], peak[4]);
        // Near and far pixels shift horizontally in OPPOSITE directions (they're on opposite
        // sides of the turning surface); the mid pixel sits on the axis and barely moves.
        assert!(near_dx.abs() > 1.0, "near pixel parallaxes: {near_dx}");
        assert!(far_dx.abs() > 1.0, "far pixel parallaxes: {far_dx}");
        assert!(near_dx.signum() != far_dx.signum(), "near/far shift opposite ways");
        assert!(mid_dx.abs() < near_dx.abs(), "axis pixel moves least");
    }

    #[test]
    fn edge_pin_holds_the_transparent_rim() {
        // Two verts at the same spot (X=Y=0, so only Z drives offset), same high relief; one on
        // opaque interior, one on a transparent rim column.
        let m = MeshData {
            vertices: vec![50.0, 50.0, 50.0, 50.0],
            uvs: vec![0.0, 0.5, 1.0, 0.5],
            triangles: vec![0, 1, 0],
            hull: 2,
            weights: None,
        };
        let relief = vec![1.0f32, 1.0];
        let alpha = [255u8, 0u8]; // vert0 interior, vert1 transparent
        let p = TurnParams { yaw_amp: 30.0, edge_pin: 1.0, ..Default::default() };
        let tl = bake("s", &m, Rect { x: 0, y: 0, w: 100, h: 100 }, &relief, &alpha, 2, 1, &p);
        let peak = &tl.keys[tl.keys.len() / 4].v;
        let v0 = (peak[0] * peak[0] + peak[1] * peak[1]).sqrt();
        let v1 = (peak[2] * peak[2] + peak[3] * peak[3]).sqrt();
        assert!(v0 > 1.0, "interior keeps its turn: {v0}");
        assert!(v1 < 0.1, "transparent rim is pinned: {v1}");
    }

    #[test]
    fn depth_gain_scales_with_real_variation() {
        let alpha = vec![255u8; 64];
        let flat = vec![0.5f32; 64];
        assert!((depth_gain(&flat, &alpha) - 0.4).abs() < 0.05, "flat art → floor gain");
        let deep: Vec<f32> = (0..64).map(|i| if i % 2 == 0 { 0.0 } else { 1.0 }).collect();
        assert!(depth_gain(&deep, &alpha) > 0.95, "deep art → full gain");
    }

    #[test]
    fn clean_relief_flattens_the_background() {
        let (w, h) = (16u32, 16u32);
        let n = (w * h) as usize;
        let relief: Vec<f32> = (0..n).map(|i| (i % 7) as f32 / 6.0).collect();
        let alpha: Vec<u8> = (0..n).map(|i| if i < 40 { 0 } else { 255 }).collect();
        let rgb = image::RgbImage::from_pixel(w, h, image::Rgb([120, 120, 120]));
        let out = clean_relief(&relief, &alpha, &rgb, w, h);
        assert_eq!(out.len(), n);
        for i in 0..40 {
            assert!((out[i] - 0.5).abs() < 1e-6, "background neutralized to 0.5");
        }
    }

    /// Dev harness (env-gated): run the REAL Depth-Anything model on a real part crop, bake a
    /// turn, and dump a relief heatmap + a peak-yaw displacement field so the parallax can be
    /// eyeballed. No-op unless `WF_TURN_DUMP` is set and the depth weights are present.
    #[test]
    fn turn_real_part_dump() {
        let Some(out) = std::env::var_os("WF_TURN_DUMP") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        let config = std::path::PathBuf::from(std::env::var_os("WF_CONFIG_DIR").unwrap_or_else(|| {
            "/Users/yigitalp/Library/Application Support/com.wishfell.assetpipeline".into()
        }));
        if !crate::layers::depth::weights_ready(&config) {
            eprintln!("depth weights not ready under {} — skipping", config.display());
            return;
        }
        let part = std::env::var_os("WF_TURN_PART")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                config.join("projects/babewyn_court/assets/symbol_h1/studio/parts/all/cut.png")
            });
        let tex = image::open(&part).expect("open part texture").to_rgba8();
        let (tw, th) = tex.dimensions();
        let bbox = Rect { x: 0, y: 0, w: tw, h: th };

        // Flatten over neutral gray for the depth model.
        let mut rgb = image::RgbImage::new(tw, th);
        for (x, y, p) in tex.enumerate_pixels() {
            let a = p.0[3] as u32;
            let blend = |c: u8| ((c as u32 * a + 128 * (255 - a)) / 255) as u8;
            rgb.put_pixel(x, y, image::Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
        }
        let raw = crate::layers::depth::estimate(&config, &rgb).expect("depth estimate");
        let (lo, hi) = raw
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        eprintln!("raw relief {tw}x{th}  range [{lo:.3}, {hi:.3}]");
        assert!(hi - lo > 0.05, "relief must have real depth variation");

        // Cleanup pipeline: subject-normalize + edge-aware smooth + bg-neutral, plus auto-depth.
        let alpha: Vec<u8> = tex.pixels().map(|p| p.0[3]).collect();
        let gain = depth_gain(&raw, &alpha);
        let relief = clean_relief(&raw, &alpha, &rgb, tw, th);
        eprintln!("depth_gain={gain:.2}");

        let mesh = crate::studio::mesh::generate_cells(&tex, bbox, 12).expect("mesh");
        let vc = mesh.vertices.len() / 2;
        let mut params = TurnParams { yaw_amp: 26.0, pitch_amp: 6.0, ..Default::default() };
        params.depth *= gain;
        let tl = bake("all", &mesh, bbox, &relief, &alpha, tw, th, &params);
        for k in &tl.keys {
            assert_eq!(k.v.len(), vc * 2, "deform key length matches mesh");
        }
        assert!(
            tl.keys[0]
                .v
                .iter()
                .zip(&tl.keys.last().unwrap().v)
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "loop closes on the real part"
        );

        // Full glue: the baked mesh + deform clip must emit valid Spine (what the command does).
        {
            use crate::studio::doc::*;
            let mut sdoc = StudioDoc::seed(
                SourceRef { variation_id: "v".into(), width: tw, height: th, sha256: "s".into() },
                0.0,
            );
            sdoc.parts[0].bbox = Some(bbox);
            sdoc.slots[0].attachment = Attachment::Mesh(mesh.clone());
            sdoc.clips = vec![Clip {
                id: "turn".into(),
                name: "turn".into(),
                duration: params.duration,
                looping: true,
                timelines: vec![tl.clone()],
            }];
            let emitted = crate::studio::spine42::emit(&sdoc).expect("turn doc emits");
            assert!(
                emitted["animations"]["turn"]["attachments"]["default"]["all"]["all"]["deform"]
                    .is_array(),
                "deform timeline present in emitted skeleton"
            );
            eprintln!("emit glue ok: {vc} verts → deform clip");
        }

        // Dump 1: relief heatmap (blue = far, warm = near).
        let mut heat = image::RgbImage::new(tw, th);
        for (i, px) in heat.pixels_mut().enumerate() {
            let v = (relief[i].clamp(0.0, 1.0) * 255.0) as u8;
            *px = image::Rgb([v, (v as u32 * 90 / 255) as u8, 255 - v]);
        }
        heat.save(out.join("relief.png")).unwrap();

        // Dump 2: displacement field at peak yaw (×4 for visibility) over the dimmed art.
        let peak = &tl.keys[tl.keys.len() / 4].v;
        let mut disp = image::RgbaImage::new(tw, th);
        for (x, y, p) in tex.enumerate_pixels() {
            let d = |c: u8| ((c as u32 * p.0[3] as u32 / 255) / 3 + 16) as u8;
            disp.put_pixel(x, y, image::Rgba([d(p.0[0]), d(p.0[1]), d(p.0[2]), 255]));
        }
        let mut plot = |x0: f64, y0: f64, x1: f64, y1: f64, c: [u8; 3]| {
            let steps = ((x1 - x0).abs().max((y1 - y0).abs())).ceil() as i32 + 1;
            for s in 0..=steps {
                let f = s as f64 / steps.max(1) as f64;
                let (x, y) = ((x0 + (x1 - x0) * f) as i32, (y0 + (y1 - y0) * f) as i32);
                if x >= 0 && y >= 0 && (x as u32) < tw && (y as u32) < th {
                    disp.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        };
        for i in 0..vc {
            let (vx, vy) = (mesh.vertices[i * 2], mesh.vertices[i * 2 + 1]);
            let (dx, dy) = (peak[i * 2] * 4.0, peak[i * 2 + 1] * 4.0);
            plot(vx, vy, vx + dx, vy + dy, [90, 255, 130]);
        }
        disp.save(out.join("displace.png")).unwrap();
        eprintln!("wrote relief.png + displace.png to {}", out.display());
    }
}
