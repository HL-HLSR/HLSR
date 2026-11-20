"use strict";

// Данные с зеркал и сервера канала, которым нельзя верить без подписи:
// удаления из манифеста патча и повтор старого подписанного канала.
// node --test scripts/delivery-trust-selftest.js

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const http = require("http");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
const ChannelService = require("../src/main/game-delivery/ChannelService");
const filesManifestLib = require("../src/main/game-delivery/files-manifest");
const signing = require("../src/main/game-delivery/manifest-signing");
const patchFormat = require("../src/main/game-delivery/patch-format");
const manifests = require("../src/main/game-delivery/manifests");

const GAME = "ghosting-renewed";
const sha = (text) => crypto.createHash("sha256").update(text).digest("hex");
const entry = (relative, text) => ({ path: relative, size: Buffer.byteLength(text), sha256: sha(text) });

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-delivery-trust-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function filesManifest(version, files) {
  return {
    schemaVersion: 1,
    gameId: GAME,
    version,
    totalFiles: files.length,
    totalSize: files.reduce((sum, file) => sum + file.size, 0),
    files,
  };
}

test("манифест патча с зеркала удаляет только то, что прежняя сборка поставляла, а новая убрала", async (t) => {
  const dir = tempDir(t);
  const previous = filesManifest("1.0.0", [entry("bin/client.dll", "c"), entry("bin/old.dll", "old"), entry("bin/server.dll", "v1")]);
  const resulting = filesManifest("1.0.1", [entry("bin/client.dll", "c"), entry("bin/server.dll", "v2")]);
  const warnings = [];
  const service = new GameDeliveryService({
    libraryPath: path.join(dir, "library"),
    filesManifests: { [GAME]: { "1.0.0": previous, "1.0.1": resulting } },
    channelService: null,
    logger: { info() {}, warn: (line) => warnings.push(line), error() {} },
  });
  t.after(() => service.dispose());

  const archivePath = path.join(dir, `${GAME}-1.0.0-to-1.0.1.7z`);
  const candidate = archivePath.replace(/\.7z$/, ".json");
  const step = (toVersion) => ({
    fromVersion: "1.0.0",
    toVersion,
    archive: { sha256: "a".repeat(64) },
    resultingFilesManifestSha256: filesManifestLib.hashManifest(resulting),
  });
  const load = (deletedFiles, toVersion) => {
    fs.writeFileSync(candidate, JSON.stringify({
      schemaVersion: 1,
      gameId: GAME,
      fromVersion: "1.0.0",
      toVersion: toVersion || "1.0.1",
      payloadRoot: "payload",
      files: [entry("bin/server.dll", "v2")],
      deletedFiles,
      archive: { filename: path.basename(archivePath), size: 10, sha256: "a".repeat(64) },
      resultingFilesManifestSha256: filesManifestLib.hashManifest(resulting),
    }));
    return service.loadPatchManifest(GAME, step(toVersion || "1.0.1"), archivePath, []);
  };

  for (const forged of [["bin/old.dll", "ghostingmod/SAVE/quick.sav"], ["ghostingmod/runs/pb.dem"], ["bin/client.dll"]]) {
    await assert.rejects(load(forged), { code: "MANIFEST_INVALID" }, forged.join(", "));
    assert.equal(fs.existsSync(candidate), false, "отвергнутый манифест не остаётся в Downloads");
  }

  assert.deepEqual((await load(["bin/old.dll"])).deletedFiles, ["bin/old.dll"], "честный патч принимается как есть");

  // Итоговой версии нет ни во встроенных, ни в сохранённых манифестах:
  // неподписанные удаления пропускаются, сам патч ставится.
  const unknown = await load(["bin/old.dll", "ghostingmod/SAVE/quick.sav"], "1.0.2");
  assert.deepEqual(unknown.deletedFiles, []);
  assert.equal(unknown.files.length, 1);
  assert.ok(warnings.some((line) => line.includes("пропущены")), warnings.join("\n"));
});

// Обновление 1.0.0 -> 1.0.1 через applyUpdate до PatchService. Манифест патча
// в форме build-patch.js (с patchManifestSha256) уже лежит в Downloads рядом
// с архивом; загрузка и 7-Zip подменены, сети нет.
async function patchUpdate(t, options) {
  const opts = options || {};
  const dir = tempDir(t);
  const previous = filesManifest("1.0.0", [
    entry("bin/old.dll", "old"),
    entry("bin/server.dll", "v1"),
    entry("ghostingmod/save/start.sav", "start"),
  ]);
  const resulting = filesManifest("1.0.1", [entry("bin/server.dll", "v2")]);
  const archiveName = `${GAME}-1.0.0-to-1.0.1.7z`;
  const archiveBytes = crypto.randomBytes(64);

  const base = manifests.MANIFESTS[GAME];
  const step = Object.assign({}, base.patches[0], {
    fromVersion: "1.0.0",
    toVersion: "1.0.1",
    archive: Object.assign({}, base.patches[0].archive, {
      filename: archiveName,
      size: archiveBytes.length,
      sha256: sha(archiveBytes),
    }),
    deletedFiles: opts.deletedFiles.length,
    resultingFilesManifestSha256: filesManifestLib.hashManifest(resulting),
  });
  const manifest = Object.assign({}, base, { version: "1.0.1", patches: [step] });
  delete manifest.buildMetadata;

  const payload = path.join(dir, "payload");
  fs.mkdirSync(path.join(payload, "payload/bin"), { recursive: true });
  fs.writeFileSync(path.join(payload, "payload/bin/server.dll"), "v2");

  const service = new GameDeliveryService({
    libraryPath: path.join(dir, "library"),
    launcherVersion: "0.4.9",
    env: { GAME_DELIVERY_REMOTE_ENABLED: "1" },
    manifests: { [GAME]: manifest },
    filesManifests: { [GAME]: opts.bothManifests ? { "1.0.0": previous, "1.0.1": resulting } : { "1.0.0": previous } },
    channelService: null,
    depotService: { assertOwns: async () => true },
    archiveService: {
      list: async () => ({ entries: [{ path: "payload/bin/server.dll", isDirectory: false }] }),
      extract: (archive, destination) => ({ promise: Promise.resolve(fs.cpSync(payload, destination, { recursive: true })) }),
    },
    updateCheckIntervalMs: 0,
    logger: { info() {}, warn() {}, error() {} },
  });
  t.after(() => service.dispose());

  const game = path.join(service.paths.games, GAME);
  for (const [relative, text] of [["bin/old.dll", "old"], ["bin/server.dll", "v1"], ["ghostingmod/save/start.sav", "start"], ["ghostingmod/SAVE/quick.sav", "мой сейв"]]) {
    fs.mkdirSync(path.dirname(path.join(game, relative)), { recursive: true });
    fs.writeFileSync(path.join(game, relative), text);
  }
  service.installService.writeInstalled({ gameId: GAME, manifest: { version: "1.0.0" }, installPath: game }, "a".repeat(64));

  const downloads = path.join(service.paths.downloads, GAME);
  const archivePath = path.join(downloads, archiveName);
  fs.mkdirSync(downloads, { recursive: true });
  fs.writeFileSync(archivePath, archiveBytes);
  service.downloadWithFallback = async () => ({ status: "completed", filePath: archivePath });

  const patchManifest = {
    schemaVersion: 1,
    gameId: GAME,
    fromVersion: "1.0.0",
    toVersion: "1.0.1",
    generatedAt: "2026-11-07T00:00:00.000Z",
    payloadRoot: "payload",
    files: [opts.files ? opts.files[0] : entry("bin/server.dll", "v2")],
    deletedFiles: opts.deletedFiles,
    archive: { filename: archiveName, size: archiveBytes.length, sha256: sha(archiveBytes) },
    resultingFilesManifestSha256: step.resultingFilesManifestSha256,
    resultingTotalFiles: resulting.totalFiles,
    resultingTotalSize: resulting.totalSize,
  };
  patchManifest.patchManifestSha256 = patchFormat.hashPatchManifest(patchManifest);
  const json = archivePath.replace(/\.7z$/, ".json");
  fs.writeFileSync(json, JSON.stringify(patchManifest, null, 2) + "\n");

  const read = (relative) => (fs.existsSync(path.join(game, relative)) ? fs.readFileSync(path.join(game, relative), "utf8") : null);
  return { service, read, json, update: () => service.applyUpdate(GAME) };
}

test("патч с удалениями ставится и без файлового манифеста новой версии: удаления пропущены, самохеш пересчитан", async (t) => {
  const env = await patchUpdate(t, { deletedFiles: ["bin/old.dll"] });
  const result = await env.update();
  assert.equal(result.status, "updated", JSON.stringify(result));
  assert.equal(env.service.installedVersionOf(GAME), "1.0.1");
  assert.equal(env.read("bin/server.dll"), "v2");
  assert.equal(env.read("bin/old.dll"), "old", "неподписанное удаление пропущено");
  assert.equal(env.read("ghostingmod/SAVE/quick.sav"), "мой сейв");
});

test("патч не удаляет пути из preservePaths, даже если прежняя сборка их поставляла", async (t) => {
  const env = await patchUpdate(t, { bothManifests: true, deletedFiles: ["bin/old.dll", "ghostingmod/save/start.sav"] });
  assert.equal((await env.update()).status, "updated");
  assert.equal(env.read("bin/old.dll"), null, "подписанное удаление выполнено");
  assert.equal(env.read("ghostingmod/save/start.sav"), "start", "ghostingmod/SAVE в preservePaths");
  assert.equal(env.read("bin/server.dll"), "v2");
});

test("манифест патча, прошедший сверку, но отвергнутый PatchService, не остаётся в Downloads", async (t) => {
  // files[] не совпадают с архивом: проверку шага это проходит, verifyPayload — нет.
  const env = await patchUpdate(t, { deletedFiles: [], files: [entry("bin/server.dll", "v3")] });
  await assert.rejects(env.update(), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(env.json), false, "повтор скачает манифест заново");
  assert.equal(env.service.installedVersionOf(GAME), "1.0.0");
  assert.equal(env.read("bin/server.dll"), "v1");
});

function channelManifest(version, publishedAt, privateKeyPem) {
  const provider = { id: "local", type: "local-http", status: "permanent", url: "http://127.0.0.1/x" };
  return signing.attach({
    schemaVersion: 1,
    gameId: GAME,
    channel: "stable",
    version,
    publishedAt,
    minimumLauncherVersion: "0.4.0",
    updateRequired: false,
    enabled: true,
    versionStatus: "assigned",
    fullArchive: { filename: `${GAME}-${version}.7z`, size: 1, sha256: "b".repeat(64), providers: [provider] },
    filesManifest: { version, sha256: "c".repeat(64), size: 1, filename: `${GAME}-${version}.files.json`, providers: [provider] },
    patches: [],
  }, privateKeyPem);
}

// Канал с loopback-сервера и временным ключом; channel() — новый ChannelService
// с тем же файлом свежести, как после перезапуска лаунчера.
async function channelFixture(t) {
  const dir = tempDir(t);
  const keys = signing.generateKeyPair();
  const served = { body: null };
  const server = http.createServer((req, res) => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(served.body));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());

  const url = `http://127.0.0.1:${server.address().port}/stable.json`;
  const freshnessFile = path.join(dir, "userData", "game-channel-freshness.json");
  const channel = (logger) => {
    const service = new ChannelService({ freshnessFile, logger, maxAttempts: 1, backoffBaseMs: 0 });
    service.configFor = () => ({ url, channel: "stable", requireSignature: true, publicKeys: [keys.publicKeyPem] });
    return service;
  };
  return { keys, served, channel, freshnessFile };
}

test("канал: повтор старого подписанного манифеста не принимается, откат — новой подписью", async (t) => {
  const { keys, served, channel } = await channelFixture(t);

  const old = channelManifest("1.0.11", "2026-10-31T10:00:00.000Z", keys.privateKeyPem);
  const current = channelManifest("1.0.12", "2026-11-01T10:00:00.000Z", keys.privateKeyPem);

  served.body = current;
  assert.equal((await channel().fetch(GAME, { ignoreCache: true })).manifest.version, "1.0.12");

  served.body = old;
  await assert.rejects(channel().fetch(GAME, { ignoreCache: true }), { code: "MANIFEST_INVALID" }, "после перезапуска тоже");

  // Владелец откатывает выпуск: прежний манифест подписан заново, дата новее.
  served.body = channelManifest("1.0.11", "2026-11-02T10:00:00.000Z", keys.privateKeyPem);
  assert.equal((await channel().fetch(GAME, { ignoreCache: true })).manifest.version, "1.0.11");

  served.body = current;
  await assert.rejects(channel().fetch(GAME, { ignoreCache: true }), { code: "MANIFEST_INVALID" }, "откат перекрыл прежний выпуск");
});

test("канал: испорченный файл свежести считается пустым и перезаписывается", async (t) => {
  const { keys, served, channel, freshnessFile } = await channelFixture(t);
  served.body = channelManifest("1.0.12", "2026-11-01T10:00:00.000Z", keys.privateKeyPem);
  fs.mkdirSync(path.dirname(freshnessFile), { recursive: true });

  for (const broken of ["5", '"x"', "true", "[]", "{мусор"]) {
    fs.writeFileSync(freshnessFile, broken);
    assert.equal((await channel().fetch(GAME, { ignoreCache: true })).manifest.version, "1.0.12", broken);
    assert.deepEqual(Object.values(JSON.parse(fs.readFileSync(freshnessFile, "utf8"))), ["2026-11-01T10:00:00.000Z"], broken);
  }
});

test("канал: publishedAt из будущего принимается, но свежесть не сдвигает; предупреждение одно на дату", async (t) => {
  const { keys, served, channel, freshnessFile } = await channelFixture(t);
  served.body = channelManifest("1.0.12", "2026-11-01T10:00:00.000Z", keys.privateKeyPem);
  await channel().fetch(GAME, { ignoreCache: true });

  // Владелец подписал с убежавшими вперёд часами.
  const warnings = [];
  const service = channel({ info() {}, warn: (line) => warnings.push(line), error() {} });
  served.body = channelManifest("1.0.13", "2030-01-01T00:00:00.000Z", keys.privateKeyPem);
  for (let check = 0; check < 3; check++)
    assert.equal((await service.fetch(GAME, { ignoreCache: true })).manifest.version, "1.0.13");
  assert.equal(warnings.filter((line) => line.includes("из будущего")).length, 1, warnings.join("\n"));

  served.body = channelManifest("1.0.13", "2026-11-02T10:00:00.000Z", keys.privateKeyPem);
  assert.equal((await channel().fetch(GAME, { ignoreCache: true })).manifest.version, "1.0.13", "честная переподпись принимается");
  assert.deepEqual(Object.values(JSON.parse(fs.readFileSync(freshnessFile, "utf8"))), ["2026-11-02T10:00:00.000Z"]);
});

test("ошибка удаления отвергнутого манифеста патча не подменяет ошибку патча", async (t) => {
  const rmSync = fs.rmSync;
  fs.rmSync = (target, options) => {
    if (String(target).endsWith(".json")) throw Object.assign(new Error("EBUSY: занят"), { code: "EBUSY" });
    return rmSync(target, options);
  };
  t.after(() => { fs.rmSync = rmSync; });

  // Отказ в PatchService (files[] не совпадают с архивом).
  const payload = await patchUpdate(t, { deletedFiles: [], files: [entry("bin/server.dll", "v3")] });
  await assert.rejects(payload.update(), { code: "CHECKSUM_MISMATCH" });

  // Отказ на сверке с подписанными файловыми манифестами.
  const forged = await patchUpdate(t, { bothManifests: true, deletedFiles: ["ghostingmod/SAVE/quick.sav"] });
  await assert.rejects(forged.update(), { code: "MANIFEST_INVALID" });
  assert.equal(forged.read("ghostingmod/SAVE/quick.sav"), "мой сейв");
});