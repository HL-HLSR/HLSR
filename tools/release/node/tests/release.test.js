"use strict";

// Тесты Node-скриптов выпуска (tools/release/node): версия X.Y.Z, место ключей, ключи аттестации,
// ченджлог, блок build канала, лента QA. Сквозной выпуск (упаковка → подпись → приёмка 0.4.7) —
// tools/release/tests/release_flow.rs. Запуск: node --test tools/release/node/tests/*.test.js

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const http = require("http");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const common = require("../lib/release-common");
const attestKeys = require("../lib/attest-keys");
const changelog = require("../lib/launcher-changelog");
const launcherChunks = require("../lib/launcher-chunks");
const frozen = require("../lib/frozen");
const verifyRelease = require("../verify-launcher-release");
const prepare = require("../launcher-release-prepare");
const feed = require("../qa-launcher-feed");
const qaBuild = require("../qa-launcher-build");

function temp(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-release-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("версия выпуска — только X.Y.Z, Cargo.toml читается без чужих разделов", (t) => {
  for (const good of ["0.5.0", "1.20.300"]) assert.equal(common.requireReleaseVersion(good), good);
  for (const bad of ["0.5.0-dev", "0.5.0-beta.1", "0.5", "v0.5.0", "", undefined, "0.5.0 "])
    assert.throws(() => common.requireReleaseVersion(bad), /только X\.Y\.Z/, String(bad));
  const dir = temp(t);
  const manifest = path.join(dir, "Cargo.toml");
  fs.writeFileSync(manifest, '[package]\nversion = "9.9.9"\n\n[workspace.package]\nedition = "2024"\nversion = "0.5.1"\n\n[dependencies]\nversion = "7"\n');
  assert.equal(common.workspaceVersion(manifest), "0.5.1");
  fs.writeFileSync(manifest, '[package]\nversion = "9.9.9"\n');
  assert.throws(() => common.workspaceVersion(manifest), /нет version/);
  assert.match(common.workspaceVersion(), /^\d+\.\d+\.\d+/);
});

test("приватный ключ канала: только абсолютный путь вне репозитория, Downloads и выхода сборки", () => {
  assert.throws(() => common.assertPrivateKeyPathAllowed(undefined), /Не задан/);
  assert.throws(() => common.assertPrivateKeyPathAllowed("keys/channel.pem"), /абсолютным/);
  for (const inside of [
    path.join(common.REPO_ROOT, "channel.pem"),
    path.join(common.REPO_ROOT, "target", "dist", "channel.pem"),
    path.join(common.REPO_ROOT, "release", "channel.pem"),
    path.join(os.homedir(), "Downloads", "channel.pem"),
  ])
    assert.throws(() => common.assertPrivateKeyPathAllowed(inside), /нельзя хранить/, inside);
  const outside = path.join(os.tmpdir(), "hlsr-keys", "channel.pem");
  assert.equal(common.assertPrivateKeyPathAllowed(outside), path.resolve(outside));
  assert.throws(() => common.loadPrivateKey({ HLSR_MANIFEST_PRIVATE_KEY_PATH: outside }), /не найден/);
});

test("ключи аттестации: выпуск берёт готовый ключ, чужая пара и каталог в репозитории — отказ", (t) => {
  const dir = temp(t);
  const env = { HLSR_ATTEST_KEYS_DIR: dir };
  assert.throws(() => attestKeys.load({ platform: "win32", version: "0.5.0", env }), /Нет ключа аттестации/);
  const created = attestKeys.loadOrCreate({ platform: "win32", version: "0.5.0", env });
  assert.equal(created.publicKey, attestKeys.publicKeyOf(created.seed));
  // Повторная сборка той же версии не меняет ключ.
  assert.equal(attestKeys.loadOrCreate({ platform: "win32", version: "0.5.0", env }).seed, created.seed);
  assert.deepEqual(attestKeys.attestationFor({ platform: "win32", version: "0.5.0", env }), { v: 1, publicKey: created.publicKey });
  assert.throws(() => attestKeys.load({ platform: "linux", version: "0.5.0", env }), /Нет ключа/);
  assert.throws(() => attestKeys.fileFor("macos", "0.5.0", env), /Неизвестная платформа/);
  assert.throws(() => attestKeys.fileFor("win32", "latest", env), /Некорректная версия/);

  const file = attestKeys.fileFor("win32", "0.5.0", env);
  const record = JSON.parse(fs.readFileSync(file, "utf8"));
  fs.writeFileSync(file, JSON.stringify({ ...record, publicKey: attestKeys.generate().publicKey }));
  assert.throws(() => attestKeys.load({ platform: "win32", version: "0.5.0", env }), /не получается из seed/);
  fs.writeFileSync(file, JSON.stringify({ ...record, version: "0.5.1" }));
  assert.throws(() => attestKeys.load({ platform: "win32", version: "0.5.0", env }), /повреждён/);

  assert.throws(() => attestKeys.keysDir({ HLSR_ATTEST_KEYS_DIR: path.join(common.REPO_ROOT, "keys") }), /нельзя хранить/);
  assert.equal(attestKeys.disabled({ HLSR_ATTEST_DISABLE: "1" }, "win32"), true);
  assert.equal(attestKeys.disabled({}, "win32"), false);
  assert.equal(attestKeys.disabled({}, "freebsd"), true);
  assert.deepEqual(attestKeys.attestProblems({ v: 1, publicKey: created.publicKey }), []);
  assert.equal(attestKeys.attestProblems({ v: 2 }).length, 1);
  assert.match(attestKeys.attestProblems({ v: 1, publicKey: "abc" })[0], /32 байта/);
});

test("ченджлог выпуска: запись обязательна, обе локали и поровну пунктов", (t) => {
  const dir = temp(t);
  const file = path.join(dir, "changelog.json");
  fs.writeFileSync(file, JSON.stringify({ releases: [
    { version: "0.5.0", ru: [" Первый ", "Второй"], en: ["First", "Second"] },
    { version: "0.5.1", ru: ["Один"], en: [] },
    { version: "0.5.2", ru: ["Один", "Два"], en: ["One"] },
  ] }));
  const built = changelog.buildChangelogJson("0.5.0", "2026-10-16T09:00:00.000Z", file);
  assert.deepEqual(built, { schemaVersion: 1, version: "0.5.0", generatedAt: "2026-10-16T09:00:00.000Z", ru: ["Первый", "Второй"], en: ["First", "Second"] });
  assert.throws(() => changelog.buildChangelogJson("0.5.9", null, file), /нет записи для 0\.5\.9/);
  assert.throws(() => changelog.buildChangelogJson("0.5.1", null, file), /пустой ченджлог на языке «en»/);
  assert.throws(() => changelog.buildChangelogJson("0.5.2", null, file), /разное число пунктов/);
  // Источник по умолчанию — данные интерфейса 0.5: там есть выпуск 0.4.7 эталона.
  assert.equal(changelog.buildChangelogJson("0.4.7").version, "0.4.7");
});

test("блок build канала: исполняемый файл с кусками как у describeTree и ключ аттестации", (t) => {
  const dir = temp(t);
  const tree = path.join(dir, "win-unpacked");
  fs.mkdirSync(path.join(tree, "resources"), { recursive: true });
  // 600 КиБ псевдослучайных байт: несколько кусков по 16–256 КиБ.
  const data = Buffer.alloc(600 * 1024);
  let state = 0x9e3779b9;
  for (let i = 0; i < data.length; i++) {
    state ^= state << 13; state >>>= 0; state ^= state >>> 17; state ^= state << 5; state >>>= 0;
    data[i] = state & 0xff;
  }
  fs.writeFileSync(path.join(tree, "HLSR.exe"), data);
  fs.writeFileSync(path.join(tree, "resources", "install-files.json"), JSON.stringify({ version: "0.5.0" }));
  const env = { HLSR_ATTEST_KEYS_DIR: path.join(dir, "attest") };
  const record = attestKeys.loadOrCreate({ platform: "win32", version: "0.5.0", env });

  const block = launcherChunks.buildBlock({ version: "0.5.0", platform: "win32", tree, env });
  assert.deepEqual(Object.keys(block), ["executable", "attest"]);
  assert.deepEqual(block.attest, { v: 1, publicKey: record.publicKey });
  assert.equal(block.executable.path, "HLSR.exe");
  assert.equal(block.executable.size, data.length);
  assert.equal(block.executable.sha256, crypto.createHash("sha256").update(data).digest("hex"));
  assert.ok(block.executable.chunks.length > 2);
  assert.equal(block.executable.chunks.reduce((sum, chunk) => sum + chunk[1], 0), data.length);
  // Те же куски, что кладёт в манифест версии describeTree (границы, хеши, размеры).
  const manifest = frozen.chunks.describeTree(tree, { version: "0.5.0", platform: "win32" });
  const entry = manifest.files.find((file) => file.path === "HLSR.exe");
  assert.deepEqual(block.executable.chunks.map(([hash, size]) => [hash, size]), entry.chunks);
  for (const [, size, packed] of block.executable.chunks) assert.ok(packed > 0 && packed <= size + 4096);

  // Выпуск без аттестации — только по явному HLSR_ATTEST_DISABLE=1.
  const none = launcherChunks.buildBlock({ version: "0.5.0", platform: "win32", tree, env: { ...env, HLSR_ATTEST_DISABLE: "1" } });
  assert.deepEqual(Object.keys(none), ["executable"]);
  assert.throws(() => launcherChunks.buildBlock({ version: "0.5.0", platform: "win32", tree: path.join(dir, "missing"), env }), /нет HLSR\.exe/);
  assert.throws(() => launcherChunks.assertTreeVersion(tree, "0.5.1", "win32"), /описывает 0\.5\.0, а выпускается 0\.5\.1/);
});

test("verify-launcher-release: блок build проверяется по форме", () => {
  const good = { executable: { path: "HLSR.exe", size: 70000, sha256: "a".repeat(64), chunks: [["b".repeat(64), 30000, 100], ["c".repeat(64), 40000, 100]] }, attest: { v: 1, publicKey: "d".repeat(64) } };
  assert.deepEqual(verifyRelease.buildBlockProblems(good), []);
  assert.deepEqual(verifyRelease.buildBlockProblems({ executable: good.executable }), []);
  assert.match(verifyRelease.buildBlockProblems(undefined)[0], /нет блока build/);
  assert.match(verifyRelease.buildBlockProblems({ ...good, executable: { ...good.executable, path: "hlsr" } }).join(), /ожидался HLSR\.exe/);
  assert.match(verifyRelease.buildBlockProblems({ ...good, executable: { ...good.executable, size: 70001 } }).join(), /дают 70000 байт/);
  assert.match(verifyRelease.buildBlockProblems({ ...good, executable: { ...good.executable, sha256: "x" } }).join(), /sha256/);
  assert.match(verifyRelease.buildBlockProblems({ ...good, executable: { ...good.executable, chunks: [] } }).join(), /chunks пуст/);
  assert.match(verifyRelease.buildBlockProblems({ ...good, attest: { v: 2 } }).join(), /build\.attest/);
});

test("аргументы скриптов: незнакомые отвергаются, QA-сборка получает своё удостоверение", () => {
  assert.throws(() => prepare.parseArgs(["--blockmap", "x"]), /Неизвестный аргумент: --blockmap/);
  assert.equal(prepare.parseArgs(["--test-public-key", "k.pem", "--no-chunks", "--chunk-store", "s"]).chunkStore, "s");
  assert.throws(() => qaBuild.parseArgs(["--publish"]), /Неизвестный аргумент/);
  assert.deepEqual(qaBuild.parseArgs(["--version", "0.5.0", "--skip-bundle", "--exe", "x.exe"]), { version: "0.5.0", skipBundle: true, exe: "x.exe" });
  assert.equal(qaBuild.QA_APP_ID, "com.hlsrteam.hlsr.qa");
  assert.equal(qaBuild.QA_PRODUCT, "HLSR-QA");
  // Тот же GUID, что считает layout.js 0.4.7 для appId QA-сборки (решение 18).
  const layout = frozen.load("main/windows-install/layout.js");
  assert.equal(layout.guidFor("com.hlsrteam.hlsr"), "3f824379-6b41-5725-b424-59cb87f2acae");
  assert.notEqual(layout.guidFor(qaBuild.QA_APP_ID), layout.guidFor("com.hlsrteam.hlsr"));
});

test("лента QA: только loopback-имена без подкаталогов, Range и намеренные поломки", async (t) => {
  const dir = temp(t);
  fs.writeFileSync(path.join(dir, "HLSR-Setup-0.5.0.exe"), Buffer.from("0123456789"));
  fs.writeFileSync(path.join(dir, "HLSR-Setup-0.5.0-linux-x86_64.AppImage"), Buffer.from("abcdefghij"));
  fs.writeFileSync(path.join(dir, "latest.yml"), "version: 0.5.0\n");
  fs.writeFileSync(path.join(dir, "launcher-channel.json"), "{}");
  const serve = async (faults) => {
    const server = feed.createServer({ dir, faults });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    t.after(() => server.close());
    return `http://127.0.0.1:${server.address().port}`;
  };
  const get = (url, headers) => new Promise((resolve, reject) => {
    http.get(url, { headers }, (response) => {
      const parts = [];
      response.on("data", (part) => parts.push(part));
      response.on("end", () => resolve({ status: response.statusCode, body: Buffer.concat(parts), headers: response.headers }));
    }).on("error", reject);
  });
  const base = await serve({});
  assert.equal((await get(`${base}/HLSR-Setup-0.5.0.exe`)).body.toString(), "0123456789");
  const part = await get(`${base}/HLSR-Setup-0.5.0.exe`, { range: "bytes=4-" });
  assert.equal(part.status, 206);
  assert.equal(part.body.toString(), "456789");
  assert.equal(part.headers["content-range"], "bytes 4-9/10");
  assert.equal((await get(`${base}/..%2Fsecret`)).status, 404);
  assert.equal((await get(`${base}/nope.exe`)).status, 404);
  const broken = await serve({ installer: true, yml: true, noChannel: true });
  assert.notEqual((await get(`${broken}/HLSR-Setup-0.5.0.exe`)).body.toString(), "0123456789");
  assert.notEqual((await get(`${broken}/HLSR-Setup-0.5.0-linux-x86_64.AppImage`)).body.toString(), "abcdefghij");
  assert.match((await get(`${broken}/latest.yml`)).body.toString(), /# broken/);
  assert.equal((await get(`${broken}/launcher-channel.json`)).status, 404);
});
