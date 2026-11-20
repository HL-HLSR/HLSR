"use strict";

// Тренировочные сейвы HL1 (src/main/game-delivery/practice-saves.js):
// докачка недостающих в SAVE мода и языковых папок (valve_WON_russian),
// сейвы игрока не перезаписываются и не перекрываются, архив проверяется по
// размеру и SHA-256.

const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { Readable } = require("stream");

const ROOT = path.resolve(__dirname, "..");
const PracticeSaves = require(path.join(ROOT, "src/main/game-delivery/practice-saves.js"));
const ArchiveService = require(path.join(ROOT, "src/main/game-delivery/ArchiveService.js"));
const endpoints = require(path.join(ROOT, "src/shared/delivery-endpoints.js"));
const HL1_PACK = require(path.join(ROOT, "config/saves/hl1-practice-saves.manifest.json"));
const RELEASE_ARCHIVE = path.join(ROOT, "release/saves", HL1_PACK.archive.filename);

const MOD = "Half-Life/valve_WON";
const TARGET = `${MOD}/SAVE`;
const RUSSIAN = "Half-Life/valve_WON_russian";
const PACK_FILES = { "e1.sav": "pack-e1", "e2.sav": "pack-e2", "e14-protopipe.sav": "pack-proto" };
const ARCHIVE_BYTES = Buffer.from("fake 7z archive");

function sha256(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex");
}

function fakePack(overrides) {
  return Object.assign(
    {
      packId: "test-saves",
      gameIds: ["hl1-won"],
      target: TARGET,
      files: Object.entries(PACK_FILES).map(([name, content]) => ({
        name,
        size: Buffer.byteLength(content),
        sha256: sha256(Buffer.from(content)),
      })),
      archive: {
        filename: "test-saves.7z",
        size: ARCHIVE_BYTES.length,
        sha256: sha256(ARCHIVE_BYTES),
        providers: [{ id: "test", url: "https://example.invalid/test-saves.7z" }],
      },
    },
    overrides || {}
  );
}

function fakeHttp(body) {
  const calls = [];
  return {
    calls,
    async request(url) {
      calls.push(url);
      const response = Readable.from([body || ARCHIVE_BYTES]);
      response.statusCode = 200;
      return response;
    },
  };
}

// Распаковщик-заглушка: «распаковывает» содержимое PACK_FILES.
function fakeArchive() {
  return {
    extract(archivePath, destination) {
      for (const [name, content] of Object.entries(PACK_FILES))
        fs.writeFileSync(path.join(destination, name), content);
      return { promise: Promise.resolve({ destination }), cancel() {} };
    },
  };
}

function sandbox(t, { modDir = true } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-saves-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));

  const paths = { root, downloads: path.join(root, "downloads"), staging: path.join(root, "staging") };
  const installPath = path.join(root, "games", "hl1-won");
  if (modDir) fs.mkdirSync(path.join(installPath, MOD), { recursive: true });

  return {
    root,
    paths,
    installPath,
    saveDir: path.join(installPath, TARGET),
    russianSave: path.join(installPath, RUSSIAN, "SAVE"),
  };
}

function savesOf(dir) {
  return fs.readdirSync(dir).sort();
}

test("все сейвы на месте — сеть не трогается", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(box.saveDir);
  for (const name of Object.keys(PACK_FILES)) fs.writeFileSync(path.join(box.saveDir, name.toUpperCase()), "x");

  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "present", added: 0 });
  assert.equal(http.calls.length, 0);
});

test("недостающие докачиваются, свои сейвы игрока не перезаписываются", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(box.saveDir);
  fs.writeFileSync(path.join(box.saveDir, "e1.sav"), "mine");

  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 2 });
  assert.equal(http.calls.length, 1);
  assert.equal(fs.readFileSync(path.join(box.saveDir, "e1.sav"), "utf8"), "mine");
  assert.equal(fs.readFileSync(path.join(box.saveDir, "e2.sav"), "utf8"), "pack-e2");
  assert.equal(fs.readFileSync(path.join(box.saveDir, "e14-protopipe.sav"), "utf8"), "pack-proto");
  assert.deepEqual(fs.readdirSync(box.paths.downloads), [], "архив и .part удалены");
  assert.equal(fs.existsSync(path.join(box.paths.staging, "saves-test-saves")), false, "staging убран");
});

test("папки SAVE нет — создаётся и заполняется", async (t) => {
  const box = sandbox(t);
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(), packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 3 });
  assert.deepEqual(savesOf(box.saveDir), Object.keys(PACK_FILES).sort());
});

test("папка save в другом регистре — сейвы кладутся в неё, вторая не создаётся", async (t) => {
  const box = sandbox(t);
  const lower = path.join(box.installPath, MOD, "save");
  fs.mkdirSync(lower);

  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(), packs: [fakePack()] });
  await saves.ensure("hl1-won", box.installPath);

  assert.deepEqual(fs.readdirSync(path.join(box.installPath, MOD)), ["save"]);
  assert.equal(fs.readdirSync(lower).length, 3);
});

test("русская папка мода — сейвы ложатся и в valve_WON, и в valve_WON_russian", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(path.join(box.installPath, RUSSIAN));

  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 6 });
  assert.equal(http.calls.length, 1);
  assert.deepEqual(savesOf(box.saveDir), Object.keys(PACK_FILES).sort());
  assert.deepEqual(savesOf(box.russianSave), Object.keys(PACK_FILES).sort());
  assert.equal(fs.readFileSync(path.join(box.russianSave, "e2.sav"), "utf8"), "pack-e2");
  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "present", added: 0 });
});

test("русская папка появилась после установки — сейвы копируются из valve_WON без загрузки", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(box.saveDir);
  for (const [name, content] of Object.entries(PACK_FILES)) fs.writeFileSync(path.join(box.saveDir, name), content);
  fs.mkdirSync(path.join(box.installPath, RUSSIAN, "SAVE"), { recursive: true });

  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 3 });
  assert.equal(http.calls.length, 0);
  assert.deepEqual(savesOf(box.russianSave), Object.keys(PACK_FILES).sort());
});

test("свой сейв игрока в valve_WON не перекрывается сейвом набора из русской папки", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(box.saveDir);
  fs.writeFileSync(path.join(box.saveDir, "E1.SAV"), "mine");
  fs.mkdirSync(path.join(box.installPath, RUSSIAN));

  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 4 });
  assert.equal(fs.readFileSync(path.join(box.saveDir, "E1.SAV"), "utf8"), "mine");
  assert.deepEqual(savesOf(box.russianSave), ["e14-protopipe.sav", "e2.sav"]);

  // Пропущенное имя не считается недостающим: архив не качается при каждом запуске.
  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "present", added: 0 });
  assert.equal(http.calls.length, 1);
});

test("свой сейв в русской папке не перезаписывается", async (t) => {
  const box = sandbox(t);
  fs.mkdirSync(box.russianSave, { recursive: true });
  fs.writeFileSync(path.join(box.russianSave, "e2.sav"), "mine-ru");

  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(), packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 5 });
  assert.equal(fs.readFileSync(path.join(box.russianSave, "e2.sav"), "utf8"), "mine-ru");
  assert.equal(fs.readFileSync(path.join(box.saveDir, "e2.sav"), "utf8"), "pack-e2");
});

test("папки valve_WON_hd, valve_WON_downloads и прочие не языковые — не трогаются", async (t) => {
  const box = sandbox(t);
  for (const name of ["valve_WON_hd", "valve_WON_downloads", "valve_WON_backup"])
    fs.mkdirSync(path.join(box.installPath, "Half-Life", name));

  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(), packs: [fakePack()] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 3 });
  for (const name of ["valve_WON_hd", "valve_WON_downloads", "valve_WON_backup"])
    assert.deepEqual(fs.readdirSync(path.join(box.installPath, "Half-Life", name)), []);
});

test("игра не установлена (нет папки мода) — ничего не качается и не создаётся", async (t) => {
  const box = sandbox(t, { modDir: false });
  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  assert.equal((await saves.ensure("hl1-won", box.installPath)).status, "not-installed");
  assert.equal(http.calls.length, 0);
  assert.equal(fs.existsSync(box.installPath), false);
});

test("чужая игра — not-applicable", async (t) => {
  const box = sandbox(t);
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(), packs: [fakePack()] });

  assert.equal((await saves.ensure("hl2-4260", box.installPath)).status, "not-applicable");
});

test("SHA-256 не совпал — отказ, сейвы не появились, .part удалён", async (t) => {
  const box = sandbox(t);
  const tampered = Buffer.from("tampered 7z bit");
  assert.equal(tampered.length, ARCHIVE_BYTES.length);

  const saves = new PracticeSaves({
    paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(tampered), packs: [fakePack()],
  });

  await assert.rejects(saves.ensure("hl1-won", box.installPath), { code: "CHECKSUM_MISMATCH" });
  assert.equal(fs.existsSync(box.saveDir), false);
  assert.deepEqual(fs.readdirSync(box.paths.downloads), []);
});

test("ответ длиннее объявленного — отказ по размеру", async (t) => {
  const box = sandbox(t);
  const saves = new PracticeSaves({
    paths: box.paths, archive: fakeArchive(), httpClient: fakeHttp(Buffer.alloc(ARCHIVE_BYTES.length + 1)), packs: [fakePack()],
  });

  await assert.rejects(saves.ensure("hl1-won", box.installPath), { code: "SIZE_MISMATCH" });
});

test("одновременные вызовы — одна загрузка", async (t) => {
  const box = sandbox(t);
  const http = fakeHttp();
  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [fakePack()] });

  const [first, second] = await Promise.all([
    saves.ensure("hl1-won", box.installPath),
    saves.ensure("hl1-won", box.installPath),
  ]);

  assert.equal(http.calls.length, 1);
  assert.equal(first, second);
  assert.equal(saves.jobs.size, 0);
});

test("второй источник подхватывает, если первый не ответил", async (t) => {
  const box = sandbox(t);
  const http = fakeHttp();
  const request = http.request;
  http.request = async (url) => {
    if (url.includes("primary")) {
      http.calls.push(url);
      const response = Readable.from([]);
      response.statusCode = 404;
      return response;
    }
    return request(url);
  };

  const pack = fakePack();
  pack.archive = Object.assign({}, pack.archive, {
    providers: [
      { id: "hlsr-primary", url: "https://example.invalid/primary/test-saves.7z" },
      { id: "github-fallback", url: "https://example.invalid/fallback/test-saves.7z" },
    ],
  });

  const saves = new PracticeSaves({ paths: box.paths, archive: fakeArchive(), httpClient: http, packs: [pack] });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 3 });
  assert.deepEqual(http.calls, [
    "https://example.invalid/primary/test-saves.7z",
    "https://example.invalid/fallback/test-saves.7z",
  ]);
});

test("манифест HL1: имена сейвов, архив и источники корректны", () => {
  assert.deepEqual(HL1_PACK.gameIds, ["hl1-won"]);
  assert.equal(HL1_PACK.target, TARGET);
  assert.equal(HL1_PACK.files.length, 63);
  assert.equal(new Set(HL1_PACK.files.map((file) => file.name.toLowerCase())).size, HL1_PACK.files.length);
  for (const file of HL1_PACK.files) {
    assert.match(file.name, PracticeSaves.SAVE_NAME);
    assert.ok(file.size > 0, file.name);
    assert.match(file.sha256, /^[0-9a-f]{64}$/, file.name);
  }
  assert.ok(HL1_PACK.files.some((file) => file.name === "e14-protopipe.sav"));

  assert.match(HL1_PACK.archive.sha256, /^[0-9a-f]{64}$/);
  assert.ok(HL1_PACK.archive.size > 0);
  assert.deepEqual(HL1_PACK.archive.providers.map((provider) => provider.id), ["hlsr-primary", "github-fallback"]);
  for (const provider of HL1_PACK.archive.providers)
    assert.ok(endpoints.isAllowedDownloadUrl(provider.url), provider.url);
});

// Настоящий архив из release/saves (лежит у того, кто его собирал) через
// настоящий 7za: в нём ровно сейвы манифеста с их SHA-256, и они доезжают
// до SAVE мода и русской папки.
test("настоящий архив распаковывается 7za и совпадает с манифестом", { skip: !fs.existsSync(RELEASE_ARCHIVE) }, async (t) => {
  const bytes = fs.readFileSync(RELEASE_ARCHIVE);
  assert.equal(bytes.length, HL1_PACK.archive.size);
  assert.equal(sha256(bytes), HL1_PACK.archive.sha256);

  const box = sandbox(t);
  fs.mkdirSync(path.join(box.installPath, RUSSIAN));
  const archive = new ArchiveService({ binPath: require("7zip-bin").path7za });
  const saves = new PracticeSaves({ paths: box.paths, archive, httpClient: fakeHttp(bytes) });

  assert.deepEqual(await saves.ensure("hl1-won", box.installPath), { status: "installed", added: 126 });
  const names = HL1_PACK.files.map((file) => file.name).sort();
  assert.deepEqual(savesOf(box.saveDir), names);
  assert.deepEqual(savesOf(box.russianSave), names);
  for (const file of HL1_PACK.files) {
    const data = fs.readFileSync(path.join(box.saveDir, file.name));
    assert.equal(data.subarray(0, 4).toString("latin1"), "JSAV", file.name);
    assert.equal(sha256(data), file.sha256, file.name);
  }
});
