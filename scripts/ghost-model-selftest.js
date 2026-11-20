"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const gm = require("../src/main/game-delivery/ghost-model");

// Модель из одного треугольника в форматах Source (MDL 48, VVD, VTX 7) — те же смещения,
// что читает ghost-model.js.
function tinyModel(points) {
  const mdl = Buffer.alloc(240 + 16 + 148 + 116);
  mdl.write("IDST", 0, "latin1"); mdl.writeInt32LE(48, 4);
  mdl.writeInt32LE(1, 232); mdl.writeInt32LE(240, 236); // одна часть тела
  const bp = 240; mdl.writeInt32LE(1, bp + 4); mdl.writeInt32LE(16, bp + 12); // одна модель сразу за частью
  const model = bp + 16; mdl.writeInt32LE(1, model + 72); mdl.writeInt32LE(148, model + 76); mdl.writeInt32LE(0, model + 84);
  const mesh = model + 148; mdl.writeInt32LE(0, mesh + 12);
  const vvd = Buffer.alloc(64 + 48 * 3);
  vvd.write("IDSV", 0, "latin1"); vvd.writeInt32LE(1, 12); vvd.writeInt32LE(3, 16); vvd.writeInt32LE(64, 56);
  points.forEach(([x, y, z], i) => { const o = 64 + i * 48 + 16; vvd.writeFloatLE(x, o); vvd.writeFloatLE(y, o + 4); vvd.writeFloatLE(z, o + 8); });
  const vtx = Buffer.alloc(36 + 8 + 8 + 12 + 9 + 25 + 27 + 3 * 9 + 3 * 2);
  vtx.writeInt32LE(7, 0); vtx.writeInt32LE(1, 28); vtx.writeInt32LE(36, 32);
  let o = 36; vtx.writeInt32LE(1, o); vtx.writeInt32LE(8, o + 4); // часть тела -> модель
  o += 8; vtx.writeInt32LE(1, o); vtx.writeInt32LE(8, o + 4); // модель -> LOD
  o += 8; vtx.writeInt32LE(1, o); vtx.writeInt32LE(12, o + 4); // LOD -> сетка
  o += 12; vtx.writeInt32LE(1, o); vtx.writeInt32LE(9, o + 4); // сетка -> группа
  const sg = o + 9;
  const verts = sg + 25 + 27, indices = verts + 27, strip = sg + 25;
  vtx.writeInt32LE(3, sg); vtx.writeInt32LE(verts - sg, sg + 4); vtx.writeInt32LE(3, sg + 8); vtx.writeInt32LE(indices - sg, sg + 12);
  vtx.writeInt32LE(1, sg + 16); vtx.writeInt32LE(strip - sg, sg + 20);
  vtx.writeInt32LE(3, strip); vtx.writeInt32LE(0, strip + 4); vtx.writeUInt8(1, strip + 18); // список треугольников
  for (let i = 0; i < 3; i += 1) { vtx.writeUInt16LE(i, verts + i * 9 + 4); vtx.writeUInt16LE(i, indices + i * 2); }
  return { mdl, vvd, vtx };
}

test("модель Source: треугольники из MDL + VVD + VTX", () => {
  const { mdl, vvd, vtx } = tinyModel([[0, 0, 0], [0, 10, 0], [0, 5, 72]]);
  const mesh = gm.parseMesh(mdl, vvd, vtx);
  assert.equal(mesh.triangles, 1);
  assert.deepEqual(mesh.positions.map((v) => Math.round(v)), [0, 0, 0, 0, 10, 0, 0, 5, 72]);
  assert.throws(() => gm.parseMesh(Buffer.from("nope"), vvd, vtx), { code: "MODEL_INVALID" });
});

test("модель призрака ищется там же, где игра: custom/*, мод, VPK hl2; путь только внутри мода", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghost-model-"));
  try {
    const put = (rel, buf) => { const f = path.join(root, ...rel.split("/")); fs.mkdirSync(path.dirname(f), { recursive: true }); fs.writeFileSync(f, buf); };
    const a = tinyModel([[0, 0, 0], [0, 10, 0], [0, 5, 72]]);
    put("ghostingmod/custom/zz/models/my/ghost.mdl", a.mdl); put("ghostingmod/custom/zz/models/my/ghost.vvd", a.vvd); put("ghostingmod/custom/zz/models/my/ghost.dx90.vtx", a.vtx);
    assert.equal(gm.readGhostMesh(root, "models/my/ghost.mdl").triangles, 1);
    // VPK v1 с файлами модели целиком в каталоге (archive 0x7fff).
    const b = tinyModel([[0, 0, 0], [0, 20, 0], [0, 10, 80]]);
    const files = [["mdl", b.mdl], ["vvd", b.vvd], ["dx90.vtx", b.vtx]];
    const tree = [], data = [];
    let offset = 0;
    for (const [ext, buf] of files) {
      const entry = Buffer.alloc(18); entry.writeUInt16LE(0x7fff, 6); entry.writeUInt32LE(offset, 8); entry.writeUInt32LE(buf.length, 12); entry.writeUInt16LE(0xffff, 16);
      tree.push(Buffer.from(ext + "\0models/vpk\0gman\0\0\0", "latin1").subarray(0, ext.length + 1 + 11 + 5), entry, Buffer.from("\0\0", "latin1"));
      data.push(buf); offset += buf.length;
    }
    const treeBuf = Buffer.concat([...tree, Buffer.from("\0", "latin1")]);
    const head = Buffer.alloc(12); head.writeUInt32LE(0x55aa1234, 0); head.writeUInt32LE(1, 4); head.writeUInt32LE(treeBuf.length, 8);
    put("hl2/hl2_misc_dir.vpk", Buffer.concat([head, treeBuf, ...data]));
    const fromVpk = gm.readGhostMesh(root, "models/vpk/gman.mdl");
    assert.equal(fromVpk.triangles, 1);
    assert.equal(Math.max(...fromVpk.positions), 80);
    assert.throws(() => gm.readGhostMesh(root, "models/nope.mdl"), { code: "ASSET_NOT_FOUND" });
    for (const bad of ["../x.mdl", "models/../../x.mdl", "C:/x.mdl", "models/a.vvd", ""])
      assert.throws(() => gm.readGhostMesh(root, bad), { code: "ASSET_INVALID_PATH" }, bad);
    assert.equal(gm.valueOfPicked(root, path.join(root, "ghostingmod", "custom", "zz", "models", "my", "ghost.mdl")), "models/my/ghost.mdl");
    assert.equal(gm.valueOfPicked(root, path.join(root, "ghostingmod", "models", "a.mdl")), "models/a.mdl");
    assert.throws(() => gm.valueOfPicked(root, path.join(os.tmpdir(), "a.mdl")), { code: "ASSET_OUTSIDE_MOD" });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("битые файлы модели не вешают и не раздувают главный процесс", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghost-bad-"));
  try {
    // VPK без нуля в конце дерева.
    const head = Buffer.alloc(12); head.writeUInt32LE(0x55aa1234, 0); head.writeUInt32LE(1, 4); head.writeUInt32LE(4, 8);
    const vpk = path.join(root, "broken_dir.vpk");
    fs.writeFileSync(vpk, Buffer.concat([head, Buffer.from("mdlx", "latin1")]));
    assert.throws(() => gm.readVpkDir(vpk), { code: "VPK_INVALID" });
    assert.throws(() => gm.readVpkDir(vpk), { code: "VPK_INVALID" }, "сбой запомнен");
    // VVD с fixup на миллиард вершин.
    const { mdl, vvd, vtx } = tinyModel([[0, 0, 0], [0, 10, 0], [0, 5, 72]]);
    const bad = Buffer.concat([vvd, Buffer.alloc(12)]);
    bad.writeInt32LE(1, 48); bad.writeInt32LE(vvd.length, 52);
    bad.writeInt32LE(0, vvd.length); bad.writeInt32LE(0, vvd.length + 4); bad.writeInt32LE(1e9, vvd.length + 8);
    assert.throws(() => gm.parseMesh(mdl, bad, vtx), { code: "MODEL_INVALID" });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
