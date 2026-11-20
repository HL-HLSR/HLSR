"use strict";

// Демо HL1 и Ghosting: список, выбор, имена в архиве и упаковка в zip настоящим 7za.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const ArchiveService = require("../src/main/game-delivery/ArchiveService");
const manifests = require("../src/main/game-delivery/manifests");
const demo = require("../src/main/game-delivery/demo-archive");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-demo-archive-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function put(root, relative, text = "demo") {
  const file = path.join(root, ...relative.split("/"));
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

test("archiveName: местное время и суффиксы при совпадении", () => {
  const date = new Date(2026, 10, 7, 4, 5);
  const base = "hl1-demos-2026-11-07-0405";
  assert.equal(demo.archiveName("hl1", date, new Set()), `${base}.zip`);
  assert.equal(demo.archiveName("hl1", date, new Set([`${base}.zip`])), `${base}-2.zip`);
  assert.equal(demo.archiveName("ghosting", date, new Set([`${base}.zip`, `${base}-2.zip`])), `ghosting-demos-2026-11-07-0405.zip`);
  assert.equal(demo.archiveName("hl1", date, new Set([`${base}.zip`, `${base}-2.zip`])), `${base}-3.zip`);
});

test("pickItems: пустой запрос, чужие и опасные имена", () => {
  const scanned = [{ dir: "a/valve_WON", name: "run.dem", size: 1, mtimeMs: 1 }];
  assert.deepEqual(demo.pickItems([], scanned), { ok: false, code: "NO_ITEMS" });
  assert.deepEqual(demo.pickItems([{ dir: "a/valve_WON", name: "other.dem" }], scanned), { ok: false, code: "DEMO_NOT_FOUND" });
  for (const bad of [
    { dir: "../x", name: "a.dem" },
    { dir: "C:/x", name: "a.dem" },
    { dir: "a/valve_WON", name: "a/../b.dem" },
    { dir: "a/valve_WON", name: "..\\run.dem" },
  ])
    assert.deepEqual(demo.pickItems([bad], scanned), { ok: false, code: "DEMO_NOT_FOUND" });
  assert.equal(demo.pickItems([{ dir: "a/valve_WON", name: "run.dem" }], scanned).ok, true);
});

test("entryNames: повтор имени получает префикс папки", () => {
  const items = [
    { dir: "Half-Life/valve_WON", name: "run.dem" },
    { dir: "Half-Life/valve_WON_russian", name: "run.dem" },
  ];
  assert.deepEqual(demo.entryNames(items).map((e) => e.entry), ["run.dem", "valve_WON_russian_run.dem"]);
});

test("packDemos: два демо — два имени в корне zip, временной папки нет", async (t) => {
  const install = tempDir(t);
  const out = path.join(tempDir(t), "Demos");
  put(install, "Half-Life/valve_WON/run.dem");
  put(install, "Half-Life/valve_WON_russian/run.dem");
  const items = [
    { dir: "Half-Life/valve_WON", name: "run.dem" },
    { dir: "Half-Life/valve_WON_russian", name: "run.dem" },
  ];
  const archive = new ArchiveService();
  // M4/M6: список файлов для 7za лежит в staging (не в Demos), ход упаковки уходит наружу.
  const create = archive.create.bind(archive);
  let seen = null;
  archive.create = (zipPath, opts) => { seen = opts; return create(zipPath, opts); };
  const progress = [];
  const result = await demo.packDemos({ installPath: install, items, archivesDir: out, slug: "hl1", now: new Date(2026, 10, 7, 4, 5), archive, onProgress: (p) => progress.push(p) });
  assert.ok(seen.listDir && path.dirname(seen.listDir) === out && path.basename(seen.listDir).startsWith(".staging-"), "listDir внутри staging");
  assert.equal(typeof seen.onProgress, "function");
  assert.ok(progress.every((p) => p >= 0 && p <= 100));
  assert.ok(fs.readdirSync(out).every((file) => file.endsWith(".zip")), "в Demos нет ничего кроме zip");
  assert.equal(result.name, "hl1-demos-2026-11-07-0405.zip");
  assert.ok(result.size > 0);
  const listed = await archive.list(path.join(out, result.name));
  const names = JSON.stringify(listed);
  assert.ok(names.includes("valve_WON_russian_run.dem") && names.includes("run.dem"), names);
  assert.deepEqual(fs.readdirSync(out), [result.name]);
});

test("entryNames: три файла с совпадающими именами — три разных записи в zip", async (t) => {
  const install = tempDir(t);
  const out = tempDir(t);
  put(install, "Half-Life/valve_WON/run.dem", "1");
  put(install, "Half-Life/valve_WON_russian/run.dem", "2");
  put(install, "Half-Life/valve_WON/valve_WON_russian_run.dem", "3");
  const items = [
    { dir: "Half-Life/valve_WON", name: "run.dem" },
    { dir: "Half-Life/valve_WON_russian", name: "run.dem" },
    { dir: "Half-Life/valve_WON", name: "valve_WON_russian_run.dem" },
  ];
  const entries = demo.entryNames(items).map((e) => e.entry);
  assert.equal(new Set(entries.map((e) => e.toLowerCase())).size, 3);
  const archive = new ArchiveService();
  const result = await demo.packDemos({ installPath: install, items, archivesDir: out, slug: "hl1", now: new Date(), archive });
  const listed = await archive.list(path.join(out, result.name));
  for (const entry of entries) assert.ok(JSON.stringify(listed).includes(entry), entry);
});

test("packDemos: старая .staging-* папка убирается", async (t) => {
  const install = tempDir(t);
  const out = tempDir(t);
  put(install, "ghostingmod/a.dem");
  fs.mkdirSync(path.join(out, ".staging-old"));
  await demo.packDemos({ installPath: install, items: [{ dir: "ghostingmod", name: "a.dem" }], archivesDir: out, slug: "ghosting", now: new Date(), archive: new ArchiveService() });
  assert.ok(!fs.existsSync(path.join(out, ".staging-old")));
});

test("isArchiveName", () => {
  for (const ok of ["hl1-demos-2026-11-07-0405.zip", "ghosting-demos-2026-11-07-0405-2.zip"]) assert.equal(demo.isArchiveName(ok), true);
  for (const bad of ["../x.zip", "hl1-demos-2026.zip\\..\\a", "HL1-demos-2026-11-07-0405.zip", "x.zip", 5]) assert.equal(demo.isArchiveName(bad), false);
});

test("packDemos: ошибка архиватора — ни zip, ни .staging-*", async (t) => {
  const install = tempDir(t);
  const out = tempDir(t);
  put(install, "ghostingmod/a.dem");
  const archive = {
    create(zipPath) {
      fs.writeFileSync(zipPath, "half");
      return { promise: Promise.reject(new Error("boom")) };
    },
  };
  await assert.rejects(
    demo.packDemos({ installPath: install, items: [{ dir: "ghostingmod", name: "a.dem" }], archivesDir: out, slug: "ghosting", now: new Date(), archive }),
    /boom/
  );
  assert.deepEqual(fs.readdirSync(out), []);
});

test("packDemos: демо пропало после списка — DEMO_NOT_FOUND и чистая папка", async (t) => {
  const install = tempDir(t);
  const out = tempDir(t);
  await assert.rejects(
    demo.packDemos({ installPath: install, items: [{ dir: "ghostingmod", name: "gone.dem" }], archivesDir: out, slug: "ghosting", now: new Date(), archive: new ArchiveService() }),
    { code: "DEMO_NOT_FOUND" }
  );
  assert.deepEqual(fs.readdirSync(out), []);
});

test("listDemos Ghosting: ghostingmod и ghostingmod/demos без вложенных, без папки demos — без ошибки", (t) => {
  const install = tempDir(t);
  const config = manifests.localConfigOf("ghosting-renewed");
  put(install, "ghostingmod/a.dem");
  put(install, "ghostingmod/runs/x.run");
  assert.deepEqual(demo.listDemos(config, install).map((d) => `${d.dir}/${d.name}`), ["ghostingmod/a.dem"]);
  put(install, "ghostingmod/demos/b.dem");
  put(install, "ghostingmod/demos/sub/x.dem");
  assert.deepEqual(demo.listDemos(config, install).map((d) => `${d.dir}/${d.name}`).sort(), ["ghostingmod/a.dem", "ghostingmod/demos/b.dem"]);
});

test("listDemos HL1: одинаковые имена из двух папок остаются оба; пустая установка — пустой список", (t) => {
  const install = tempDir(t);
  const config = manifests.localConfigOf("hl1-won");
  assert.deepEqual(demo.listDemos(config, install), []);
  put(install, "Half-Life/valve_WON/run.dem");
  put(install, "Half-Life/valve_WON_russian/run.dem");
  assert.equal(demo.listDemos(config, install).length, 2);
});
