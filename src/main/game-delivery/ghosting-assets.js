"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const zlib = require("zlib");

const MAX_ASSET_BYTES = 16 * 1024 * 1024;
const TYPES = {
  font: { extensions: [".ttf", ".otf"], directory: ["resource", "hlsr-fonts"] },
  spray: { extensions: [".png", ".bmp", ".tga"], directory: ["materials", "vgui", "logos", "hlsr"] },
};

function fail(code, message) {
  const error = new Error(message);
  error.code = code;
  throw error;
}

function absolutePath(value) {
  if (typeof value !== "string" || !value || value.includes("\0") || !path.isAbsolute(value))
    fail("ASSET_INVALID_PATH", "An absolute file path is required");
  return path.resolve(value);
}

function readSource(file) {
  let fd;
  try {
    const before = fs.lstatSync(file);
    if (!before.isFile() || before.isSymbolicLink())
      fail("ASSET_INVALID_FILE", "The selected asset must be a regular file");
    if (before.size > MAX_ASSET_BYTES) fail("ASSET_TOO_LARGE", "The selected asset exceeds 16 MB");
    fd = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.dev !== before.dev || stat.ino !== before.ino)
      fail("ASSET_INVALID_FILE", "The selected asset changed while opening it");
    if (stat.size > MAX_ASSET_BYTES) fail("ASSET_TOO_LARGE", "The selected asset exceeds 16 MB");
    if (!stat.size) fail("ASSET_INVALID_FILE", "The selected asset is empty");
    const bytes = Buffer.alloc(stat.size);
    let read = 0;
    while (read < bytes.length) {
      const count = fs.readSync(fd, bytes, read, bytes.length - read, read);
      if (!count) fail("ASSET_INVALID_FILE", "The selected asset changed while reading it");
      read += count;
    }
    if (fs.readSync(fd, Buffer.alloc(1), 0, 1, read))
      fail("ASSET_INVALID_FILE", "The selected asset changed while reading it");
    const after = fs.fstatSync(fd);
    if (after.size !== stat.size || after.mtimeMs !== stat.mtimeMs)
      fail("ASSET_INVALID_FILE", "The selected asset changed while reading it");
    return bytes;
  } catch (error) {
    if (error.code && error.code.startsWith("ASSET_")) throw error;
    fail("ASSET_READ_FAILED", "The selected asset could not be read");
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

function validFont(bytes, extension) {
  if (bytes.length < 28) return false;
  const signature = bytes.toString("ascii", 0, 4);
  const validSignature = extension === ".otf" ? signature === "OTTO"
    : bytes.readUInt32BE(0) === 0x00010000 || signature === "true";
  const tables = bytes.readUInt16BE(4);
  if (!validSignature || !tables || tables > 256 || 12 + tables * 16 > bytes.length) return false;
  for (let index = 0; index < tables; index++) {
    const table = 12 + index * 16;
    const start = bytes.readUInt32BE(table + 8);
    const length = bytes.readUInt32BE(table + 12);
    if (start < 12 + tables * 16 || start + length > bytes.length) return false;
  }
  return true;
}

function validPng(bytes) {
  if (bytes.length < 45 || !bytes.subarray(0, 8).equals(Buffer.from("89504e470d0a1a0a", "hex"))) return false;
  if (bytes.readUInt32BE(8) !== 13 || bytes.toString("ascii", 12, 16) !== "IHDR") return false;
  if (!bytes.readUInt32BE(16) || !bytes.readUInt32BE(20)) return false;
  const depths = { 0: [1, 2, 4, 8, 16], 2: [8, 16], 3: [1, 2, 4, 8], 4: [8, 16], 6: [8, 16] };
  if (!depths[bytes[25]] || !depths[bytes[25]].includes(bytes[24]) || bytes[26] || bytes[27] || bytes[28] !== 0)
    return false;
  let offset = 33, data = false;
  while (offset + 12 <= bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    if (offset + length + 12 > bytes.length) return false;
    if (type === "IDAT" && length) data = true;
    if (type === "IEND") return length === 0 && data;
    offset += length + 12;
  }
  return false;
}

function validBmp(bytes) {
  if (bytes.length < 54 || bytes.toString("ascii", 0, 2) !== "BM") return false;
  const headerSize = bytes.readUInt32LE(14);
  const pixels = bytes.readUInt32LE(10);
  const width = bytes.readInt32LE(18), height = Math.abs(bytes.readInt32LE(22)), depth = bytes.readUInt16LE(28);
  if (![40, 52, 56, 108, 124].includes(headerSize) || 14 + headerSize > bytes.length ||
    width <= 0 || !height || bytes.readUInt16LE(26) !== 1 || ![8, 24, 32].includes(depth) || bytes.readUInt32LE(30) !== 0)
    return false;
  const palette = depth === 8 ? (bytes.readUInt32LE(46) || 256) * 4 : 0;
  const rows = Math.ceil(width * depth / 32) * 4 * height;
  return pixels >= 14 + headerSize + palette && pixels + rows <= bytes.length;
}

function validTga(bytes) {
  if (bytes.length < 19 || !bytes.readUInt16LE(12) || !bytes.readUInt16LE(14)) return false;
  const type = bytes[2], depth = bytes[16];
  if (![2, 3, 10, 11].includes(type) || bytes[1] !== 0 || ![8, 24, 32].includes(depth)) return false;
  const pixels = bytes.readUInt16LE(12) * bytes.readUInt16LE(14), pixelBytes = depth / 8;
  let offset = 18 + bytes[0];
  if (type < 9) return offset + pixels * pixelBytes <= bytes.length;
  let decoded = 0;
  while (offset < bytes.length && decoded < pixels) {
    const packet = bytes[offset++], count = (packet & 0x7f) + 1;
    decoded += count;
    offset += (packet & 0x80 ? 1 : count) * pixelBytes;
    if (decoded > pixels || offset > bytes.length) return false;
  }
  return decoded === pixels;
}

function validateContent(bytes, extension, kind) {
  const valid = kind === "font" ? validFont(bytes, extension)
    : extension === ".png" ? validPng(bytes) : extension === ".bmp" ? validBmp(bytes) : validTga(bytes);
  if (!valid) fail("ASSET_INVALID_FILE", "The selected file does not contain a supported font or picture");
}

function safeDirectory(directory, create) {
  if (create) {
    try { fs.mkdirSync(directory); } catch (error) { if (error.code !== "EEXIST") throw error; }
  }
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink())
    fail("ASSET_UNSAFE_PATH", "The asset destination contains a link or is not a directory");
}

function prepareDestination(install, directories, create) {
  safeDirectory(install, false);
  let directory = path.join(install, "ghostingmod");
  safeDirectory(directory, false);
  for (const segment of directories) {
    directory = path.join(directory, segment);
    safeDirectory(directory, create);
  }
  return directory;
}

function importAsset(installPath, sourcePath, kind) {
  if (!Object.prototype.hasOwnProperty.call(TYPES, kind)) fail("ASSET_BAD_KIND", "Unsupported asset kind");
  const install = absolutePath(installPath), source = absolutePath(sourcePath);
  const type = TYPES[kind], extension = path.extname(source).toLowerCase();
  if (!type.extensions.includes(extension)) fail("ASSET_INVALID_TYPE", "Unsupported file extension");
  try {
    prepareDestination(install, [], false);
  } catch (error) {
    if (error.code === "ASSET_UNSAFE_PATH") throw error;
    fail("ASSET_NOT_INSTALLED", "The Ghosting Mod installation was not found");
  }
  const bytes = readSource(source);
  validateContent(bytes, extension, kind);
  const sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
  const name = sha256 + extension;
  const result = { value: type.directory.concat(name).join("/"), name: path.basename(source), bytes: bytes.length, sha256 };
  let temporary = null;
  try {
    const directory = prepareDestination(install, type.directory, true);
    const file = path.join(directory, name);
    try {
      const stat = fs.lstatSync(file);
      if (!stat.isFile() || stat.isSymbolicLink()) fail("ASSET_UNSAFE_PATH", "The asset destination is not a regular file");
      if (stat.size === bytes.length && readSource(file).equals(bytes)) return result;
    } catch (error) { if (error.code !== "ENOENT") throw error; }
    temporary = path.join(directory, `.${name}.${crypto.randomBytes(12).toString("hex")}.tmp`);
    const fd = fs.openSync(temporary, "wx", 0o644);
    try {
      fs.writeFileSync(fd, bytes);
      fs.fsyncSync(fd);
    } finally { fs.closeSync(fd); }
    prepareDestination(install, type.directory, false);
    fs.renameSync(temporary, file);
    temporary = null;
    return result;
  } catch (error) {
    if (error.code && error.code.startsWith("ASSET_")) throw error;
    fail("ASSET_WRITE_FAILED", "The selected asset could not be copied into the mod");
  } finally {
    if (temporary) {
      try { fs.unlinkSync(temporary); } catch (ignored) { /* Best-effort temporary file cleanup. */ }
    }
  }
}

// Preview reads only assets inside the installed mod, or a known font from its
// own Proton prefix. A filename entered in the UI is never an arbitrary read.
const FONT_FAMILIES = new Map([
  ["arial", "arial.ttf"], ["arial black", "ariblk.ttf"], ["impact", "impact.ttf"],
  ["times new roman", "times.ttf"], ["courier new", "cour.ttf"], ["verdana", "verdana.ttf"],
  ["georgia", "georgia.ttf"], ["trebuchet ms", "trebuc.ttf"], ["comic sans ms", "comic.ttf"],
  ["andale mono", "andalemo.ttf"], ["calibri", "carlito.ttf"], ["carlito", "carlito.ttf"], ["segoe ui", "carlito.ttf"],
]);

// Пути поиска игры из gameinfo.txt Ghosting, в том же порядке: сначала
// custom/* мода, потом сам мод, его content/*, затем custom/* и папка hl2.
// Файлы в VPK предпросмотр не читает.
function searchRoots(install) {
  const mod = path.join(install, "ghostingmod");
  const subdirs = (directory) => {
    try {
      return fs.readdirSync(directory, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => path.join(directory, entry.name))
        .sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
    } catch (error) { return []; }
  };
  return subdirs(path.join(mod, "custom")).concat([mod], subdirs(path.join(mod, "content")),
    subdirs(path.join(install, "hl2", "custom")), [path.join(install, "hl2")]);
}

// Движок под Windows и Proton не различает регистр, поэтому и путь ищется без
// учёта регистра: точное имя, если есть, иначе первое совпадение в папке.
function findInsensitive(root, segments) {
  let current = root;
  for (const segment of segments) {
    const exact = path.join(current, segment);
    if (fs.existsSync(exact)) { current = exact; continue; }
    let entries;
    try { entries = fs.readdirSync(current); } catch (error) { return null; }
    const lower = segment.toLowerCase();
    const match = entries.find((entry) => entry.toLowerCase() === lower);
    if (!match) return null;
    current = path.join(current, match);
  }
  return current;
}

// Файл берётся только из папок установки: ссылка наружу не читается.
function findInSearchPaths(install, segments) {
  let installReal;
  try { installReal = fs.realpathSync(install); } catch (error) { fail("ASSET_READ_FAILED", "The preview asset could not be read"); }
  let unsafe = false;
  for (const root of searchRoots(install)) {
    const candidate = findInsensitive(root, segments);
    if (!candidate) continue;
    let real, rootReal;
    try {
      real = fs.realpathSync(candidate);
      rootReal = fs.realpathSync(root);
    } catch (error) { continue; }
    const inside = (base) => {
      const relative = path.relative(base, real);
      return !!relative && relative !== ".." && !relative.startsWith(".." + path.sep) && !path.isAbsolute(relative);
    };
    if (!inside(rootReal) || !inside(installReal)) {
      unsafe = true;
      continue;
    }
    try { if (!fs.statSync(real).isFile()) continue; } catch (error) { continue; }
    return real;
  }
  if (unsafe) fail("ASSET_UNSAFE_PATH", "Asset is outside the mod");
  return null;
}

function readPreviewAsset(installPath, kind, value, prefix) {
  if (!["font", "spray"].includes(kind) || typeof value !== "string" || value.length > 512)
    fail("ASSET_INVALID_PATH", "Invalid preview asset");
  const systemFont = kind === "font" && prefix ? FONT_FAMILIES.get(value.trim().toLowerCase()) : undefined;
  let file;
  if (systemFont) {
    const directory = path.join(prefix, "drive_c", "windows", "Fonts");
    let name;
    // A prefix that has not been prepared yet has no Fonts directory.
    try { name = fs.readdirSync(directory).find((entry) => entry.toLowerCase() === systemFont); } catch (ignored) { /* reported below */ }
    if (!name) fail("ASSET_INVALID_FILE", "Font is not available");
    file = path.join(directory, name);
  } else {
    const relative = value.replace(/\\/g, "/");
    if (!relative || relative.startsWith("/") || relative.includes(":") || relative.split("/").includes(".."))
      fail("ASSET_INVALID_PATH", "A mod-relative asset path is required");
    file = findInSearchPaths(absolutePath(installPath), relative.split("/").filter(Boolean));
    if (!file) fail("ASSET_READ_FAILED", "The preview asset could not be read");
  }
  const extension = path.extname(file).toLowerCase();
  if (!TYPES[kind].extensions.includes(extension)) fail("ASSET_INVALID_TYPE", "Unsupported preview asset");
  const bytes = readSource(file);
  validateContent(bytes, extension, kind);
  const output = extension === ".tga" ? tgaToPng(bytes) : bytes;
  const mime = kind === "font" ? (extension === ".otf" ? "font/otf" : "font/ttf") : extension === ".bmp" ? "image/bmp" : "image/png";
  return { dataUrl: `data:${mime};base64,${output.toString("base64")}` };
}

// Browsers cannot decode TGA, so previews convert it to a plain RGBA PNG.
function tgaToPng(input) {
  const width = input.readUInt16LE(12), height = input.readUInt16LE(14), depth = input[16] / 8;
  if (width * height > 4 * 1024 * 1024) fail("ASSET_TOO_LARGE", "Preview image dimensions are too large");
  let position = 18 + input[0], pixel = 0;
  const pixels = Buffer.alloc(width * height * 4), rle = input[2] >= 9;
  const read = () => {
    if (position + depth > input.length) fail("ASSET_INVALID_FILE", "Truncated TGA image");
    const p = depth === 1 ? [input[position], input[position], input[position], 255]
      : [input[position + 2], input[position + 1], input[position], depth === 4 ? input[position + 3] : 255];
    position += depth; return p;
  };
  const writePixel = (rgba) => {
    if (pixel >= width * height) fail("ASSET_INVALID_FILE", "Invalid TGA run");
    let x = pixel % width, y = Math.floor(pixel / width);
    if (!(input[17] & 32)) y = height - y - 1;
    if (input[17] & 16) x = width - x - 1;
    const offset = (y * width + x) * 4;
    for (let channel = 0; channel < 4; channel++) pixels[offset + channel] = rgba[channel];
    pixel++;
  };
  while (pixel < width * height) {
    if (!rle) { writePixel(read()); continue; }
    if (position >= input.length) fail("ASSET_INVALID_FILE", "Truncated TGA run");
    const packet = input[position++], count = (packet & 127) + 1;
    if (packet & 128) { const rgba = read(); for (let i = 0; i < count; i++) writePixel(rgba); }
    else for (let i = 0; i < count; i++) writePixel(read());
  }
  const chunk = (name, data) => {
    const body = Buffer.concat([Buffer.from(name), data]);
    const length = Buffer.alloc(4), checksum = Buffer.alloc(4);
    length.writeUInt32BE(data.length); checksum.writeUInt32BE(zlib.crc32(body));
    return Buffer.concat([length, body, checksum]);
  };
  const header = Buffer.alloc(13); header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 6;
  const rows = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) pixels.copy(rows, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([Buffer.from("89504e470d0a1a0a", "hex"), chunk("IHDR", header), chunk("IDAT", zlib.deflateSync(rows)), chunk("IEND", Buffer.alloc(0))]);
}

module.exports = { importAsset, readPreviewAsset, MAX_ASSET_BYTES };
