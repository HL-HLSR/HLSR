"use strict";

// 7-Zip лаунчера: на Windows — свежая сборка из resources/7zip, а не 21.07 из 7zip-bin
// (там известные уязвимости декодеров, а архив .hlsrpack приходит от чужих людей).

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const sevenZip = require("../src/main/seven-zip");
const ArchiveService = require("../src/main/game-delivery/ArchiveService");

const windows = { skip: process.platform !== "win32" };
const ROOT = path.resolve(__dirname, "..");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-seven-zip-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("Windows: работает 7-Zip 26.05 из resources/7zip", windows, () => {
  const bin = sevenZip.binPath({ resourcesPath: null });
  assert.equal(bin, path.join(ROOT, "resources", "7zip", "win-x64", "7za.exe"));
  assert.match(spawnSync(bin, [], { encoding: "utf8" }).stdout, /7-Zip \(a\) 26\.05/);
});

test("упакованный лаунчер берёт 7za из своей папки resources", windows, (t) => {
  const resources = tempDir(t);
  fs.mkdirSync(path.join(resources, "7zip"));
  fs.writeFileSync(path.join(resources, "7zip", "7za.exe"), "MZ");
  assert.equal(sevenZip.binPath({ resourcesPath: resources }), path.join(resources, "7zip", "7za.exe"));
});

test("Linux пока остаётся на 7zip-bin", () => {
  assert.match(sevenZip.binPath({ platform: "linux", resourcesPath: null }), /7zip-bin/);
});

test("код лаунчера берёт 7-Zip только через seven-zip.js", () => {
  for (const file of ["game-delivery/ArchiveService.js", "diagnostics/DiagnosticsExport.js", "diagnostics/DiagnosticsZip.js", "profile-transfer/ProfileTransfer.js"]) {
    const code = fs.readFileSync(path.join(ROOT, "src", "main", file), "utf8");
    assert.doesNotMatch(code, /require\("7zip-bin"\)/, file);
    assert.match(code, /seven-zip/, file);
  }
});

test("сборка кладёт 7-Zip в resources и не везёт старый на Windows", () => {
  const build = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8")).build;
  assert.ok(build.win.extraResources.some((item) => item.from === "resources/7zip/win-x64" && item.to === "7zip"));
  assert.ok(!build.win.asarUnpack.some((item) => item.includes("7zip-bin")));
  // Только в общем списке и после ${os}: своё win.files заменило бы весь список, и в сборку попал бы репозиторий.
  assert.equal(build.win.files, undefined);
  assert.ok(build.files.indexOf("!**/node_modules/7zip-bin/win/**") > build.files.indexOf("**/node_modules/7zip-bin/${os}/**"));
});

test("в сборку идут только dist и icons, без исходников и скриптов", () => {
  const files = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8")).build.files;
  assert.deepEqual(files.filter((item) => !item.startsWith("!") && !item.includes("node_modules")), ["dist/electron/**/*", "icons"]);
});

test("архив с паролем не вешает чтение: 7-Zip не ждёт ввода", windows, async (t) => {
  const dir = tempDir(t);
  const file = path.join(dir, "secret.7z");
  fs.writeFileSync(path.join(dir, "note.json"), "{}");
  const made = spawnSync(sevenZip.binPath({ resourcesPath: null }), ["a", "-psecret", "-mhe=on", file, path.join(dir, "note.json")], { encoding: "utf8" });
  assert.equal(made.status, 0, made.stderr);

  const archive = new ArchiveService();
  const race = (promise) => Promise.race([
    promise.then(() => "ответил", () => "ответил"),
    new Promise((resolve) => setTimeout(() => resolve("завис"), 15000).unref()),
  ]);
  assert.equal(await race(archive.readText(file, "note.json", 1024)), "ответил");
  assert.equal(await race(archive.list(file)), "ответил");
});
