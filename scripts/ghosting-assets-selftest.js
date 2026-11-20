"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const os = require("os");
const path = require("path");
const zlib = require("zlib");
const { importAsset, readPreviewAsset, MAX_ASSET_BYTES } = require("../src/main/game-delivery/ghosting-assets");
const { NO_SYMLINKS } = require("./lib/selftest-caps");

const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aGhsAAAAASUVORK5CYII=", "base64");

function font(otf) {
  const bytes = Buffer.alloc(32);
  if (otf) bytes.write("OTTO");
  else bytes.writeUInt32BE(0x00010000, 0);
  bytes.writeUInt16BE(1, 4);
  bytes.write("head", 12);
  bytes.writeUInt32BE(28, 20);
  bytes.writeUInt32BE(4, 24);
  return bytes;
}

function bmp() {
  const bytes = Buffer.alloc(58);
  bytes.write("BM"); bytes.writeUInt32LE(58, 2); bytes.writeUInt32LE(54, 10);
  bytes.writeUInt32LE(40, 14); bytes.writeInt32LE(1, 18); bytes.writeInt32LE(1, 22);
  bytes.writeUInt16LE(1, 26); bytes.writeUInt16LE(24, 28);
  return bytes;
}

function tga() {
  const bytes = Buffer.alloc(21);
  bytes[2] = 2; bytes.writeUInt16LE(1, 12); bytes.writeUInt16LE(1, 14); bytes[16] = 24;
  return bytes;
}

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghosting-assets-"));
  const install = path.join(root, "game with spaces"), sources = path.join(root, "sources");
  fs.mkdirSync(path.join(install, "ghostingmod"), { recursive: true });
  fs.mkdirSync(sources);
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  function file(name, bytes) {
    const result = path.join(sources, name);
    fs.writeFileSync(result, bytes);
    return result;
  }
  return { root, install, sources, file };
}

test("imports each supported format to a stable mod-relative path and preserves the source", (t) => {
  const f = fixture(t);
  const assets = [["font", "my font.TTF", font(false)], ["font", "font.otf", font(true)],
    ["spray", "my picture.png", PNG], ["spray", "picture.bmp", bmp()], ["spray", "picture.tga", tga()]];
  for (const [kind, name, bytes] of assets) {
    const source = f.file(name, bytes), before = fs.statSync(source);
    const result = importAsset(f.install, source, kind);
    assert.match(result.value, kind === "font" ? /^resource\/hlsr-fonts\/[a-f0-9]{64}\.(ttf|otf)$/
      : /^materials\/vgui\/logos\/hlsr\/[a-f0-9]{64}\.(png|bmp|tga)$/);
    assert.equal(result.name, name);
    assert.equal(result.bytes, bytes.length);
    assert.equal(result.sha256.length, 64);
    assert.deepEqual(fs.readFileSync(source), bytes);
    assert.equal(fs.statSync(source).mtimeMs, before.mtimeMs);
    assert.deepEqual(fs.readFileSync(path.join(f.install, "ghostingmod", result.value)), bytes);
    assert.deepEqual(importAsset(f.install, source, kind), result);
    const managedFile = path.join(f.install, "ghostingmod", result.value), managedBefore = fs.statSync(managedFile);
    assert.equal(importAsset(f.install, managedFile, kind).value, result.value);
    assert.equal(fs.statSync(managedFile).mtimeMs, managedBefore.mtimeMs);
    assert.equal(fs.statSync(managedFile).ino, managedBefore.ino);
  }
});

test("rejects unsupported kinds, relative paths, missing installations, and wrong extensions", (t) => {
  const f = fixture(t), source = f.file("picture.png", PNG);
  assert.throws(() => importAsset(f.install, source, "__proto__"), { code: "ASSET_BAD_KIND" });
  assert.throws(() => importAsset(f.install, "picture.png", "spray"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => importAsset("relative/game", source, "spray"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => importAsset(path.join(f.root, "missing"), source, "spray"), { code: "ASSET_NOT_INSTALLED" });
  assert.throws(() => importAsset(f.install, source, "font"), { code: "ASSET_INVALID_TYPE" });
  assert.throws(() => importAsset(f.install, path.join(f.sources, "absent.png"), "spray"), { code: "ASSET_READ_FAILED" });
});

test("rejects empty, spoofed, truncated, and oversized source files", (t) => {
  const f = fixture(t);
  for (const [name, bytes, kind] of [["empty.png", Buffer.alloc(0), "spray"],
    ["spoof.png", Buffer.from("not a picture"), "spray"], ["broken.ttf", Buffer.from("not a font"), "font"],
    ["truncated.png", PNG.subarray(0, 33), "spray"], ["bad.bmp", Buffer.alloc(54), "spray"],
    ["bad.tga", Buffer.alloc(30), "spray"]]) {
    assert.throws(() => importAsset(f.install, f.file(name, bytes), kind), { code: "ASSET_INVALID_FILE" });
  }
  const brokenFont = font(false); brokenFont.writeUInt32BE(9999, 24);
  assert.throws(() => importAsset(f.install, f.file("tables.ttf", brokenFont), "font"), { code: "ASSET_INVALID_FILE" });
  const huge = f.file("huge.png", PNG); fs.truncateSync(huge, MAX_ASSET_BYTES + 1);
  assert.throws(() => importAsset(f.install, huge, "spray"), { code: "ASSET_TOO_LARGE" });
  assert.equal(fs.existsSync(path.join(f.install, "ghostingmod", "materials")), false);
});

test("rejects directories and source symlinks", (t) => {
  const f = fixture(t), directory = path.join(f.sources, "directory.png");
  fs.mkdirSync(directory);
  assert.throws(() => importAsset(f.install, directory, "spray"), { code: "ASSET_INVALID_FILE" });
  const source = f.file("source.png", PNG), linked = path.join(f.sources, "linked.png");
  try { fs.symlinkSync(source, linked); } catch (error) {
    if (process.platform === "win32" && error.code === "EPERM") return;
    throw error;
  }
  assert.throws(() => importAsset(f.install, linked, "spray"), { code: "ASSET_INVALID_FILE" });
});

test("rejects image encodings the game cannot decode", (t) => {
  const f = fixture(t);
  const interlaced = Buffer.from(PNG); interlaced[28] = 1;
  const compressedBmp = bmp(); compressedBmp.writeUInt32LE(1, 30);
  const bmp16 = bmp(); bmp16.writeUInt16LE(16, 28);
  const mappedTga = tga(); mappedTga[1] = 1; mappedTga[2] = 1;
  const tga16 = tga(); tga16[16] = 16;
  const truncatedTga = tga().subarray(0, 19);
  const badRleTga = tga(); badRleTga[2] = 10; badRleTga[18] = 0xff;
  for (const [name, bytes] of [["interlaced.png", interlaced], ["compressed.bmp", compressedBmp],
    ["16bit.bmp", bmp16], ["mapped.tga", mappedTga], ["16bit.tga", tga16],
    ["truncated.tga", truncatedTga], ["rle.tga", badRleTga]]) {
    assert.throws(() => importAsset(f.install, f.file(name, bytes), "spray"), { code: "ASSET_INVALID_FILE" });
  }
});

test("refuses symlinked mod and destination directories without writing outside the install", (t) => {
  const f = fixture(t), source = f.file("source.png", PNG), outside = path.join(f.root, "outside");
  fs.mkdirSync(outside);
  const mod = path.join(f.install, "ghostingmod");
  fs.rmdirSync(mod);
  fs.symlinkSync(outside, mod, "junction");
  assert.throws(() => importAsset(f.install, source, "spray"), { code: "ASSET_UNSAFE_PATH" });
  fs.unlinkSync(mod); fs.mkdirSync(mod);
  fs.symlinkSync(outside, path.join(mod, "materials"), "junction");
  assert.throws(() => importAsset(f.install, source, "spray"), { code: "ASSET_UNSAFE_PATH" });
  assert.deepEqual(fs.readdirSync(outside), []);
});

test("refuses a linked final destination and leaves its target intact", (t) => {
  const f = fixture(t), source = f.file("source.png", PNG);
  const result = importAsset(f.install, source, "spray"), destination = path.join(f.install, "ghostingmod", result.value);
  const outside = f.file("unrelated.txt", "keep me");
  fs.unlinkSync(destination);
  try { fs.symlinkSync(outside, destination); } catch (error) {
    if (process.platform === "win32" && error.code === "EPERM") return;
    throw error;
  }
  assert.throws(() => importAsset(f.install, source, "spray"), { code: "ASSET_UNSAFE_PATH" });
  assert.equal(fs.readFileSync(outside, "utf8"), "keep me");
  assert.deepEqual(fs.readFileSync(source), PNG);
  assert.equal(fs.readdirSync(path.dirname(destination)).some((name) => name.endsWith(".tmp")), false);
});

test("overlay preview reads imported assets and rejects escapes or other file types", (t) => {
  const f = fixture(t);
  const asset = importAsset(f.install, f.file("preview.png", PNG), "spray");
  assert.equal(readPreviewAsset(f.install, "spray", asset.value).dataUrl, "data:image/png;base64," + PNG.toString("base64"));
  assert.throws(() => readPreviewAsset(f.install, "spray", "../sources/preview.png"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => readPreviewAsset(f.install, "font", asset.value), { code: "ASSET_INVALID_TYPE" });
  const outside = f.file("outside.png", PNG), link = path.join(f.install, "ghostingmod", "outside.png");
  if (NO_SYMLINKS) return t.skip(NO_SYMLINKS);
  fs.symlinkSync(outside, link);
  assert.throws(() => readPreviewAsset(f.install, "spray", "outside.png"), { code: "ASSET_UNSAFE_PATH" });
});

test("overlay preview finds files like the game: any case, custom/* and hl2", (t) => {
  const f = fixture(t);
  const put = (relative, bytes) => {
    const file = path.join(f.install, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, bytes);
  };
  const url = "data:image/png;base64," + PNG.toString("base64");
  // Под Proton игра не различает регистр, предпросмотр тоже.
  put("ghostingmod/Materials/VGUI/Logos/MySpray.PNG", PNG);
  assert.equal(readPreviewAsset(f.install, "spray", "materials/vgui/logos/myspray.png").dataUrl, url);
  assert.equal(readPreviewAsset(f.install, "spray", "materials\\vgui\\logos\\MYSPRAY.png").dataUrl, url);
  // Пути поиска из gameinfo.txt: custom/* мода и папка hl2.
  put("ghostingmod/custom/myhud/resource/fonts/hud.ttf", font(false));
  assert.equal(readPreviewAsset(f.install, "font", "resource/fonts/hud.ttf").dataUrl, "data:font/ttf;base64," + font(false).toString("base64"));
  put("hl2/materials/shared.png", PNG);
  assert.equal(readPreviewAsset(f.install, "spray", "materials/shared.png").dataUrl, url);
  assert.throws(() => readPreviewAsset(f.install, "spray", "materials/missing.png"), { code: "ASSET_READ_FAILED" });
});

test("overlay TGA conversion retains RGB pixels and writes valid PNG chunks", (t) => {
  const f = fixture(t), picture = tga();
  picture[18] = 15; picture[19] = 80; picture[20] = 210;
  const asset = importAsset(f.install, f.file("preview.tga", picture), "spray");
  const png = Buffer.from(readPreviewAsset(f.install, "spray", asset.value).dataUrl.split(",")[1], "base64");
  assert.equal(png.readUInt32BE(16), 1); assert.equal(png.readUInt32BE(20), 1);
  let offset = 8, decoded;
  const types = [];
  while (offset < png.length) {
    const length = png.readUInt32BE(offset), type = png.toString("ascii", offset + 4, offset + 8);
    // A browser rejects a PNG whose chunk checksum does not match.
    assert.equal(png.readUInt32BE(offset + 8 + length), zlib.crc32(png.subarray(offset + 4, offset + 8 + length)), type);
    if (type === "IDAT") decoded = zlib.inflateSync(png.subarray(offset + 8, offset + 8 + length));
    types.push(type);
    offset += length + 12;
  }
  assert.deepEqual(types, ["IHDR", "IDAT", "IEND"]);
  assert.deepEqual([...decoded], [0, 210, 80, 15, 255]);
});

test("overlay font family lookup stays in the game prefix", (t) => {
  const f = fixture(t);
  const prefix = path.join(f.root, "pfx"), dir = path.join(prefix, "drive_c/windows/Fonts");
  // A prefix that has not been prepared has no Fonts directory yet.
  assert.throws(() => readPreviewAsset(f.install, "font", "Impact", prefix), { code: "ASSET_INVALID_FILE" });
  fs.mkdirSync(dir, { recursive: true }); fs.writeFileSync(path.join(dir, "IMPACT.TTF"), font(false));
  assert.equal(readPreviewAsset(f.install, "font", "Impact", prefix).dataUrl, "data:font/ttf;base64," + font(false).toString("base64"));
  assert.throws(() => readPreviewAsset(f.install, "font", "Verdana", prefix), { code: "ASSET_INVALID_FILE" });
  // Without a prefix a family name is just a missing mod-relative file.
  assert.throws(() => readPreviewAsset(f.install, "font", "Impact"), { code: "ASSET_READ_FAILED" });
  // Names inherited from Object.prototype are not font families.
  ["constructor", "__proto__", "toString", "hasOwnProperty"].forEach((name) =>
    assert.throws(() => readPreviewAsset(f.install, "font", name, prefix), { code: "ASSET_READ_FAILED" }, name));
});

test("overlay preview reports unavailable assets with stable error codes", (t) => {
  const f = fixture(t);
  assert.throws(() => readPreviewAsset(f.install, "spray", "materials/vgui/logos/hlsr/absent.png"), { code: "ASSET_READ_FAILED" });
  assert.throws(() => readPreviewAsset(path.join(f.root, "missing"), "spray", "materials/x.png"), { code: "ASSET_READ_FAILED" });
  assert.throws(() => readPreviewAsset(f.install, "executable", "x.png"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => readPreviewAsset("relative/game", "spray", "x.png"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => readPreviewAsset(f.install, "spray", "C:/Windows/x.png"), { code: "ASSET_INVALID_PATH" });
  assert.throws(() => readPreviewAsset(f.install, "spray", "x".repeat(513)), { code: "ASSET_INVALID_PATH" });
});

// Разрешение для предпросмотра HUD: настройки лаунчера важнее сохранённого
// игрой, под Proton оно читается из user.reg префикса, в Windows — reg query.
test("разрешение Ghosting для предпросмотра HUD", async () => {
  const sourceScreen = require("../src/main/game-delivery/source-screen");
  const prefix = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-screen-"));
  fs.writeFileSync(path.join(prefix, "user.reg"), [
    "WINE REGISTRY Version 2",
    "",
    "[Software\\\\Valve\\\\Source\\\\hl2\\\\Settings] 1791620700",
    "\"ScreenWidth\"=dword:00000780",
    "\"ScreenHeight\"=dword:00000438",
    "",
    "[Software\\\\Valve\\\\Source\\\\ghostingmod\\\\Settings] 1791620751",
    "#time=1dc39a9c2b4c1e0",
    "\"ScreenHeight\"=dword:00000384",
    "\"ScreenWidth\"=dword:000005a0",
    "",
  ].join("\n"));
  try {
    assert.deepEqual(await sourceScreen.screenOf({ settings: { width: 0, height: 0 }, mod: "ghostingmod", platform: "linux", prefix }), { width: 1440, height: 900, source: "game" });
    assert.deepEqual(await sourceScreen.screenOf({ settings: { width: 2560, height: 1080 }, mod: "ghostingmod", platform: "linux", prefix }), { width: 2560, height: 1080, source: "launcher" });
    assert.equal(await sourceScreen.screenOf({ settings: {}, mod: "cstrike", platform: "linux", prefix }), null);
    assert.equal(await sourceScreen.screenOf({ settings: {}, mod: "..\\x", platform: "linux", prefix }), null);
    assert.equal(await sourceScreen.screenOf({ settings: {}, mod: "ghostingmod", platform: "linux", prefix: path.join(prefix, "none") }), null);
    const execFile = (file, args, options, done) => {
      assert.deepEqual(args, ["query", "HKCU\\Software\\Valve\\Source\\ghostingmod\\Settings"]);
      done(null, "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Source\\ghostingmod\\Settings\r\n    ScreenWidth    REG_DWORD    0xa00\r\n    ScreenHeight    REG_DWORD    0x5a0\r\n");
    };
    assert.deepEqual(await sourceScreen.screenOf({ settings: null, mod: "ghostingmod", platform: "win32", execFile }), { width: 2560, height: 1440, source: "game" });
    const missing = (file, args, options, done) => done(new Error("not found"), "");
    assert.equal(await sourceScreen.screenOf({ settings: null, mod: "ghostingmod", platform: "win32", execFile: missing }), null);
    // Мусор в реестре не даёт кадр нулевой или гигантской ширины.
    assert.equal(sourceScreen.fromRegQuery("    ScreenWidth    REG_DWORD    0x0\r\n    ScreenHeight    REG_DWORD    0x5a0"), null);
  } finally {
    fs.rmSync(prefix, { recursive: true, force: true });
  }
});
