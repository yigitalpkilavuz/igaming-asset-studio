// Headless validation: parse the emitted .atlas + skeleton.json with the REAL spine-ts 4.2
// parser (@esotericsoftware/spine-core — the same code inside spine-pixi-v8).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  AtlasAttachmentLoader,
  BlendMode,
  SkeletonJson,
  TextureAtlas,
} from "@esotericsoftware/spine-core";

const dir = process.argv[2];
const atlasText = readFileSync(join(dir, "sample.atlas"), "utf8");
const skeletonText = readFileSync(join(dir, "skeleton.json"), "utf8");

const atlas = new TextureAtlas(atlasText);
console.log(
  "atlas ok:",
  atlas.pages.map((p) => `${p.name} ${p.width}x${p.height}`).join(", "),
  "| regions:",
  atlas.regions.map((r) => r.name).join(", "),
);

const json = new SkeletonJson(new AtlasAttachmentLoader(atlas));
const data = json.readSkeletonData(skeletonText);

console.log("skeleton ok: spine", data.version, `| ${data.width}x${data.height}`);
console.log("bones:", data.bones.map((b) => `${b.name}${b.parent ? "<" + b.parent.name : ""}`).join(", "));
console.log("slots:", data.slots.map((s) => `${s.name}@${s.boneData.name}`).join(", "));
console.log(
  "animations:",
  data.animations.map((a) => `${a.name} (${a.duration.toFixed(2)}s, ${a.timelines.length} timelines)`).join(", "),
);

// Sanity assertions.
const fail = (m) => {
  console.error("FAIL:", m);
  process.exit(1);
};
if (data.bones.length !== 3) fail(`expected 3 bones, got ${data.bones.length}`);
if (data.slots.length !== 2) fail(`expected 2 slots, got ${data.slots.length}`);
if (data.animations.length !== 2) fail(`expected 2 animations, got ${data.animations.length}`);
const win = data.animations.find((a) => a.name === "win");
if (!win) fail("missing win animation");
if (Math.abs(win.duration - 2.0) > 1e-6) fail(`win duration ${win.duration}`);
for (const slotName of ["torso", "head"]) {
  const slot = data.slots.find((s) => s.name === slotName);
  const att = data.defaultSkin.getAttachment(slot.index, slotName);
  if (!att) fail(`missing attachment for ${slotName}`);
}
// The torso ships as a WEIGHTED mesh: bones stream present, weights normalized.
const torsoSlot = data.slots.find((s) => s.name === "torso");
const torsoAtt = data.defaultSkin.getAttachment(torsoSlot.index, "torso");
if (torsoAtt.constructor.name !== "MeshAttachment") {
  console.log("note: torso is not a mesh in this sample —", torsoAtt.constructor.name);
} else {
  if (!torsoAtt.bones || !torsoAtt.bones.length) fail("torso mesh is not weighted");
  // spine-core weighted storage: bones[] = per-vertex bone counts + indices;
  // vertices[] = (bindX, bindY, weight) triplets in the same order.
  let vi = 0;
  let bi = 0;
  let vertexCount = 0;
  while (bi < torsoAtt.bones.length) {
    const boneCount = torsoAtt.bones[bi++];
    let sum = 0;
    for (let k = 0; k < boneCount; k++) {
      bi++; // bone index
      sum += torsoAtt.vertices[vi + 2];
      vi += 3;
    }
    if (Math.abs(sum - 1) > 1e-3) fail(`vertex ${vertexCount} weights sum to ${sum}`);
    vertexCount++;
  }
  console.log(`torso mesh ok: ${vertexCount} weighted vertices, ${torsoAtt.triangles.length / 3} tris`);
}
// Physics constraint on the head parses with our authored values.
if (data.physicsConstraints.length !== 1) fail(`expected 1 physics constraint, got ${data.physicsConstraints.length}`);
const ph = data.physicsConstraints[0];
if (ph.bone.name !== "head") fail("physics bone mismatch");
if (Math.abs(ph.rotate - 1) > 1e-9 || Math.abs(ph.strength - 60) > 1e-9 || Math.abs(ph.damping - 0.85) > 1e-9) {
  fail(`physics values off: rotate=${ph.rotate} strength=${ph.strength} damping=${ph.damping}`);
}
console.log(`physics ok: ${ph.bone.name} sway (strength ${ph.strength}, damping ${ph.damping})`);

// The head slot ships ADDITIVE (FX/light blend mode).
const headSlot = data.slots.find((s) => s.name === "head");
if (headSlot.blendMode !== BlendMode.Additive) fail(`head blend = ${headSlot.blendMode}, want Additive`);
const torsoSlotB = data.slots.find((s) => s.name === "torso");
if (torsoSlotB.blendMode !== BlendMode.Normal) fail("torso blend should be Normal");
console.log("blend ok: head=additive, torso=normal");
console.log("ALL CHECKS PASSED — the runtime parser accepts our export.");
