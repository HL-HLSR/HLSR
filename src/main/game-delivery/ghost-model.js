"use strict";

// Геометрия модели призрака Ghosting (gh_model) для предпросмотра в «Персонализации».
// Модель Source — три файла: .mdl (части тела, модели, сетки), .vvd (вершины в позе
// привязки, координаты модели), .dx90.vtx (треугольники по вершинам сеток). Текстуры не
// нужны: игра красит призрака цветом (kRenderTransColor), предпросмотр делает так же.
//
// Где ищется файл — как его найдёт игра: ghostingmod/custom/* (по алфавиту), сам мод,
// затем VPK (*_dir.vpk мода и hl2). Читаются только пути models/….mdl внутри этих мест.

const fs = require("fs");
const path = require("path");

const MAX_FILE_BYTES = 32 * 1024 * 1024;
// Дерево каталога VPK (у hl2_misc_dir.vpk — меньше мегабайта); данные архива не читаются.
const MAX_VPK_TREE_BYTES = 64 * 1024 * 1024;
const MAX_TRIANGLES = 60000;
const VVD_VERTEX_SIZE = 48; // mstudiovertex_t
const MODEL_VALUE = /^models\/[A-Za-z0-9_\-./ ]+\.mdl$/i;

class GhostModelError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}
const fail = (code, message) => { throw new GhostModelError(code, message); };

// Значение gh_model из настроек: путь внутри мода, без «..» и абсолютных путей.
function normalizeValue(value) {
  const text = typeof value === "string" ? value.trim().replace(/\\/g, "/") : "";
  if (!text || text.length > 200 || !MODEL_VALUE.test(text) || text.split("/").some((part) => part === ".." || part === "."))
    fail("ASSET_INVALID_PATH", "Invalid model path");
  return text;
}

// ---- VPK (v1 и v2): каталог файлов один раз на файл, данные — по запросу.
const vpkCache = new Map();
// Читаются только заголовок и дерево: у одиночного VPK (ghostingmod_pak.vpk) за деревом —
// все данные архива. Сломанный VPK запоминается тоже, чтобы не разбирать его на каждый запрос.
function readVpkDir(file) {
  const stat = fs.statSync(file);
  const cached = vpkCache.get(file);
  if (cached && cached.mtimeMs === stat.mtimeMs) {
    if (cached.error) throw cached.error;
    return cached.entries;
  }
  try {
    const entries = parseVpkDir(file);
    vpkCache.set(file, { mtimeMs: stat.mtimeMs, entries });
    return entries;
  } catch (error) {
    vpkCache.set(file, { mtimeMs: stat.mtimeMs, error });
    throw error;
  }
}
function parseVpkDir(file) {
  const fd = fs.openSync(file, "r");
  let buf, header, treeSize;
  try {
    const head = Buffer.alloc(28);
    fs.readSync(fd, head, 0, 28, 0);
    if (head.readUInt32LE(0) !== 0x55aa1234) fail("VPK_INVALID", "Not a VPK");
    treeSize = head.readUInt32LE(8);
    header = head.readUInt32LE(4) === 2 ? 28 : 12;
    if (treeSize > MAX_VPK_TREE_BYTES) fail("VPK_INVALID", "VPK tree too large");
    buf = Buffer.alloc(header + treeSize);
    fs.readSync(fd, buf, 0, buf.length, 0);
  } finally {
    fs.closeSync(fd);
  }
  let p = header;
  const str = () => {
    const end = buf.indexOf(0, p);
    if (end < 0) fail("VPK_INVALID", "Broken VPK tree"); // без нуля в конце разбор пошёл бы сначала
    const s = buf.toString("latin1", p, end);
    p = end + 1;
    return s;
  };
  const entries = new Map();
  for (;;) {
    const ext = str(); if (!ext) break;
    for (;;) {
      const dir = str(); if (!dir) break;
      for (;;) {
        const name = str(); if (!name) break;
        const preload = buf.readUInt16LE(p + 4), archive = buf.readUInt16LE(p + 6);
        const offset = buf.readUInt32LE(p + 8), length = buf.readUInt32LE(p + 12);
        p += 18;
        if (p + preload > buf.length) fail("VPK_INVALID", "Broken VPK tree");
        const full = (dir === " " ? "" : dir + "/") + name + "." + ext;
        entries.set(full.toLowerCase(), { file, archive, offset, length, preloadAt: p, preload, dataAt: header + treeSize });
        p += preload;
      }
    }
  }
  return entries;
}
function readVpkEntry(entry) {
  if (entry.preload + entry.length > MAX_FILE_BYTES) fail("ASSET_TOO_LARGE", "Model file too large");
  const parts = [];
  if (entry.preload) {
    const fd = fs.openSync(entry.file, "r");
    try { const b = Buffer.alloc(entry.preload); fs.readSync(fd, b, 0, entry.preload, entry.preloadAt); parts.push(b); } finally { fs.closeSync(fd); }
  }
  if (entry.length) {
    const archive = entry.archive === 0x7fff
      ? entry.file
      : entry.file.replace(/_dir\.vpk$/i, `_${String(entry.archive).padStart(3, "0")}.vpk`);
    const at = entry.archive === 0x7fff ? entry.dataAt + entry.offset : entry.offset;
    const fd = fs.openSync(archive, "r");
    try { const b = Buffer.alloc(entry.length); fs.readSync(fd, b, 0, entry.length, at); parts.push(b); } finally { fs.closeSync(fd); }
  }
  return Buffer.concat(parts);
}

// Места, где игра ищет файл модели, по порядку SearchPaths из ghostingmod/gameinfo.txt:
// custom/* мода и hl2 (папки и VPK по алфавиту), файлы мода, его VPK, content/*,
// mapbase_shared, VPK hl2 по списку, файлы hl2. «name.vpk» на диске — name_dir.vpk.
const HL2_VPKS = ["hl2_english", "hl2_pak", "hl2_textures", "hl2_sound_vo_english", "hl2_sound_misc", "hl2_misc"];
function vpkOf(base) {
  for (const file of [base + "_dir.vpk", base + ".vpk"]) if (fs.existsSync(file)) return { vpk: file };
  return null;
}
function each(dir) {
  let names;
  try { names = fs.readdirSync(dir).sort(); } catch (error) { return []; }
  return names.map((name) => path.join(dir, name)).flatMap((full) => {
    // Один битый элемент (ссылка в никуда, занятый файл) не прячет остальные.
    let isDir = false;
    try { isDir = fs.statSync(full).isDirectory(); } catch (error) { return []; }
    if (isDir) return [{ dir: full }];
    // Архив из нескольких частей монтируется по _dir.vpk; части _000… не отдельно.
    return /\.vpk$/i.test(full) && !/_\d{3}\.vpk$/i.test(full) ? [{ vpk: full }] : [];
  });
}
// Настоящий путь (ссылки раскрыты); null — файла нет.
function real(file) {
  try { return fs.realpathSync(file); } catch (error) { return null; }
}
const inside = (child, parent) => !!child && !!parent && child.toLowerCase().startsWith(parent.toLowerCase() + path.sep);
function sources(installPath) {
  const mod = path.join(installPath, "ghostingmod"), hl2 = path.join(installPath, "hl2"), shared = path.join(installPath, "mapbase_shared");
  return [].concat(
    each(path.join(mod, "custom")), each(path.join(hl2, "custom")),
    [{ dir: mod }], [vpkOf(path.join(mod, "ghostingmod_pak"))], each(path.join(mod, "content")),
    [{ dir: path.join(shared, "shared_misc") }], [vpkOf(path.join(shared, "shared_content"))],
    HL2_VPKS.map((name) => vpkOf(path.join(hl2, name))), [{ dir: hl2 }]
  ).filter(Boolean);
}
function readGameFile(installPath, relative) {
  const root = real(installPath);
  for (const source of sources(installPath)) {
    if (source.dir) {
      const file = path.join(source.dir, ...relative.split("/"));
      // Ссылка внутри мода на файл снаружи игры не читается.
      if (!inside(real(file), root)) continue;
      let stat;
      try { stat = fs.statSync(file); } catch (error) { continue; }
      if (!stat.isFile()) continue;
      if (stat.size > MAX_FILE_BYTES) fail("ASSET_TOO_LARGE", "Model file too large");
      return fs.readFileSync(file);
    }
    let entries;
    if (!inside(real(source.vpk), root)) continue;
    try { entries = readVpkDir(source.vpk); } catch (error) { continue; }
    const entry = entries.get(relative.toLowerCase());
    if (entry) return readVpkEntry(entry);
  }
  return null;
}

// ---- Разбор: треугольники LOD 0, первая модель каждой части тела.
function vvdVertices(vvd) {
  if (vvd.toString("latin1", 0, 4) !== "IDSV") fail("MODEL_INVALID", "Not a VVD");
  const numLods = vvd.readInt32LE(12), lod0 = vvd.readInt32LE(16);
  const numFixups = vvd.readInt32LE(48), fixupStart = vvd.readInt32LE(52), dataStart = vvd.readInt32LE(56);
  if (numLods < 1 || lod0 < 0 || dataStart + lod0 * VVD_VERTEX_SIZE > vvd.length) fail("MODEL_INVALID", "Bad VVD");
  // Вершин не больше, чем помещается в сам файл: счётчики из файла не раздувают память.
  const stored = Math.floor((vvd.length - dataStart) / VVD_VERTEX_SIZE);
  if (lod0 > stored) fail("MODEL_INVALID", "Bad VVD");
  const at = (i) => { const o = dataStart + i * VVD_VERTEX_SIZE + 16; return [vvd.readFloatLE(o), vvd.readFloatLE(o + 4), vvd.readFloatLE(o + 8)]; };
  if (!numFixups) return Array.from({ length: lod0 }, (_, i) => at(i));
  if (numFixups < 0 || fixupStart < 0 || fixupStart + numFixups * 12 > vvd.length) fail("MODEL_INVALID", "Bad VVD fixups");
  // Вершины LOD 0 собираются из таблицы fixup (так же делает движок); итог — не больше lod0.
  const out = [];
  for (let f = 0; f < numFixups; f += 1) {
    const o = fixupStart + f * 12, from = vvd.readInt32LE(o + 4), count = vvd.readInt32LE(o + 8);
    if (from < 0 || count < 0 || from + count > stored || out.length + count > lod0) fail("MODEL_INVALID", "Bad VVD fixups");
    for (let i = 0; i < count; i += 1) out.push(at(from + i));
  }
  return out;
}

function parseMesh(mdl, vvd, vtx) {
  if (mdl.toString("latin1", 0, 4) !== "IDST") fail("MODEL_INVALID", "Not an MDL");
  const version = mdl.readInt32LE(4);
  if (version < 44 || version > 49) fail("MODEL_UNSUPPORTED", `MDL version ${version}`);
  if (vtx.readInt32LE(0) !== 7) fail("MODEL_UNSUPPORTED", "VTX version");
  const verts = vvdVertices(vvd);
  const numBodyParts = mdl.readInt32LE(232), bodyPartIndex = mdl.readInt32LE(236);
  const vtxBodyParts = vtx.readInt32LE(28), vtxBodyPartAt = vtx.readInt32LE(32);
  const positions = [];
  let triangles = 0;
  for (let b = 0; b < Math.min(numBodyParts, vtxBodyParts); b += 1) {
    const bp = bodyPartIndex + b * 16;
    if (mdl.readInt32LE(bp + 4) < 1) continue;
    const model = bp + mdl.readInt32LE(bp + 12); // первая модель части тела
    const numMeshes = mdl.readInt32LE(model + 72), meshIndex = mdl.readInt32LE(model + 76);
    const vertexBase = Math.floor(mdl.readInt32LE(model + 84) / VVD_VERTEX_SIZE);
    const vbp = vtxBodyPartAt + b * 8;
    if (vtx.readInt32LE(vbp) < 1) continue;
    const vmodel = vbp + vtx.readInt32LE(vbp + 4);
    if (vtx.readInt32LE(vmodel) < 1) continue;
    const lod = vmodel + vtx.readInt32LE(vmodel + 4);
    const vtxMeshes = vtx.readInt32LE(lod), meshAt = lod + vtx.readInt32LE(lod + 4);
    for (let m = 0; m < Math.min(numMeshes, vtxMeshes); m += 1) {
      const mesh = model + meshIndex + m * 116;
      const vertexOffset = mdl.readInt32LE(mesh + 12);
      const vm = meshAt + m * 9;
      const groups = vtx.readInt32LE(vm), groupAt = vm + vtx.readInt32LE(vm + 4);
      for (let g = 0; g < groups; g += 1) {
        const sg = groupAt + g * 25;
        const numVerts = vtx.readInt32LE(sg), vertAt = sg + vtx.readInt32LE(sg + 4);
        const indexAt = sg + vtx.readInt32LE(sg + 12);
        const numStrips = vtx.readInt32LE(sg + 16), stripAt = sg + vtx.readInt32LE(sg + 20);
        const vertexOf = (i) => {
          if (i < 0 || i >= numVerts) return null;
          const id = vtx.readUInt16LE(vertAt + i * 9 + 4);
          return verts[vertexBase + vertexOffset + id] || null;
        };
        const push = (a, b2, c) => {
          const va = vertexOf(a), vb = vertexOf(b2), vc = vertexOf(c);
          if (!va || !vb || !vc || triangles >= MAX_TRIANGLES) return;
          positions.push(...va, ...vb, ...vc);
          triangles += 1;
        };
        for (let s = 0; s < numStrips; s += 1) {
          const st = stripAt + s * 27;
          const count = vtx.readInt32LE(st), first = vtx.readInt32LE(st + 4), flags = vtx.readUInt8(st + 18);
          const index = (k) => vtx.readUInt16LE(indexAt + (first + k) * 2);
          if (flags & 2) {
            for (let k = 0; k + 2 < count; k += 1) {
              if (k % 2) push(index(k + 1), index(k), index(k + 2)); else push(index(k), index(k + 1), index(k + 2));
            }
          } else {
            for (let k = 0; k + 2 < count; k += 3) push(index(k), index(k + 1), index(k + 2));
          }
        }
      }
    }
  }
  if (!triangles) fail("MODEL_EMPTY", "Model has no triangles");
  return { positions, triangles };
}

// gh_model -> { triangles, positions } (x, y, z по три вершины на треугольник, координаты модели).
function readGhostMesh(installPath, value) {
  const relative = normalizeValue(value);
  const stem = relative.slice(0, -4);
  const mdl = readGameFile(installPath, relative);
  if (!mdl) fail("ASSET_NOT_FOUND", "Model not found");
  const vvd = readGameFile(installPath, stem + ".vvd");
  const vtx = readGameFile(installPath, stem + ".dx90.vtx") || readGameFile(installPath, stem + ".dx80.vtx") || readGameFile(installPath, stem + ".sw.vtx");
  if (!vvd || !vtx) fail("ASSET_NOT_FOUND", "Model files incomplete");
  const mesh = parseMesh(mdl, vvd, vtx);
  // Два знака после точки: размер ответа меньше, точность — сотая единицы.
  return { triangles: mesh.triangles, positions: mesh.positions.map((v) => Math.round(v * 100) / 100) };
}

// Выбранный файл -> значение gh_model: только внутри мода (ghostingmod/models/… или
// ghostingmod/custom/<пак>/models/…), иначе игра его не увидит.
function valueOfPicked(installPath, file) {
  // Ссылки раскрываются: файл, который лишь лежит ссылкой в моде, игра всё равно читает снаружи.
  const mod = real(path.join(installPath, "ghostingmod")) || path.resolve(installPath, "ghostingmod");
  const full = real(file) || path.resolve(file);
  if (!inside(full, mod)) fail("ASSET_OUTSIDE_MOD", "Model is outside the mod");
  let relative = path.relative(mod, full).split(path.sep).join("/");
  const custom = /^custom\/[^/]+\/(.+)$/i.exec(relative);
  if (custom) relative = custom[1];
  return normalizeValue(relative);
}

module.exports = { readGhostMesh, valueOfPicked, normalizeValue, parseMesh, readVpkDir, GhostModelError };
