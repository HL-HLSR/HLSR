"use strict";

// Эпизоды Ghosting (episodic\, ep2\ в папке игры): docs/superpowers/specs/2026-11-14-ghosting-episodes-design.md.
// node --test scripts/episodes-selftest.js

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("fs");
const os = require("os");
const path = require("path");
const mutable = require("../src/main/game-delivery/mutable-paths");

test("эпизоды: проверка файлов не судит, ремонт не трогает, но это не файлы игрока для копирования", () => {
  for (const p of ["episodic/gameinfo.txt", "episodic/ep1_pak_000.vpk", "ep2/bin/client.dll"])
    assert.equal(mutable.matchesMutable(p), true, p);
  assert.equal(mutable.matchesMutable("ghostingmod/bin/client.dll"), false);
  assert.deepEqual(mutable.EPISODE_DIRS, ["episodic", "ep2"]);
  assert.ok(!mutable.USER_DATA.includes("episodic"), "6 ГБ не копируются как файлы игрока");
});

test("эпизоды: ни один выпущенный список файлов не поставляет episodic/ и ep2/ (правило общее для всех игр)", () => {
  const dir = path.join(__dirname, "..", "config", "games");
  for (const name of fs.readdirSync(dir).filter((n) => n.endsWith(".files.json"))) {
    const files = JSON.parse(fs.readFileSync(path.join(dir, name), "utf8")).files || [];
    const hit = files.find((f) => mutable.EPISODE_DIRS.some((d) => f.path === d || f.path.startsWith(d + "/")));
    assert.equal(hit, undefined, `${name}: ${hit && hit.path}`);
  }
});

// Старая сборка с эпизодами; withUpdate — полное обновление (commitUpdate), иначе переустановка (swapIntoLibrary).
// opts.newTree — файлы новой сборки, opts.fault — имя шага, на котором обновление падает.
async function reinstall(t, withUpdate, opts = {}) {
  const InstallService = require("../src/main/game-delivery/InstallService");
  const { resolveLibrary, gameInstallPath } = require("../src/main/game-delivery/paths");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-episodes-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = resolveLibrary(root);
  fs.mkdirSync(paths.state, { recursive: true });
  const installPath = gameInstallPath(paths, "ghosting-renewed");
  const old = {
    "hl2.exe": "старая",
    "ghostingmod/SAVE/quick.sav": "сейв",
    "episodic/gameinfo.txt": "эпизод 1",
    "ep2/bin/client.dll": "эпизод 2",
    "installed.json": JSON.stringify({ state: "ready", version: "1.0.0" }),
  };
  for (const [relative, text] of Object.entries(old)) {
    fs.mkdirSync(path.dirname(path.join(installPath, relative)), { recursive: true });
    fs.writeFileSync(path.join(installPath, relative), text);
  }

  const newTree = opts.newTree || { "hl2.exe": "новая" };
  const service = new InstallService({
    paths,
    spaceMargin: 0,
    faultInjector: (name) => {
      if (name === opts.fault) throw new Error(`сбой на ${name}`);
    },
    archiveService: {
      inspect: async () => ({ listing: { entries: [] }, report: { uncompressedSize: 5 } }),
      extract: (archive, staging) => ({
        cancel() {},
        promise: Promise.resolve().then(() => {
          for (const [relative, text] of Object.entries(newTree)) {
            fs.mkdirSync(path.dirname(path.join(staging, relative)), { recursive: true });
            fs.writeFileSync(path.join(staging, relative), text);
          }
        }),
      }),
    },
  });
  t.mock.method(service, "verifyArchive", async () => "sha");
  t.mock.method(service, "checkSpace");
  const copied = [];
  const copyFileSync = fs.copyFileSync;
  t.mock.method(fs, "copyFileSync", (from, to, mode) => {
    copied.push(from);
    return copyFileSync(from, to, mode);
  });

  const manifest = { gameId: "ghosting-renewed", version: "1.0.1", expectedInstalledSize: 5, preservePaths: [], baseArchive: {}, filesManifest: { requiredFiles: ["hl2.exe"] } };
  const update = withUpdate
    ? { fromVersion: "1.0.0", filesManifest: { version: "1.0.1" }, integrityService: { check: async () => ({ ok: true, extra: [] }) } }
    : undefined;
  let error = null;
  try {
    await service.install({ manifest, update, archivePath: path.join(root, "ghosting-renewed.7z") });
  } catch (e) {
    error = e;
  }
  if (error && !opts.fault) throw error;
  const read = (relative) => (fs.existsSync(path.join(installPath, relative)) ? fs.readFileSync(path.join(installPath, relative), "utf8") : null);
  const park = service.episodesParkPath("ghosting-renewed");
  return { read, copied, service, paths, error, installPath, park };
}

test("обновление упало после подмены — старая сборка вернулась вместе с эпизодами", async (t) => {
  const { read, error, park } = await reinstall(t, true, { fault: "before-installed-write" });
  assert.ok(error, "обновление не прошло");
  assert.equal(read("hl2.exe"), "старая");
  assert.equal(read("episodic/gameinfo.txt"), "эпизод 1");
  assert.equal(read("ep2/bin/client.dll"), "эпизод 2");
  assert.equal(fs.existsSync(park), false, "ничего не осталось отложенным");
});

test("в новой сборке своя папка episodic — прежний эпизод не удаляется, ждёт отложенным", async (t) => {
  const { read, park } = await reinstall(t, true, { newTree: { "hl2.exe": "новая", "episodic/gameinfo.txt": "из сборки" } });
  assert.equal(read("episodic/gameinfo.txt"), "из сборки");
  assert.equal(read("ep2/bin/client.dll"), "эпизод 2");
  assert.equal(fs.readFileSync(path.join(park, "episodic", "gameinfo.txt"), "utf8"), "эпизод 1");
});

test("удаление игры убирает и отложенные эпизоды; старая отложенная копия не стопорит обновление", async (t) => {
  const { removeGame } = require("../src/main/game-delivery/game-removal");
  const { service, paths, park, installPath, read } = await reinstall(t, true);
  fs.mkdirSync(path.join(park, "episodic"), { recursive: true });
  fs.writeFileSync(path.join(park, "episodic", "gameinfo.txt"), "старая отложенная");
  service.parkEpisodes("ghosting-renewed", installPath); // в игре свой episodic: старая копия отходит в сторону
  assert.ok(fs.readdirSync(park).some((name) => name.startsWith("episodic.stale-")));
  service.unparkEpisodes("ghosting-renewed", installPath);
  assert.equal(read("episodic/gameinfo.txt"), "эпизод 1");
  await removeGame(paths, "ghosting-renewed");
  assert.equal(fs.existsSync(park), false, "6 ГБ не остаются в библиотеке");
});

test("эпизоды, отложенные прерванной подменой, возвращаются на старте лаунчера", async (t) => {
  const { service, installPath, park } = await reinstall(t, true);
  fs.mkdirSync(park, { recursive: true });
  fs.renameSync(path.join(installPath, "ep2"), path.join(park, "ep2"));
  service.unparkEpisodes("ghosting-renewed");
  assert.equal(fs.readFileSync(path.join(installPath, "ep2", "bin", "client.dll"), "utf8"), "эпизод 2");
  assert.equal(fs.existsSync(park), false);
});

for (const withUpdate of [true, false]) {
  test(`эпизоды переезжают в новую сборку переименованием (${withUpdate ? "обновление" : "переустановка"})`, async (t) => {
    const { read, copied, service, paths } = await reinstall(t, withUpdate);
    assert.equal(read("hl2.exe"), "новая");
    assert.equal(read("episodic/gameinfo.txt"), "эпизод 1");
    assert.equal(read("ep2/bin/client.dll"), "эпизод 2");
    assert.ok(!copied.some((from) => /[\\/](episodic|ep2)[\\/]/.test(from)), "не копировались: " + copied.join(", "));
    if (withUpdate) {
      assert.equal(read("ghostingmod/SAVE/quick.sav"), "сейв");
      assert.equal(service.checkSpace.mock.calls[0].arguments[1], Buffer.byteLength("сейв"), "место под эпизоды не просится");
    }
    assert.deepEqual(fs.existsSync(paths.staging) ? fs.readdirSync(paths.staging) : [], [], "staging убран");
  });
}
