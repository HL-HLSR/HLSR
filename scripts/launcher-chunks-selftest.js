"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), os = require("os"), path = require("path"), crypto = require("crypto"), vm = require("vm");
const { execFileSync } = require("child_process");
const { createRequire } = require("module");
const { EventEmitter } = require("events");
const chunks = require("../src/main/launcher-update/chunks");
const channel = require("../src/main/launcher-update/launcher-channel");
const signing = require("../src/main/game-delivery/manifest-signing");
const Gate = require("../src/main/launcher-update/LauncherUpdateGate");
const Install = require("../src/main/launcher-update/ArchiveInstall");
const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
const UpdateSession = require("../src/updater/core/session");
const { prepare } = require("./launcher-release-prepare-linux");
const { NO_EXEC_BIT, NOT_LINUX } = require("./lib/selftest-caps");

// Здесь проверяется механика чанков, а не аттестация (scripts/attestation-selftest.js):
// в синтетических деревьях app.asar — просто байты без маркера ключа сборки.
process.env.HLSR_ATTEST_DISABLE = "1";

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-chunks-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function write(root, name, data, mode) {
  const file = path.join(root, name);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data, mode ? { mode } : undefined);
}

// Сборка Linux: бинарник, большой app.asar, библиотека и мелочь. Во второй
// версии app.asar правится посередине, библиотека не меняется, один файл
// исчезает, другой появляется.
function base() {
  const elf = Buffer.concat([Buffer.alloc(64), crypto.randomBytes(600 * 1024)]);
  Buffer.from("7f454c46020101", "hex").copy(elf);
  elf.writeUInt16LE(62, 18);
  return { elf, asar: crypto.randomBytes(1536 * 1024), lib: crypto.randomBytes(400 * 1024) };
}

function tree(root, version, parts, next) {
  write(root, "hlsr", parts.elf, 0o755);
  write(root, "libffmpeg.so", parts.lib, 0o755);
  write(root, "resources/icons/hlsr.png", "icon");
  write(root, ".hlsr-release.json", JSON.stringify({ version, platform: "linux", arch: "x64", executable: "hlsr" }) + "\n");
  if (!next) {
    write(root, "resources/app.asar", parts.asar);
    write(root, "resources/old.txt", "только в старой версии");
    return root;
  }
  const asar = Buffer.from(parts.asar);
  crypto.randomBytes(300).copy(asar, 700 * 1024);
  write(root, "resources/app.asar", Buffer.concat([asar, Buffer.from("хвост новой версии")]));
  write(root, "resources/new.txt", "только в новой версии");
  return root;
}

function files(root) {
  const out = {};
  const walk = (relative) => {
    for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(child);
      else out[child] = crypto.createHash("sha256").update(fs.readFileSync(path.join(root, child))).digest("hex");
    }
  };
  walk("");
  return out;
}

// Сервер hlsr.pro из локального хранилища: манифест по имени, куски по хешу.
function server(store, manifests, log = []) {
  return async (url, options) => {
    log.push(url);
    const name = url.split("/").pop();
    let body;
    if (manifests[name]) body = manifests[name];
    else {
      const file = path.join(store, name.slice(0, 2), name);
      if (!fs.existsSync(file)) throw new Error(`HTTP 404: ${url}`);
      body = fs.readFileSync(file);
    }
    if (body.length > options.maxBytes) throw new Error("слишком большой ответ");
    return body;
  };
}

const allowAll = (url) => /^https:\/\/hlsr\.pro\//.test(url);

test("нарезка: куски покрывают файл, укладываются в пределы и не зависят от сдвига", () => {
  const data = crypto.randomBytes(3 * 1024 * 1024);
  const pieces = chunks.cut(data);
  assert.equal(pieces[0][0], 0);
  assert.equal(pieces[pieces.length - 1][1], data.length);
  for (let i = 0; i < pieces.length; i++) {
    const size = pieces[i][1] - pieces[i][0];
    if (i > 0) assert.equal(pieces[i][0], pieces[i - 1][1]);
    assert.ok(size <= chunks.MAX_CHUNK);
    if (i < pieces.length - 1) assert.ok(size >= chunks.MIN_CHUNK);
  }
  assert.deepEqual(chunks.cut(data), pieces);
  // Вставка в начало меняет только первые куски: остальные находятся снова.
  const shifted = Buffer.concat([Buffer.from("вставка"), data]);
  const hashes = new Set(pieces.map(([a, b]) => chunks.sha256(data.subarray(a, b))));
  const again = chunks.cut(shifted).map(([a, b]) => chunks.sha256(shifted.subarray(a, b)));
  assert.ok(again.filter((hash) => hashes.has(hash)).length >= pieces.length - 2);
});

test("сборка версии: совпавшее берётся с диска, качаются только изменённые куски", async (t) => {
  const dir = tempDir(t), store = path.join(dir, "store"), parts = base();
  const old = tree(path.join(dir, "old"), "0.4.3", parts);
  const next = tree(path.join(dir, "next"), "0.4.4", parts, true);
  chunks.buildStore(old, store, { version: "0.4.3", platform: "linux" });
  const { manifest } = chunks.buildStore(next, store, { version: "0.4.4", platform: "linux" });
  assert.deepEqual(chunks.problemsOf(manifest, { version: "0.4.4", platform: "linux" }), []);
  // Бит исполнения берётся из файлов дерева; на Windows его у них нет.
  if (!NO_EXEC_BIT) assert.equal(manifest.files.find((file) => file.path === "hlsr").exec, true);
  assert.equal(manifest.files.find((file) => file.path === "resources/app.asar").exec, undefined);

  const requested = [];
  const target = path.join(dir, "target");
  fs.mkdirSync(target);
  const progress = [];
  const result = await chunks.assemble({
    manifest, sourceRoot: old, targetDir: target, cacheDir: path.join(dir, "cache"),
    chunkBaseUrls: ["https://hlsr.pro/downloads/launcher/chunks"], isAllowedUrl: allowAll,
    fetch: server(store, {}, requested), onProgress: (value) => progress.push(value),
  });
  assert.deepEqual(files(target), files(next));
  if (!NO_EXEC_BIT) assert.equal(fs.statSync(path.join(target, "hlsr")).mode & 0o111, 0o111);
  const total = manifest.files.reduce((sum, file) => sum + file.chunks.length, 0);
  assert.ok(requested.length > 0 && requested.length <= 6, `скачано ${requested.length} из ${total} кусков`);
  assert.ok(result.reusedBytes > 2 * 1024 * 1024);
  assert.equal(progress[progress.length - 1].done, result.downloadedBytes);

  // Без старой версии собирается целиком из сети.
  const clean = path.join(dir, "clean");
  fs.mkdirSync(clean);
  await chunks.assemble({
    manifest, sourceRoot: null, targetDir: clean, cacheDir: path.join(dir, "cache2"),
    chunkBaseUrls: ["https://hlsr.pro/downloads/launcher/chunks"], isAllowedUrl: allowAll, fetch: server(store, {}),
  });
  assert.deepEqual(files(clean), files(next));
});

test("подменённый кусок, чужой манифест и адрес вне allowlist не проходят", async (t) => {
  const dir = tempDir(t), store = path.join(dir, "store"), parts = base();
  const next = tree(path.join(dir, "next"), "0.4.4", parts, true);
  const { manifest } = chunks.buildStore(next, store, { version: "0.4.4", platform: "linux" });
  const victim = manifest.files.find((file) => file.path === "resources/app.asar").chunks[1][0];
  const victimFile = path.join(store, victim.slice(0, 2), victim);
  const original = fs.readFileSync(victimFile);
  fs.writeFileSync(victimFile, chunks.compress(crypto.randomBytes(1000)).subarray(0, original.length));
  const target = path.join(dir, "target");
  fs.mkdirSync(target);
  await assert.rejects(chunks.assemble({
    manifest, sourceRoot: null, targetDir: target, cacheDir: path.join(dir, "cache"),
    chunkBaseUrls: ["https://hlsr.pro/downloads/launcher/chunks"], isAllowedUrl: allowAll, fetch: server(store, {}),
  }), /повреждён|размер|brotli|Decompression|unexpected end/i);

  const body = Buffer.from(JSON.stringify(manifest));
  const delta = { schema: 1, manifest: { filename: "HLSR-0.4.4-linux.chunks.json", size: body.length, sha256: chunks.sha256(body),
    urls: ["https://hlsr.pro/downloads/launcher/linux/stable/HLSR-0.4.4-linux.chunks.json"] },
  chunkBaseUrls: ["https://hlsr.pro/downloads/launcher/chunks"] };
  const fetchManifest = (data, expected) => chunks.fetchManifest(delta, {
    isAllowedUrl: allowAll, version: expected.version, platform: expected.platform,
    fetch: async () => data,
  });
  assert.equal((await fetchManifest(body, { version: "0.4.4", platform: "linux" })).version, "0.4.4");
  await assert.rejects(fetchManifest(body, { version: "0.4.5", platform: "linux" }), /версия/);
  await assert.rejects(fetchManifest(body, { version: "0.4.4", platform: "win32" }), /платформа/);
  await assert.rejects(fetchManifest(Buffer.concat([body.subarray(1), Buffer.from(" ")]), { version: "0.4.4" }), /не совпал/);

  const traversal = JSON.parse(JSON.stringify(manifest));
  traversal.files[0].path = "../evil";
  assert.match(chunks.problemsOf(traversal).join(";"), /недопустимый путь/);
  assert.deepEqual(chunks.deltaProblems(delta, channel.isAllowedUrl), []);
  assert.match(chunks.deltaProblems({ ...delta, chunkBaseUrls: ["https://evil.example/chunks"] }, channel.isAllowedUrl).join(";"), /allowlist/);
});

// Linux-релиз с чанками: AppImage-заглушка (полный путь) и delta в канале.
function release(t, dir, version, parts, next, keys) {
  const source = tree(path.join(dir, `tree-${version}`), version, parts, next);
  const archive = path.join(dir, `HLSR-${version}-linux-x86_64.tar.gz`);
  execFileSync("tar", ["--format=ustar", "-czf", archive, "-C", source, "."]);
  const unpackedSize = Object.keys(files(source)).reduce((sum, name) => sum + fs.statSync(path.join(source, name)).size, 0);
  const bytes = fs.readFileSync(archive);
  const descriptor = embeddedPayload.describe({ version, size: bytes.length,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    sha512: crypto.createHash("sha512").update(bytes).digest("base64"), unpackedSize });
  const payload = path.join(dir, `payload-${version}`);
  fs.mkdirSync(payload);
  fs.copyFileSync(archive, path.join(payload, path.basename(archive)));
  fs.writeFileSync(path.join(payload, embeddedPayload.DESCRIPTOR), JSON.stringify(descriptor));
  const out = path.join(dir, `out-${version}`);
  fs.mkdirSync(out);
  const appImage = path.join(out, `HLSR-Setup-${version}-linux-x86_64.AppImage`);
  fs.writeFileSync(appImage, `#!/bin/sh\n[ "$1" = --appimage-extract ] && [ "$2" = resources/payload ] || exit 2\n` +
    `mkdir -p squashfs-root/resources && cp -r '${payload}' squashfs-root/resources/payload\n`, { mode: 0o755 });
  const result = prepare({ version, minimumSupported: "0.4.0", buildDir: out, outDir: out, tree: source,
    chunkStore: path.join(dir, "store"), privateKey: keys.privateKeyPem, publicKeys: [keys.publicKeyPem] });
  return { manifest: result.manifest, out, archive, descriptor, source, appImage };
}

function manager(t, root, gate, opts) {
  const fakeApp = new EventEmitter();
  fakeApp.isPackaged = true; fakeApp.getVersion = () => "0.4.3";
  fakeApp.relaunch = () => {}; fakeApp.quit = () => {};
  const location = path.resolve(__dirname, "../src/main/linux/UpdateManager.js");
  const localRequire = createRequire(location), output = { exports: {} };
  vm.runInNewContext(fs.readFileSync(location, "utf8"), {
    module: output, exports: output.exports, process, AbortController,
    require: (name) => name === "electron" ? { app: fakeApp, ipcMain: { handle() {} } }
      : name === "electron-log" ? { info() {}, warn() {}, error() {} } : localRequire(name),
    setTimeout, clearTimeout, setImmediate, Date, URL,
  }, { filename: location });
  const instance = new output.exports(null, { app: fakeApp, installRoot: root, gate, ...opts });
  instance.initialize();
  t.after(() => instance.dispose());
  return instance;
}

async function update(instance) {
  assert.equal(instance.checkUpdates("manual"), true); await instance.checkPromise;
  assert.equal(instance.state, "available");
  assert.equal(instance.download(), true); await instance.downloadPromise;
  assert.equal(instance.state, "downloaded", instance.error);
  assert.equal(instance.applyUpdate(false), true);
}

test("Linux: обновление чанками, откат на полный установщик и удаление позапрошлых версий", { skip: NOT_LINUX }, async (t) => {
  const dir = tempDir(t), keys = signing.generateKeyPair(), parts = base();
  const first = release(t, dir, "0.4.3", parts, false, keys);
  const next = release(t, dir, "0.4.4", parts, true, keys);
  assert.equal(next.manifest.delta.manifest.filename, "HLSR-0.4.4-linux.chunks.json");
  assert.deepEqual(next.manifest.delta.chunkBaseUrls, ["https://hlsr.pro/downloads/launcher/chunks"]);
  // Клиент без поддержки чанков принимает тот же подписанный канал.
  assert.equal(channel.check(next.manifest, { publicKeys: [keys.publicKeyPem], platform: "linux" }).ok, true);
  assert.ok(channel.deltaOf(next.manifest));

  const root = path.join(dir, "install");
  // Позапрошлые версии от прежних обновлений.
  for (const name of ["0.4.1-aaaaaaaaaaaaaaaa", "0.4.2-bbbbbbbbbbbbbbbb"]) write(path.join(root, "versions", name), "hlsr", "old");
  const initial = new Install(root);
  await initial.prepare(first.archive, first.descriptor);
  initial.commit();
  const installed = Install.readCurrent(root);

  const manifests = { "HLSR-0.4.4-linux.chunks.json": fs.readFileSync(path.join(next.out, "HLSR-0.4.4-linux.chunks.json")) };
  const requested = [];
  const gate = new Gate({ currentVersion: "0.4.3", publicKeys: [keys.publicKeyPem],
    httpGet: async (url) => url.endsWith(".yml") ? fs.readFileSync(path.join(next.out, "latest-linux.yml")) : Buffer.from(JSON.stringify(next.manifest)) });
  let fullDownloads = 0;
  const instance = manager(t, root, gate, {
    fetchChunk: server(path.join(dir, "store"), manifests, requested),
    download: async () => { fullDownloads++; throw new Error("полный установщик не нужен"); },
  });
  await update(instance);
  assert.equal(fullDownloads, 0);
  assert.ok(requested.length <= 7, `запрошено ${requested.length}`);
  assert.deepEqual(files(path.join(root, Install.readCurrent(root))), files(next.source));
  assert.equal(fs.readlinkSync(path.join(root, "previous")), installed);
  assert.deepEqual(fs.readdirSync(path.join(root, "versions")).sort(),
    [path.basename(installed), path.basename(Install.readCurrent(root))].sort());

  // Сломанное хранилище: клиент качает установщик целиком и всё равно ставит.
  const third = release(t, dir, "0.4.5", { ...parts, lib: crypto.randomBytes(400 * 1024) }, true, keys);
  fs.rmSync(path.join(dir, "store"), { recursive: true, force: true });
  const gate2 = new Gate({ currentVersion: "0.4.4", publicKeys: [keys.publicKeyPem],
    httpGet: async (url) => url.endsWith(".yml") ? fs.readFileSync(path.join(third.out, "latest-linux.yml")) : Buffer.from(JSON.stringify(third.manifest)) });
  const fallback = manager(t, root, gate2, {
    fetchChunk: server(path.join(dir, "store"), { "HLSR-0.4.5-linux.chunks.json": fs.readFileSync(path.join(third.out, "HLSR-0.4.5-linux.chunks.json")) }),
    download: async (manifest, directory) => {
      fullDownloads++;
      const target = path.join(directory, path.basename(third.appImage));
      fs.copyFileSync(third.appImage, target);
      return target;
    },
  });
  await update(fallback);
  assert.equal(fullDownloads, 1);
  assert.deepEqual(files(path.join(root, Install.readCurrent(root))), files(third.source));
  assert.equal(fs.readdirSync(path.join(root, "versions")).length, 2);
});

function session(t, delta, deps) {
  const dir = tempDir(t);
  const plan = {
    manifest: { delta, installer: {} }, currentVersion: "0.4.3", targetVersion: "0.4.4",
    installer: { filename: "HLSR-Setup-0.4.4.exe", size: 10, sha256: "a".repeat(64), sha512: "b", urls: ["https://hlsr.pro/x"] },
    installDir: "C:\\HLSR\\Launcher", launcherExe: "C:\\HLSR\\Launcher\\HLSR.exe", receiptFile: "C:\\x\\version-state.json",
  };
  const calls = [];
  const instance = new UpdateSession({ plan, workDir: dir, deps: Object.assign({
    download: async () => { calls.push("download"); return path.join(dir, "HLSR-Setup-0.4.4.exe"); },
    verifyInstaller: async () => ({ ok: true }),
    waitForExit: async () => calls.push("wait"),
    runInstaller: async (file, options) => calls.push({ file, options }),
    verifyInstalled: () => ({ ok: true }),
    relaunch: () => calls.push("relaunch"),
    writeReceipt: () => true,
    fetchManifest: async () => ({ version: "0.4.4" }),
  }, deps) });
  return { instance, calls, dir };
}

test("Windows: апдейтер ставит собранную из чанков версию её же установщиком, при сбое качает полный", async (t) => {
  const delta = { schema: 1, manifest: { filename: "HLSR-0.4.4-win32.chunks.json", size: 10, sha256: "c".repeat(64),
    urls: ["https://hlsr.pro/downloads/launcher/stable/HLSR-0.4.4-win32.chunks.json"] },
  chunkBaseUrls: ["https://hlsr.pro/downloads/launcher/chunks"] };
  let assembled = null;
  const ok = session(t, delta, { assemble: async (options) => { assembled = options; return { downloadedBytes: 1, reusedBytes: 2 }; } });
  assert.equal((await ok.instance.run()).ok, true);
  assert.equal(assembled.sourceRoot, "C:\\HLSR\\Launcher");
  assert.equal(assembled.targetDir, path.join(ok.dir, "tree"));
  const install = ok.calls.find((call) => call.file);
  assert.equal(install.file, path.join(ok.dir, "tree", "HLSR.exe"));
  assert.deepEqual(install.options.args, ["/S", "--updated", "/D=C:\\HLSR\\Launcher"]);
  assert.equal(install.options.env.PORTABLE_EXECUTABLE_FILE, path.join(ok.dir, "tree", "HLSR-Setup-0.4.4.exe"));
  assert.ok(!ok.calls.includes("download"));

  const broken = session(t, delta, { assemble: async () => { throw new Error("HTTP 404"); } });
  assert.equal((await broken.instance.run()).ok, true);
  assert.ok(broken.calls.includes("download"));
  assert.equal(broken.calls.find((call) => call.file).file, path.join(broken.dir, "HLSR-Setup-0.4.4.exe"));

  const plain = session(t, undefined, { assemble: async () => { throw new Error("не должен вызываться"); } });
  assert.equal((await plain.instance.run()).ok, true);
  assert.ok(plain.calls.includes("download"));
});
