"use strict";

// Редактор cfg-файлов: список, чтение и запись только внутри папки игры.

const test = require("node:test");
const assert = require("node:assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const cfgFiles = require("../src/main/game-delivery/cfg-files");

function makeGame() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-files-"));
  const write = (rel, data) => {
    const full = path.join(root, rel);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, data);
  };
  write("valve/config.cfg", "bind w +forward\r\n");
  write("valve/autoexec.cfg", "// мой автоэкзек\n");
  write("valve/scripts/jump.cfg", Buffer.from([0x2f, 0x2f, 0x20, 0xcf, 0xf0, 0xe8, 0x0a])); // cp1251
  write("valve/maps/skip.cfg", "x");
  write("valve/readme.txt", "x");
  return root;
}

test("cfg-files: список — все cfg сборки, без папок ресурсов и не-cfg", () => {
  const root = makeGame();
  const { files } = cfgFiles.list(root);
  assert.deepStrictEqual(files.map((f) => f.path), ["valve/autoexec.cfg", "valve/config.cfg", "valve/scripts/jump.cfg"]);
});

test("cfg-files: чтение и запись сохраняют CRLF и байты не-UTF-8", () => {
  const root = makeGame();
  const config = cfgFiles.read(root, "valve/config.cfg");
  assert.equal(config.content, "bind w +forward\n");
  assert.equal(config.eol, "crlf");
  cfgFiles.write(root, "valve/config.cfg", config.content + "bind s +back\n", { eol: config.eol, encoding: config.encoding, expectedMtime: config.mtime });
  assert.equal(fs.readFileSync(path.join(root, "valve/config.cfg"), "utf8"), "bind w +forward\r\nbind s +back\r\n");

  const legacy = cfgFiles.read(root, "valve/scripts/jump.cfg");
  assert.equal(legacy.encoding, "latin1");
  cfgFiles.write(root, "valve/scripts/jump.cfg", legacy.content + "alias j +jump\n", { encoding: legacy.encoding, eol: legacy.eol });
  const bytes = fs.readFileSync(path.join(root, "valve/scripts/jump.cfg"));
  assert.deepStrictEqual([...bytes.subarray(0, 7)], [0x2f, 0x2f, 0x20, 0xcf, 0xf0, 0xe8, 0x0a], "старые байты на месте");
  assert.throws(() => cfgFiles.write(root, "valve/scripts/jump.cfg", "// кириллица", { encoding: "latin1" }), { code: "ENCODING" });
});

test("cfg-files: файл, изменённый после открытия, не затирается", () => {
  const root = makeGame();
  const opened = cfgFiles.read(root, "valve/autoexec.cfg");
  const file = path.join(root, "valve/autoexec.cfg");
  fs.writeFileSync(file, "// игра переписала\n");
  fs.utimesSync(file, new Date(), new Date(Date.now() + 5000));
  assert.throws(() => cfgFiles.write(root, "valve/autoexec.cfg", "x", { expectedMtime: opened.mtime }), { code: "CHANGED" });
});

test("cfg-files: путь вне папки игры, не cfg и симлинк наружу отклоняются", () => {
  const root = makeGame();
  for (const bad of ["../escape.cfg", "valve/../../x.cfg", "valve/readme.txt", "/etc/passwd", ""])
    assert.throws(() => cfgFiles.read(root, bad), { code: "PATH_INVALID" }, bad);
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-outside-"));
  fs.writeFileSync(path.join(outside, "secret.cfg"), "x");
  try {
    fs.symlinkSync(path.join(outside, "secret.cfg"), path.join(root, "valve/link.cfg"));
  } catch (error) {
    return; // Windows без прав на симлинки
  }
  assert.throws(() => cfgFiles.read(root, "valve/link.cfg"), { code: "PATH_INVALID" });
  assert.throws(() => cfgFiles.write(root, "valve/link.cfg", "y"), { code: "PATH_INVALID" });
  assert.equal(fs.readFileSync(path.join(outside, "secret.cfg"), "utf8"), "x");
});
