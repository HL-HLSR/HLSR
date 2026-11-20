"use strict";

// Эпизоды Ghosting: подписанный манифест эпизода, загрузка 7z с 127.0.0.1, проверка записей
// архива до распаковки и подмена папки episodic\ / ep2\ внутри установленной Ghosting.
// Настоящий 7-Zip, тестовый ключ подписи через QA-переменные channel-config.
// node --test scripts/episode-install-selftest.js

const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");

const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const manifests = require("../src/main/game-delivery/manifests");
const signing = require("../src/main/game-delivery/manifest-signing");
const episodes = require("../src/main/game-delivery/episodes");
const episodeInstall = require("../src/main/game-delivery/episode-install");
const sevenZip = require("../src/main/seven-zip");
const { gameInstallPath } = require("../src/main/game-delivery/paths");

const GAME = "ghosting-renewed";
const KEYS = signing.generateKeyPair();
const quiet = { info() {}, warn() {}, error() {} };
const sha = (data) => crypto.createHash("sha256").update(data).digest("hex");
const full = (root, relative) => path.join(root, ...relative.split("/"));

function write(root, relative, data) {
  fs.mkdirSync(path.dirname(full(root, relative)), { recursive: true });
  fs.writeFileSync(full(root, relative), data);
}

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-episode-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function unsigned(id, overrides) {
  return Object.assign(
    {
      schemaVersion: 1,
      gameId: GAME,
      episode: id,
      version: "1.0.0",
      publishedAt: "2026-11-14T00:00:00.000Z",
      requires: episodes.EPISODES[id].requires,
      archive: { fileName: `${GAME}-${id}-1.0.0.7z`, size: 10, sha256: "a".repeat(64) },
    },
    overrides
  );
}

const signed = (id, overrides) => signing.attach(unsigned(id, overrides), KEYS.privateKeyPem);

// Архив 7z из files (путь → содержимое), корни архива — первые сегменты путей.
function make7z(dir, name, files) {
  const stage = path.join(dir, `stage-${name}`);
  for (const [relative, data] of Object.entries(files)) write(stage, relative, data);
  const archive = path.join(dir, "archives", name);
  fs.mkdirSync(path.dirname(archive), { recursive: true });
  const roots = [...new Set(Object.keys(files).map((relative) => relative.split("/")[0]))];
  execFileSync(sevenZip.binPath(), ["a", "-t7z", "-bd", "-y", archive, ...roots], { cwd: stage, stdio: "pipe" });
  return fs.readFileSync(archive);
}

async function serve(t, routes, requests) {
  const server = http.createServer((request, response) => {
    requests.push(request.url);
    const body = routes.get(request.url);
    if (!body) {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, { "content-length": body.length });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  return `http://127.0.0.1:${server.address().port}/`;
}

// Выложить эпизод на сервер: архив и подписанный манифест рядом, как на hlsr.pro.
const fileNameOf = (id, version) => `${GAME}-${id}-${version || "1.0.0"}.7z`;

function publish(env, id, archive, overrides = {}) {
  const fileName = fileNameOf(id, overrides.version);
  const manifest = signed(id, Object.assign({ archive: { fileName, size: archive.length, sha256: sha(archive) } }, overrides));
  env.routes.set(`/${GAME}/episodes/${id}/${manifest.archive.fileName}`, archive);
  env.routes.set(`/${GAME}/episodes/${id}/stable.json`, Buffer.from(JSON.stringify(manifest)));
  return manifest;
}

// Временная библиотека с установленной Ghosting и QA-каналом на 127.0.0.1.
async function setup(t, { installed = true, channel = true } = {}) {
  const dir = tempDir(t);
  const routes = new Map();
  const requests = [];
  const base = await serve(t, routes, requests);
  const manifest = Object.assign({}, manifests.MANIFESTS[GAME]);
  delete manifest.buildMetadata;
  const service = new GameDeliveryService({
    libraryPath: path.join(dir, "library"),
    launcherVersion: "0.4.9",
    manifests: { [GAME]: manifest },
    channelService: null,
    originUpdates: null,
    updateCheckIntervalMs: 0,
    maxAttempts: 1,
    backoffBaseMs: 1,
    env: channel
      ? {
          GAME_DELIVERY_ALLOW_LOCAL_HTTP: "1",
          HLSR_GAME_DELIVERY_CHANNEL_URL: `${base}${GAME}/channel/stable.json`,
          HLSR_GAME_DELIVERY_CHANNEL_KEY: KEYS.publicKeyPem,
        }
      : {},
    logger: quiet,
  });
  t.after(() => service.dispose());
  const installPath = gameInstallPath(service.paths, GAME);
  if (installed) {
    write(installPath, "ghostingmod/bin/client.dll", "ghosting client");
    write(installPath, "ghostingmod/cfg/config.cfg", "bind w +forward\n");
    service.installService.writeInstalled(
      { gameId: GAME, manifest: { version: "1.0.13" }, installPath, publishedAt: "2026-11-12T00:00:00.000Z" },
      "a".repeat(64)
    );
  }
  return { dir, routes, requests, base, service, installPath };
}

const archiveRequests = (env, id, version) => env.requests.filter((url) => url.endsWith(`/${fileNameOf(id, version)}`)).length;
const parkOf = (env) => env.service.installService.episodesParkPath(GAME);

const EP1 = { "episodic/gameinfo.txt": "\"GameInfo\" { game \"Episode One\" }\n", "episodic/cfg/chapter1.cfg": "map ep1_citadel_00\n" };
const EP2 = { "ep2/gameinfo.txt": "\"GameInfo\" { game \"Episode Two\" }\n", "ep2/cfg/Chapter1.cfg": "map ep2_outland_01\n" };

const leftovers = (env) =>
  fs.readdirSync(env.service.paths.staging).concat(
    fs.existsSync(path.join(env.service.paths.downloads, GAME)) ? fs.readdirSync(path.join(env.service.paths.downloads, GAME)) : []
  );

test("манифест эпизода: подписанный принят, чужой байт, эпизод, имя архива и лишнее поле — нет", () => {
  const keys = [KEYS.publicKeyPem];
  const good = signed("ep1");
  assert.equal(episodeInstall.validateManifest(good, "ep1", keys).archive.fileName, `${GAME}-ep1-1.0.0.7z`);

  const flipped = JSON.parse(JSON.stringify(good).replace('"version":"1.0.0"', '"version":"1.0.1"'));
  assert.throws(() => episodeInstall.validateManifest(flipped, "ep1", keys), { code: "SIGNATURE_INVALID" });
  assert.throws(() => episodeInstall.validateManifest(unsigned("ep1"), "ep1", keys), { code: "SIGNATURE_INVALID" });
  assert.throws(() => episodeInstall.validateManifest(good, "ep1", [signing.generateKeyPair().publicKeyPem]), { code: "SIGNATURE_INVALID" });

  assert.throws(() => episodeInstall.validateManifest(signed("ep2"), "ep1", keys), { code: "MANIFEST_INVALID" });
  assert.throws(
    () => episodeInstall.validateManifest(signed("ep1", { archive: { fileName: "../x.7z", size: 10, sha256: "a".repeat(64) } }), "ep1", keys),
    { code: "MANIFEST_INVALID" }
  );
  assert.throws(
    () => episodeInstall.validateManifest(signed("ep1", { archive: { fileName: `${GAME}-ep2-1.0.0.7z`, size: 10, sha256: "a".repeat(64) } }), "ep1", keys),
    { code: "MANIFEST_INVALID" },
    "архив чужого эпизода"
  );
  assert.throws(() => episodeInstall.validateManifest(signed("ep1", { extra: 1 }), "ep1", keys), { code: "MANIFEST_INVALID" });
  assert.throws(() => episodeInstall.validateManifest(signed("ep2", { requires: [] }), "ep2", keys), { code: "MANIFEST_INVALID" });
});

test("адрес манифеста эпизода выводится из канала Ghosting; без канала эпизодов нет", () => {
  const source = episodeInstall.manifestSource("ep2", {
    HLSR_GAME_DELIVERY_CHANNEL_URL: "https://hlsr.pro/downloads/ghosting-renewed/channel/stable.json",
  });
  assert.equal(source.url, "https://hlsr.pro/downloads/ghosting-renewed/episodes/ep2/stable.json");
  assert.equal(episodeInstall.manifestSource("ep1", {}), null);
  const descriptor = episodeInstall.providerDescriptor(signed("ep2"), source.url);
  assert.deepEqual([descriptor.type, descriptor.url], ["https-direct", `https://hlsr.pro/downloads/ghosting-renewed/episodes/ep2/${GAME}-ep2-1.0.0.7z`]);
});


// Ghosting не тронута: ни её файлы, ни конфиг игрока.
function assertGhostingUntouched(env) {
  assert.equal(fs.readFileSync(path.join(env.installPath, "ghostingmod/bin/client.dll"), "utf8"), "ghosting client");
  assert.equal(fs.readFileSync(path.join(env.installPath, "ghostingmod/cfg/config.cfg"), "utf8"), "bind w +forward\n");
}

test("Episode One ставится в папку Ghosting: маркер со стоковыми cfg, Ghosting не тронута, архив убран", async (t) => {
  const env = await setup(t);
  const archive = make7z(env.dir, "ep1.7z", EP1);
  publish(env, "ep1", archive);

  const states = [];
  env.service.on("status", (status) => status.gameId === GAME && states.push(status.state));
  const result = await env.service.installEpisode("ep1");
  assert.deepEqual(result, { status: "installed", episode: "ep1", version: "1.0.0" });

  const marker = JSON.parse(fs.readFileSync(path.join(env.installPath, "episodic", episodes.MARKER), "utf8"));
  assert.equal(marker.version, "1.0.0");
  assert.equal(marker.sha256, sha(archive));
  assert.deepEqual(marker.stockCfg, ["chapter1.cfg"]);
  assert.ok(!Number.isNaN(Date.parse(marker.installedAt)));
  assertGhostingUntouched(env);
  assert.deepEqual(leftovers(env), [], "ни staging, ни архива в Downloads");
  assert.ok(states.includes("downloading") && states.includes("extracting"), states.join(" "));

  const status = env.service.statusOf(GAME);
  assert.equal(status.state, "ready");
  assert.equal(status.episode, null);
  assert.equal(status.operation, null);
  assert.deepEqual(status.episodes.ep1, { installed: true, version: "1.0.0", size: archive.length, paused: false });
  assert.equal(env.service.isBusy(), false);
});

test("переустановка эпизода: SAVE, runs, custom и демо игрока переезжают в новую папку, прежняя убрана", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await env.service.installEpisode("ep1");

  const player = {
    "episodic/SAVE/quick.sav": "quick",
    "episodic/runs/pb.dem": "pb",
    "episodic/custom/hud/layout.res": "my hud",
    "episodic/screenshots/s.jpg": "shot",
    "episodic/run1.dem": "root demo",
  };
  for (const [relative, data] of Object.entries(player)) write(env.installPath, relative, data);

  // В новой версии свой save/ и свой файл: данные игрока главнее, остальное — из архива.
  const next = make7z(env.dir, "ep1-next.7z", { "episodic/gameinfo.txt": "v2\n", "episodic/save/stock.sav": "stock", "episodic/maps/new.bsp": "bsp" });
  publish(env, "ep1", next, { version: "1.0.1" });
  await env.service.installEpisode("ep1");

  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.1");
  for (const [relative, data] of Object.entries(player)) assert.equal(fs.readFileSync(full(env.installPath, relative), "utf8"), data, relative);
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic/save/stock.sav")), false, "SAVE игрока целиком вместо save из архива");
  assert.equal(fs.readFileSync(path.join(env.installPath, "episodic/maps/new.bsp"), "utf8"), "bsp");
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic/cfg/chapter1.cfg")), false, "прочее — из нового архива");
  assert.equal(fs.existsSync(path.join(parkOf(env), "episodic")), false, "прежняя копия удалена");
  assertGhostingUntouched(env);
  assert.deepEqual(leftovers(env), []);
});

test("крах посреди подмены: прежняя папка в park, unparkEpisodes на старте возвращает её", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await env.service.installEpisode("ep1");
  write(env.installPath, "episodic/SAVE/quick.sav", "quick");
  publish(env, "ep1", make7z(env.dir, "ep1-next.7z", { "episodic/gameinfo.txt": "v2\n" }), { version: "1.0.1" });

  // Процесс «умер» между уходом прежней папки в park и приходом новой: никакой rename на её место не проходит.
  const target = path.join(env.installPath, "episodic");
  const rename = fs.renameSync;
  fs.renameSync = (from, to) => {
    if (path.resolve(to) === path.resolve(target)) throw Object.assign(new Error("крах"), { code: "EIO" });
    return rename(from, to);
  };
  try {
    await assert.rejects(env.service.installEpisode("ep1"));
  } finally {
    fs.renameSync = rename;
  }
  assert.equal(fs.existsSync(target), false);
  assert.equal(fs.existsSync(path.join(parkOf(env), "episodic/SAVE/quick.sav")), true);

  env.service.installService.unparkEpisodes(GAME);
  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.0");
  assert.equal(fs.readFileSync(path.join(target, "SAVE/quick.sav"), "utf8"), "quick");
});

test("откат версии эпизода — MANIFEST_INVALID, та же версия ставится заново", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1-v2.7z", EP1), { version: "1.0.1" });
  await env.service.installEpisode("ep1");

  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1), { version: "1.0.0" });
  await assert.rejects(env.service.installEpisode("ep1"), { code: "MANIFEST_INVALID", message: /Откат версии/ });
  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.1");
  assert.equal(archiveRequests(env, "ep1", "1.0.0"), 0, "старый архив даже не качается");

  publish(env, "ep1", make7z(env.dir, "ep1-v2-again.7z", EP1), { version: "1.0.1" });
  assert.equal((await env.service.installEpisode("ep1")).version, "1.0.1");
});

test("архив с чужим корнем или ../ отклоняется до распаковки, установленный эпизод цел", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "two-roots.7z", Object.assign({ "ep2/x": "x" }, EP1)));
  await assert.rejects(env.service.installEpisode("ep1"), { code: "ARCHIVE_UNSAFE_ENTRY" });
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic")), false);
  assert.equal(fs.existsSync(path.join(env.installPath, "ep2")), false);
  assert.deepEqual(leftovers(env), [], "доказанно плохой архив удалён");
  assert.equal(env.service.statusOf(GAME).error, null, "ошибка эпизода не висит на карточке Ghosting");

  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await env.service.installEpisode("ep1");
  write(env.installPath, "episodic/SAVE/quick.sav", "quick");

  // 7-Zip сам ../ не запишет: запись добавляется в листинг настоящего архива.
  publish(env, "ep1", make7z(env.dir, "ep1-next.7z", EP1), { version: "1.0.1" });
  const list = env.service.archiveService.list.bind(env.service.archiveService);
  env.service.archiveService.list = async (file) => {
    const listing = await list(file);
    listing.entries.push({ path: "../x", size: 1, isDirectory: false, isSymlink: false, encrypted: false });
    return listing;
  };
  await assert.rejects(env.service.installEpisode("ep1"), { code: "ARCHIVE_UNSAFE_ENTRY" });
  assert.equal(fs.existsSync(path.join(env.service.paths.root, "x")), false);
  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.0");
  assert.equal(fs.readFileSync(path.join(env.installPath, "episodic/SAVE/quick.sav"), "utf8"), "quick");
  assert.deepEqual(leftovers(env), []);
});

test("неверный sha256 — CHECKSUM_MISMATCH: нового эпизода нет, установленный цел", async (t) => {
  const env = await setup(t);
  const archive = make7z(env.dir, "ep1.7z", EP1);
  publish(env, "ep1", archive, { archive: { fileName: fileNameOf("ep1"), size: archive.length, sha256: "b".repeat(64) } });
  await assert.rejects(env.service.installEpisode("ep1"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic")), false);
  assert.equal(env.service.isBusy(), false);

  publish(env, "ep1", archive);
  await env.service.installEpisode("ep1");
  const next = make7z(env.dir, "ep1-next.7z", { "episodic/gameinfo.txt": "v2\n" });
  publish(env, "ep1", next, { version: "1.0.1", archive: { fileName: fileNameOf("ep1", "1.0.1"), size: next.length, sha256: "c".repeat(64) } });
  await assert.rejects(env.service.installEpisode("ep1"), { code: "CHECKSUM_MISMATCH" });
  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.0");
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic/cfg/chapter1.cfg")), true);
  assert.deepEqual(leftovers(env), []);
});

test("игра запустилась после загрузки — GAME_RUNNING, архив остаётся, повтор ставит без второй загрузки", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  let running = false;
  env.service.processService.isRunning = () => running;
  const onState = (event) => {
    if (event.state === "downloaded") running = true;
  };
  env.service.downloadManager.on("state", onState);
  await assert.rejects(env.service.installEpisode("ep1"), { code: "GAME_RUNNING" });
  env.service.downloadManager.off("state", onState);
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic")), false);
  assert.deepEqual(leftovers(env), [fileNameOf("ep1")], "архив цел, staging убран");

  running = false;
  await env.service.installEpisode("ep1");
  assert.equal(episodes.status(env.installPath).ep1.installed, true);
  assert.equal(archiveRequests(env, "ep1"), 1, "архив скачан один раз");
  assert.deepEqual(leftovers(env), []);
});

test("второй вызов во время установки — ALREADY_RUNNING", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  const first = env.service.installEpisode("ep1");
  await assert.rejects(env.service.installEpisode("ep1"), { code: "ALREADY_RUNNING" });
  await assert.rejects(env.service.removeEpisode("ep1"), { code: "ALREADY_RUNNING" });
  await first;
  assert.equal(episodes.status(env.installPath).ep1.installed, true);
});

test("Episode Two без Episode One ставит оба; удаление Episode One снимает и Two", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  publish(env, "ep2", make7z(env.dir, "ep2.7z", EP2));

  const result = await env.service.installEpisode("ep2");
  assert.equal(result.episode, "ep2");
  const state = episodes.status(env.installPath);
  assert.deepEqual([state.ep1.installed, state.ep2.installed], [true, true]);
  assert.deepEqual(episodes.readMarker(env.installPath, "ep2").stockCfg, ["chapter1.cfg"]);

  const removed = await env.service.removeEpisode("ep1");
  assert.deepEqual(removed, { status: "removed", removed: ["ep2", "episodic"] });
  assert.equal(fs.existsSync(path.join(env.installPath, "episodic")), false);
  assert.equal(fs.existsSync(path.join(env.installPath, "ep2")), false);
  assertGhostingUntouched(env);
  assert.equal(env.service.isBusy(), false);
});

test("без Ghosting — NOT_INSTALLED, неизвестный эпизод — LAUNCH_CONFIG_INVALID", async (t) => {
  const env = await setup(t, { installed: false });
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await assert.rejects(env.service.installEpisode("ep1"), { code: "NOT_INSTALLED" });
  await assert.rejects(env.service.removeEpisode("ep1"), { code: "NOT_INSTALLED" });
  await assert.rejects(env.service.installEpisode("ep3"), { code: "LAUNCH_CONFIG_INVALID" });
  await assert.rejects(env.service.installEpisode(["ep1"]), { code: "LAUNCH_CONFIG_INVALID" });
  assert.equal(env.service.isBusy(), false);
});

// .part и его состояние, как их оставляет GameDownloadManager на паузе.
function pausedPart(env, fileName, bytes) {
  const manager = env.service.downloadManager;
  const files = manager.partPaths(GAME, fileName);
  fs.mkdirSync(files.dir, { recursive: true });
  fs.writeFileSync(files.part, bytes);
  manager.writeState(files.state, { gameId: GAME, filename: fileName, expectedSize: 1000, expectedSha256: "d".repeat(64), bytesDownloaded: bytes.length });
  return files;
}

test("состояние эпизодов: размер из манифеста, без канала — null, отказ сервера помнится минуту", async (t) => {
  const offline = await setup(t, { channel: false });
  const none = await offline.service.episodesStatus();
  assert.deepEqual(none.ep1, { installed: false, version: null, size: null, paused: false, available: false });
  await assert.rejects(offline.service.installEpisode("ep1"), { code: "CHANNEL_UNAVAILABLE" });

  const env = await setup(t);
  const archive = make7z(env.dir, "ep1.7z", EP1);
  publish(env, "ep1", archive);
  const view = await env.service.episodesStatus();
  assert.deepEqual(view.ep1, { installed: false, version: null, size: archive.length, paused: false, available: true });
  assert.equal(view.ep2.available, false);
  assert.equal(env.service.statusOf(GAME).episodes.ep1.size, archive.length, "размер приходит и событием статуса");

  const asked = env.requests.filter((url) => url.endsWith("/ep2/stable.json")).length;
  assert.equal((await env.service.episodesStatus()).ep2.available, false);
  assert.equal(env.requests.filter((url) => url.endsWith("/ep2/stable.json")).length, asked, "второй раз сервер не спрошен");
});

test("пауза: эпизод на паузе виден после перезапуска, отмена Ghosting его не трогает, новая версия убирает старый .part", async (t) => {
  const env = await setup(t);
  // Без манифеста в памяти (как после перезапуска лаунчера) — по файлам в Downloads.
  const episodePart = pausedPart(env, fileNameOf("ep1", "0.9.0"), Buffer.from("12345"));
  assert.equal(env.service.refresh(GAME).episodes.ep1.paused, true);

  // Своя загрузка Ghosting на паузе: её отмена снимает только её .part.
  const ghostingPart = pausedPart(env, `${GAME}-1.0.13.7z`, Buffer.from("abc"));
  env.service.setStatus(GAME, { state: "paused" });
  assert.equal(env.service.cancel(GAME), true);
  assert.equal(fs.existsSync(ghostingPart.part), false);
  assert.equal(fs.existsSync(episodePart.part), true);

  // Отмена эпизода на паузе, когда Ghosting стоит.
  env.service.refresh(GAME);
  assert.equal(env.service.cancel(GAME), true);
  assert.equal(fs.existsSync(episodePart.part), false);
  assert.equal(env.service.statusOf(GAME).episodes.ep1.paused, false);

  // Сервер выложил новую версию: .part прежней убирается, когда начинается новая.
  const stale = pausedPart(env, fileNameOf("ep1", "0.9.0"), Buffer.from("12345"));
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await env.service.installEpisode("ep1");
  assert.equal(fs.existsSync(stale.part), false);
  assert.equal(fs.existsSync(stale.state), false);
  assert.deepEqual(leftovers(env), []);
});

// Крах после того, как новая папка встала, но до переезда данных игрока: новая — на месте,
// прежняя с SAVE и демо — в park. Старт лаунчера (unparkEpisodes) переносит их в эпизод.
async function crashedAfterPlace(t) {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  await env.service.installEpisode("ep1");
  const parked = path.join(parkOf(env), "episodic");
  write(parked, "SAVE/quick.sav", "quick");
  write(parked, "run1.dem", "root demo");
  write(parked, "maps/old.bsp", "old content");
  write(parked, episodes.MARKER, JSON.stringify({ version: "0.9.0" }));
  return { env, parked };
}

test("крах после новой папки: unparkEpisodes переносит SAVE и демо из park, контент игры убирает", async (t) => {
  const { env, parked } = await crashedAfterPlace(t);
  env.service.installService.unparkEpisodes(GAME);
  const episodeDir = path.join(env.installPath, "episodic");
  assert.equal(fs.readFileSync(path.join(episodeDir, "SAVE/quick.sav"), "utf8"), "quick");
  assert.equal(fs.readFileSync(path.join(episodeDir, "run1.dem"), "utf8"), "root demo");
  assert.equal(fs.existsSync(path.join(episodeDir, "maps/old.bsp")), false);
  assert.equal(episodes.status(env.installPath).ep1.version, "1.0.0", "маркер новой папки не тронут");
  assert.equal(fs.existsSync(parked), false);
  assert.equal(fs.existsSync(parkOf(env)), false);
});

test("крах после новой папки, а в ней свой SAVE: ничего игрока не удаляется", async (t) => {
  const { env } = await crashedAfterPlace(t);
  write(env.installPath, "episodic/save/own.sav", "own");
  env.service.installService.unparkEpisodes(GAME);
  assert.equal(fs.readFileSync(path.join(env.installPath, "episodic/save/own.sav"), "utf8"), "own");
  assert.equal(fs.readFileSync(path.join(env.installPath, "episodic/run1.dem"), "utf8"), "root demo");
  const aside = fs.readdirSync(parkOf(env)).filter((name) => name.startsWith("episodic.player-"));
  assert.equal(aside.length, 1);
  assert.equal(fs.readFileSync(path.join(parkOf(env), aside[0], "SAVE/quick.sav"), "utf8"), "quick");
  assert.equal(fs.existsSync(path.join(parkOf(env), "episodic")), false);
});

test("отмена из карточки эпизода на паузе убирает только его .part; чужой id — архивы всех", async (t) => {
  const env = await setup(t);
  const ep1 = pausedPart(env, fileNameOf("ep1"), Buffer.from("12345"));
  const ep2 = pausedPart(env, fileNameOf("ep2"), Buffer.from("67890"));
  env.service.refresh(GAME);

  assert.equal(env.service.cancel(GAME, { episode: "ep2" }), true);
  assert.equal(fs.existsSync(ep2.part), false);
  assert.equal(fs.existsSync(ep1.part), true, "Episode One на паузе остался");
  assert.equal(env.service.statusOf(GAME).episodes.ep1.paused, true);

  assert.equal(env.service.cancel(GAME, { episode: "../ep1" }), true);
  assert.equal(fs.existsSync(ep1.part), false);
});

test("оставленный отказом архив эпизода убирают и отмена, и удаление эпизода", async (t) => {
  const env = await setup(t);
  publish(env, "ep1", make7z(env.dir, "ep1.7z", EP1));
  let running = false;
  env.service.processService.isRunning = () => running;
  env.service.downloadManager.on("state", (event) => {
    if (event.state === "downloaded") running = true;
  });

  await assert.rejects(env.service.installEpisode("ep1"), { code: "GAME_RUNNING" });
  assert.deepEqual(leftovers(env), [fileNameOf("ep1")]);
  running = false;
  assert.equal(env.service.cancel(GAME), true);
  assert.deepEqual(leftovers(env), []);
  assert.equal(env.service.cancel(GAME), false, "убирать больше нечего");

  await assert.rejects(env.service.installEpisode("ep1"), { code: "GAME_RUNNING" });
  assert.deepEqual(leftovers(env), [fileNameOf("ep1")]);
  running = false;
  assert.deepEqual(await env.service.removeEpisode("ep1"), { status: "removed", removed: [] });
  assert.deepEqual(leftovers(env), []);
});
