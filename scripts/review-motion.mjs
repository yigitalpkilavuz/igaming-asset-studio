// Headless GEOMETRY review of an exported animation. Loads the emitted atlas + skeleton with
// the REAL spine-core 4.2 runtime, samples each animation over time, computes the WORLD vertices
// of every part, and measures body distortion that reads as a "deformity":
//   • shear   — a rigid part's quad corner angles drifting from their setup 90° (a parent's
//               non-uniform scale shearing a rotated child part is the classic mascot deformity)
//   • aspect  — a part's width:height ratio drifting from setup (non-uniform scale stretch)
//   • fold    — (meshes) triangles flipping sign = the surface folding through itself
// Usage: node scripts/review-motion.mjs <export-dir>
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import * as core from "@esotericsoftware/spine-core";

const { AtlasAttachmentLoader, TextureAtlas, SkeletonJson, Skeleton, MeshAttachment, RegionAttachment, MixBlend, MixDirection, Physics } = core;

const dir = process.argv[2];
if (!dir) {
  console.error("usage: node scripts/review-motion.mjs <export-dir>");
  process.exit(1);
}
const atlasFile = readdirSync(dir).find((f) => f.endsWith(".atlas"));
if (!atlasFile) {
  console.error(`no .atlas in ${dir}`);
  process.exit(1);
}
const atlas = new TextureAtlas(readFileSync(join(dir, atlasFile), "utf8"));
const json = new SkeletonJson(new AtlasAttachmentLoader(atlas));
const data = json.readSkeletonData(readFileSync(join(dir, "skeleton.json"), "utf8"));
const skeleton = new Skeleton(data);
const phys = Physics ? Physics.update : undefined;

const RAD = 180 / Math.PI;
const corners = (v) => [[v[0], v[1]], [v[2], v[3]], [v[4], v[5]], [v[6], v[7]]];
function quadAngles(v) {
  const p = corners(v);
  return [0, 1, 2, 3].map((i) => {
    const a = p[(i + 3) % 4], b = p[i], c = p[(i + 1) % 4];
    const u = [a[0] - b[0], a[1] - b[1]], w = [c[0] - b[0], c[1] - b[1]];
    return Math.abs(Math.atan2(Math.abs(u[0] * w[1] - u[1] * w[0]), u[0] * w[0] + u[1] * w[1]) * RAD);
  });
}
function quadAspect(v) {
  const p = corners(v);
  const e = [0, 1, 2, 3].map((i) => Math.hypot(p[(i + 1) % 4][0] - p[i][0], p[(i + 1) % 4][1] - p[i][1]));
  return (e[0] + e[2]) / (e[1] + e[3] || 1e-9); // mean top/bottom over mean left/right
}
function regionWorld(slot, att) {
  const w = new Array(8).fill(0);
  att.computeWorldVertices(slot, w, 0, 2);
  return w;
}
function meshWorld(slot, att) {
  const n = att.worldVerticesLength;
  const w = new Array(n).fill(0);
  att.computeWorldVertices(slot, 0, n, w, 0, 2);
  return w;
}
const triSign = (verts, tris) => {
  const s = [];
  for (let i = 0; i < tris.length; i += 3) {
    const a = tris[i] * 2, b = tris[i + 1] * 2, c = tris[i + 2] * 2;
    s.push(Math.sign((verts[b] - verts[a]) * (verts[c + 1] - verts[a + 1]) - (verts[c] - verts[a]) * (verts[b + 1] - verts[a + 1])));
  }
  return s;
};

function pose(t, loop) {
  skeleton.setToSetupPose();
  anim.apply(skeleton, 0, t, loop, [], 1, MixBlend.setup, MixDirection.mixIn);
  skeleton.updateWorldTransform(phys);
}

let anim;
let hadWarning = false;
console.log(`skeleton: spine ${data.version} · ${data.bones.length} bones · ${data.slots.length} slots\n`);
for (const a of data.animations) {
  anim = a;
  const loop = !/^(win|win_big|mega|anticip|bonus|expand|reveal|land|scatter|celebrate|hit)/i.test(a.name);
  pose(0, loop);
  const setup = {};
  for (const slot of skeleton.slots) {
    const att = slot.getAttachment();
    if (att instanceof RegionAttachment) setup[slot.data.name] = { ang: quadAngles(regionWorld(slot, att)), asp: quadAspect(regionWorld(slot, att)) };
    else if (att instanceof MeshAttachment) setup[slot.data.name] = { sign: triSign(meshWorld(slot, att), att.triangles), tris: att.triangles };
  }

  const worst = {};
  const N = 60;
  for (let s = 0; s <= N; s++) {
    const t = (a.duration * s) / N;
    pose(t, loop);
    for (const slot of skeleton.slots) {
      const att = slot.getAttachment();
      const name = slot.data.name;
      const ref = setup[name];
      if (!ref) continue;
      const cur = worst[name] || { kind: "?", shear: 0, aspect: 0, folds: 0, tS: 0, tA: 0 };
      if (att instanceof RegionAttachment) {
        cur.kind = "region";
        const w = regionWorld(slot, att);
        const shear = Math.max(...quadAngles(w).map((x, i) => Math.abs(x - ref.ang[i])));
        const aspect = Math.abs(quadAspect(w) / ref.asp - 1) * 100;
        if (shear > cur.shear) { cur.shear = shear; cur.tS = t; }
        if (aspect > cur.aspect) { cur.aspect = aspect; cur.tA = t; }
      } else if (att instanceof MeshAttachment) {
        cur.kind = "mesh";
        const sign = triSign(meshWorld(slot, att), att.triangles);
        cur.folds = Math.max(cur.folds, sign.filter((v, i) => v !== ref.sign[i] && ref.sign[i] !== 0).length);
      }
      worst[name] = cur;
    }
  }

  console.log(`=== ${a.name}  (${a.duration.toFixed(2)}s, ${loop ? "loop" : "one-shot"}) ===`);
  for (const [name, w] of Object.entries(worst)) {
    const bad = w.shear > 4 || w.aspect > 10 || w.folds > 0;
    if (bad) hadWarning = true;
    const detail = w.kind === "mesh"
      ? `folded tris=${w.folds}`
      : `shear=${w.shear.toFixed(1)}° @${w.tS.toFixed(2)}s  aspectDrift=${w.aspect.toFixed(1)}% @${w.tA.toFixed(2)}s`;
    console.log(`  ${name.padEnd(8)} ${w.kind.padEnd(6)} ${detail}${bad ? "   ⚠ DEFORMED" : ""}`);
  }
  console.log("");
}
console.log(hadWarning ? "⚠ distortion detected — see ⚠ rows above." : "✓ no body distortion: every part stays rigid (rotate/uniform-scale/translate only).");
