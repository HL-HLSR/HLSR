"use strict";

// Полная сборка в архиве .hlsrpack: экспорт папки игры и импорт с проверкой
// по подписанному списку файлов (src/main/game-delivery/game-pack.js).
// Работает на настоящем 7-Zip из 7zip-bin.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const ArchiveService = require("../src/main/game-delivery/ArchiveService");
const manifests = require("../src/main/game-delivery/manifests");
const filesManifestLib = require("../src/main/game-delivery/files-manifest");
const { gameInstallPath } = require("../src/main/game-delivery/paths");
const gamePack = require("../src/main/game-delivery/game-pack");

const GAME = "hl1-won";
const MZ = Buffer.concat([Buffer.from("MZ"), Buffer.alloc(200, 7)]);

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-pack-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function write(root, relative, data) {
  const full = path.join(root, ...relative.split("/"));
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, data);
}

function read(root, relative) {
  return fs.readFileSync(path.join(root, ...relative.split("/")), "utf8");
}

function exists(root, relative) {
  return fs.existsSync(path.join(root, ...relative.split("/")));
}

// Сборка: exe и данные — в подписанном списке; Bunnymod XT в списке тоже
// есть, но проверка его не сверяет (лаунчер обновляет его сам).
function buildFiles(root) {
  write(root, "Half-Life/hl.exe", MZ);
  write(root, "Half-Life/valve_WON/pak0.pak", "pak-data ".repeat(5000));
  write(root, "Half-Life/valve_WON/config.cfg", "name player\n");
  write(root, "Bunnymod XT/Injector.exe", MZ);
}

function setup(t) {
  const dir = tempDir(t);
  const buildRoot = path.join(dir, "build");
  buildFiles(buildRoot);
  const filesManifest = filesManifestLib.build({ root: buildRoot, gameId: GAME, version: "1.0.0" }).manifest;
  const manifest = Object.assign({}, manifests.MANIFESTS[GAME]);
  delete manifest.buildMetadata;
  const library = path.join(dir, "library");
  const service = new GameDeliveryService({
    libraryPath: library,
    launcherVersion: "0.4.6",
    manifests: { [GAME]: manifest },
    filesManifests: { [GAME]: { "1.0.0": filesManifest } },
    channelService: null,
    depotService: { assertOwns: async () => true },
    updateCheckIntervalMs: 0,
    logger: { info() {}, warn() {}, error() {} },
  });
  const installPath = gameInstallPath(service.paths, GAME);
  return { dir, buildRoot, service, installPath };
}

function install(env, extra) {
  fs.cpSync(env.buildRoot, env.installPath, { recursive: true });
  for (const [relative, data] of Object.entries(extra || {})) write(env.installPath, relative, data);
  env.service.installService.writeInstalled(
    { gameId: GAME, manifest: { version: "1.0.0" }, installPath: env.installPath, publishedAt: "2026-09-01T00:00:00.000Z" },
    "a".repeat(64)
  );
}

async function exportPack(env, name) {
  const file = path.join(env.dir, name || "hl1.hlsrpack");
  const result = await env.service.exportPack(GAME, file, { launcherId: "70", build: null, launcherVersion: "0.4.6", settings: { args: "-dev" } });
  return Object.assign(result, { file });
}

async function importPack(env, file) {
  const meta = await env.service.readPackMeta(file);
  return env.service.importPack(file, meta);
}

test("экспорт: игра, сейвы и cfg в одном архиве, служебный файл первым, без installed.json", async (t) => {
  const env = setup(t);
  install(env, { "Half-Life/valve_WON/SAVE/quick.sav": "save", "Half-Life/valve_WON/config.cfg": "name mine\n" });

  const result = await exportPack(env);
  assert.equal(result.status, "ok");
  assert.ok(!fs.existsSync(`${result.file}.hlsr-partial`));

  const listing = await new ArchiveService().list(result.file);
  // 7za на Windows отдаёт пути с обратной косой чертой.
  const names = listing.entries.filter((entry) => !entry.isDirectory).map((entry) => entry.path.split("\\").join("/"));
  assert.equal(names[0], gamePack.META_FILE);
  assert.ok(names.includes("Half-Life/valve_WON/SAVE/quick.sav"));
  assert.ok(names.includes("Half-Life/hl.exe"));
  assert.ok(!names.includes("installed.json"));

  const meta = await env.service.readPackMeta(result.file);
  assert.equal(meta.gameId, GAME);
  assert.equal(meta.version, "1.0.0");
  assert.equal(meta.installedVersion, "1.0.0");
  assert.deepEqual(meta.settings, { args: "-dev" });
  assert.equal(env.service.statusOf(GAME).state, "ready");
});

test("экспорт не берёт копии лаунчера .hlsr/backup, .hlsr/cfg-import-backup и .hlsr/imported-modified, остальное в .hlsr едет", (t) => {
  const root = tempDir(t);
  buildFiles(root);
  for (const relative of [".hlsr/backup/20261106-120000/import.json", ".hlsr/backup/migration-20261106-123456/Half-Life/valve_WON/userconfig.cfg",
    ".hlsr/cfg-import-backup/userconfig.cfg", ".hlsr/imported-modified/Half-Life/valve_WON/pak0.pak", ".hlsr/config-setup.json", ".hlsr/packs/state.json"])
    write(root, relative, "x");
  const { files } = gamePack.collect(root);
  assert.deepEqual(files.filter((file) => file.startsWith(".hlsr/")), [".hlsr/config-setup.json", ".hlsr/packs/state.json"]);
  assert.ok(files.includes("Half-Life/valve_WON/config.cfg"));
});

test("импорт на чистую библиотеку ставит сборку с сейвами и cfg, Bunnymod XT скачается заново", async (t) => {
  const source = setup(t);
  install(source, {
    "Half-Life/valve_WON/SAVE/quick.sav": "save",
    "Half-Life/valve_WON/config.cfg": "name mine\n",
    "Bunnymod XT/version": "1.2.3",
    "Bunnymod XT/.hlsr-injector-version": "1.0",
  });
  const { file } = await exportPack(source);

  const target = setup(t);
  const meta = await target.service.readPackMeta(file);
  assert.equal(meta.installedVersion, null);
  const result = await target.service.importPack(file, meta);

  assert.equal(result.status, "ok");
  const installed = target.service.installService.readInstalled(GAME);
  assert.equal(installed.version, "1.0.0");
  assert.equal(installed.publishedAt, "2026-09-01T00:00:00.000Z");
  assert.equal(read(target.installPath, "Half-Life/valve_WON/SAVE/quick.sav"), "save");
  assert.equal(read(target.installPath, "Half-Life/valve_WON/config.cfg"), "name mine\n");
  assert.ok(exists(target.installPath, "Half-Life/hl.exe"), "exe из подписанного списка остаётся");
  // Bunnymod XT проверка не сверяет — из архива он не берётся, а без файла
  // версии лаунчер скачает его перед запуском.
  assert.ok(!exists(target.installPath, "Bunnymod XT/Injector.exe"));
  assert.ok(!exists(target.installPath, "Bunnymod XT/.hlsr-injector-version"));
  assert.equal(result.dropped, 1);
  assert.deepEqual(fs.readdirSync(target.service.paths.staging), []);
  assert.equal(target.service.statusOf(GAME).state, "ready");
});

test("импорт отбрасывает посторонние и неподтверждённые исполняемые файлы, в том числе в сейвах", async (t) => {
  const source = setup(t);
  install(source, {
    "Half-Life/opengl32.dll": MZ,
    "Half-Life/valve_WON/SAVE/quick.sav": "save",
    "Half-Life/valve_WON/SAVE/hook.asi": MZ,
    "Half-Life/valve_WON/SAVE/run.bat": "start calc",
    "Half-Life/valve_WON/sprites/hud.spr": "sprite",
  });
  const { file } = await exportPack(source);

  const target = setup(t);
  const result = await importPack(target, file);
  assert.equal(result.status, "ok");
  for (const relative of ["Half-Life/opengl32.dll", "Half-Life/valve_WON/SAVE/hook.asi", "Half-Life/valve_WON/SAVE/run.bat"])
    assert.ok(!exists(target.installPath, relative), relative);
  assert.equal(read(target.installPath, "Half-Life/valve_WON/SAVE/quick.sav"), "save");
  assert.equal(read(target.installPath, "Half-Life/valve_WON/sprites/hud.spr"), "sprite");
});

// Подробно (зеркало, custom/imported, .dll) — game-pack-modified-selftest.js.
test("подменённый файл сборки: экспорт спрашивает, импорт без оригиналов с сервера отменяется, сборка не тронута", async (t) => {
  const source = setup(t);
  install(source);
  write(source.installPath, "Half-Life/valve_WON/pak0.pak", "tampered");
  await assert.rejects(exportPack(source), (error) => error.code === "PACK_MODIFIED" && error.details.modified === 1);
  const file = path.join(source.dir, "hl1.hlsrpack");
  await source.service.exportPack(GAME, file, { launcherId: "70", allowModified: true });

  const target = setup(t);
  install(target, { "Half-Life/valve_WON/SAVE/mine.sav": "keep" });
  await assert.rejects(importPack(target, file), (error) => error.code === "PACK_ORIGINALS_UNAVAILABLE");
  assert.equal(read(target.installPath, "Half-Life/valve_WON/SAVE/mine.sav"), "keep");
  assert.equal(target.service.installService.readInstalled(GAME).version, "1.0.0");
  assert.deepEqual(fs.readdirSync(target.service.paths.staging), []);
});

test("импорт поверх установленной сборки заменяет её целиком", async (t) => {
  const source = setup(t);
  install(source, { "Half-Life/valve_WON/SAVE/from-pack.sav": "pack" });
  const { file } = await exportPack(source);

  const target = setup(t);
  install(target, { "Half-Life/valve_WON/SAVE/old.sav": "old" });
  const meta = await target.service.readPackMeta(file);
  assert.equal(meta.installedVersion, "1.0.0");
  await target.service.importPack(file, meta);
  assert.equal(read(target.installPath, "Half-Life/valve_WON/SAVE/from-pack.sav"), "pack");
  assert.ok(!exists(target.installPath, "Half-Life/valve_WON/SAVE/old.sav"));
});

test("отмена экспорта не оставляет недописанный архив", async (t) => {
  const env = setup(t);
  install(env, { "Half-Life/valve_WON/big.bin": require("crypto").randomBytes(24 * 1024 * 1024) });
  const file = path.join(env.dir, "cancel.hlsrpack");
  const seen = [];
  env.service.on("status", (status) => {
    if (status.gameId === GAME) seen.push({ state: status.state, total: status.totalBytes, done: status.bytesDownloaded });
  });
  const pending = env.service.exportPack(GAME, file, { launcherId: "70" });
  const started = Date.now();
  while (env.service.statusOf(GAME).state !== "exporting" && Date.now() - started < 5000) await new Promise((r) => setTimeout(r, 10));
  await new Promise((r) => setTimeout(r, 100));
  assert.equal(env.service.cancel(GAME), true);
  await assert.rejects(pending, (error) => error.code === "CANCELLED");
  assert.ok(!fs.existsSync(file));
  assert.ok(!fs.existsSync(`${file}.hlsr-partial`));
  assert.equal(env.service.statusOf(GAME).state, "ready");
  // Прогресс — в байтах папки игры; после отмены карточка не пишет
  // «Установка завершена»: между экспортом и ready стоит cancelled.
  const exporting = seen.filter((item) => item.state === "exporting");
  assert.ok(exporting.length && exporting.every((item) => item.total > 24 * 1024 * 1024));
  const states = seen.map((item) => item.state).filter((state, i, all) => state !== all[i - 1]);
  assert.deepEqual(states.slice(-3), ["exporting", "cancelled", "ready"]);
});

test("чужой архив и сломанные сведения отклоняются", async (t) => {
  const env = setup(t);
  const plain = path.join(env.dir, "plain.7z");
  write(env.dir, "loose/readme.txt", "hi");
  await new ArchiveService().create(plain, { cwd: path.join(env.dir, "loose"), files: ["readme.txt"], threads: 1 }).promise;
  await assert.rejects(env.service.readPackMeta(plain), (error) => error.code === "PACK_BAD_FORMAT");

  const base = { format: "hlsr-pack", version: 1, game: { gameId: GAME, version: "1.0.0" } };
  const parse = (raw) => gamePack.parseMeta(JSON.stringify(raw), {
    isKnownGame: manifests.isKnownGame,
    isDepotBuild: manifests.isDepotBuild,
  });
  assert.equal(parse(base).gameId, GAME);
  assert.throws(() => parse(Object.assign({}, base, { version: 2 })), { code: "NEWER_FORMAT" });
  assert.throws(() => parse(Object.assign({}, base, { game: { gameId: "nope", version: "1.0.0" } })), { code: "UNKNOWN_GAME" });
  assert.throws(() => parse(Object.assign({}, base, { game: { gameId: "hl2-5288", version: "1.0.0" } })), { code: "UNKNOWN_GAME" });
  assert.throws(() => parse(Object.assign({}, base, { game: { gameId: GAME, version: "../x" } })), { code: "BAD_FORMAT" });
  assert.throws(() => gamePack.parseMeta("{"), { code: "BAD_FORMAT" });
  assert.equal(parse(Object.assign({}, base, { game: Object.assign({}, base.game, { manifestSha256: "zz" }) })).manifestSha256, null);
});

test("сборку из Steam целиком не экспортировать", async (t) => {
  const env = setup(t);
  await assert.rejects(env.service.exportPack("hl2-5288", path.join(env.dir, "x.hlsrpack"), {}), (error) => error.code === "PACK_DEPOT_BUILD");
});

test("исполняемые файлы опознаются по заголовку, а не по расширению", (t) => {
  const dir = tempDir(t);
  const cases = { "a.flt": MZ, "b.dat": Buffer.from([0x7f, 0x45, 0x4c, 0x46, 1]), "c.cmd": "x", "d.sav": "MZ" };
  for (const [name, data] of Object.entries(cases)) write(dir, name, data);
  write(dir, "e.txt", "plain");
  write(dir, "f.sav", "Z");
  for (const name of Object.keys(cases)) assert.equal(gamePack.isExecutable(path.join(dir, name)), true, name);
  assert.equal(gamePack.isExecutable(path.join(dir, "e.txt")), false);
  assert.equal(gamePack.isExecutable(path.join(dir, "f.sav")), false);
});
