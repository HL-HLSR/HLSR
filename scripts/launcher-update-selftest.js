"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), os = require("os"), path = require("path"), crypto = require("crypto"), vm = require("vm");
const { execFileSync } = require("child_process");
const { createRequire } = require("module");
const { EventEmitter } = require("events");
const signing = require("../src/main/game-delivery/manifest-signing");
const channel = require("../src/main/launcher-update/launcher-channel");
const Gate = require("../src/main/launcher-update/LauncherUpdateGate");
const Install = require("../src/main/launcher-update/ArchiveInstall");
const { download } = require("../src/main/launcher-update/ArchiveDownload");
const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
const { extract } = require("../src/main/launcher-update/archive");
const { prepare } = require("./launcher-release-prepare-linux");
const { verify } = require("./verify-launcher-release-linux");
const { NO_SYMLINKS, NO_EXEC_BIT, NO_SHELL_SCRIPTS, NOT_LINUX, TAR } = require("./lib/selftest-caps");

function fixture(t, version = "0.3.18") {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-archive-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const source = path.join(dir, "source");
  fs.mkdirSync(path.join(source, "resources/icons"), { recursive: true });
  const elf = Buffer.alloc(40);
  Buffer.from("7f454c46020101", "hex").copy(elf); elf.writeUInt16LE(62, 18);
  fs.writeFileSync(path.join(source, "hlsr"), elf, { mode: 0o755 });
  fs.writeFileSync(path.join(source, "resources/app.asar"), "test application " + version);
  fs.writeFileSync(path.join(source, "resources/icons/hlsr.png"), "icon");
  fs.writeFileSync(path.join(source, ".hlsr-release.json"), JSON.stringify({ version, platform: "linux", arch: "x64", executable: "hlsr" }));
  const file = path.join(dir, `HLSR-${version}-linux-x86_64.tar.gz`);
  execFileSync(TAR, ["--format=ustar", "-czf", file, "-C", source, "."]);
  const unpackedSize = ["hlsr", "resources/app.asar", "resources/icons/hlsr.png", ".hlsr-release.json"]
    .reduce((sum, name) => sum + fs.statSync(path.join(source, name)).size, 0);
  fs.writeFileSync(`${file}.json`, JSON.stringify({ version, unpackedSize }));
  const descriptor = embeddedPayload.describe({ version, size: fs.statSync(file).size,
    sha256: crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"),
    sha512: crypto.createHash("sha512").update(fs.readFileSync(file)).digest("base64"), unpackedSize });
  const descriptorFile = path.join(dir, embeddedPayload.DESCRIPTOR);
  fs.writeFileSync(descriptorFile, JSON.stringify(descriptor));
  // Вместо рантайма AppImage — скрипт с тем же контрактом --appimage-extract.
  const appImage = path.join(dir, `HLSR-Setup-${version}-linux-x86_64.AppImage`);
  fs.writeFileSync(appImage, `#!/bin/sh\n[ "$1" = --appimage-extract ] && [ "$2" = resources/payload ] || exit 2\n` +
    `mkdir -p squashfs-root/resources/payload && cp '${descriptorFile}' '${file}' squashfs-root/resources/payload/\n`, { mode: 0o755 });
  const keys = signing.generateKeyPair();
  const result = prepare({ version, minimumSupported: "0.3.17", buildDir: dir, outDir: dir, privateKey: keys.privateKeyPem, publicKeys: [keys.publicKeyPem], chunks: false });
  return { dir, source, file, appImage, keys, manifest: result.manifest, archive: { version, installer: descriptor.installer } };
}
function fetchInstaller(f) {
  return async (manifest, directory) => {
    const target = path.join(directory, path.basename(f.appImage));
    fs.copyFileSync(f.appImage, target);
    return target;
  };
}
function served(f, url) {
  return url.endsWith(".yml") ? fs.readFileSync(path.join(f.dir, "latest-linux.yml")) : Buffer.from(JSON.stringify(f.manifest));
}
function gateFor(f, options = {}) {
  return new Gate({ currentVersion: "0.3.17", publicKeys: [f.keys.publicKeyPem],
    httpGet: async (url) => served(f, url), ...options });
}
function managerFor(t, f, opts = {}) {
  const fakeApp = new EventEmitter();
  fakeApp.isPackaged = true; fakeApp.getVersion = () => "0.3.17";
  fakeApp.relaunch = (options) => { fakeApp.relaunchOptions = options; };
  fakeApp.quit = () => { fakeApp.quitCalled = true; };
  const location = path.resolve(__dirname, "../src/main/linux/UpdateManager.js");
  const localRequire = createRequire(location), output = { exports: {} };
  vm.runInNewContext(fs.readFileSync(location, "utf8"), {
    module: output, exports: output.exports, process, AbortController,
    require: (name) => name === "electron" ? { app: fakeApp, ipcMain: { handle() {} } }
      : name === "electron-log" ? { info() {}, warn() {}, error() {} } : localRequire(name),
    setTimeout, clearTimeout, setImmediate, Date, URL,
  }, { filename: location });
  const root = path.join(f.dir, "installation");
  const manager = new output.exports(null, {
    app: fakeApp, installRoot: root, gate: gateFor(f),
    download: fetchInstaller(f), ...opts,
  });
  manager.initialize(); t.after(() => manager.dispose());
  return { manager, fakeApp, root };
}
async function ready(manager) {
  assert.equal(manager.checkUpdates("manual"), true); await manager.checkPromise;
  assert.equal(manager.state, "available");
  assert.equal(manager.download(), true); await manager.downloadPromise;
  assert.equal(manager.state, "downloaded");
}

test("signed release is the installer, latest-linux.yml and channel; production rejects test keys", { skip: NO_SHELL_SCRIPTS }, async (t) => {
  const f = fixture(t);
  assert.equal((await verify(f.dir, [f.keys.publicKeyPem])).version, "0.3.18");
  assert.deepEqual(Object.keys(f.manifest).sort(), ["installer", "latestYmlSha256", "minimumSupportedVersion", "publishedAt", "schemaVersion", "signature", "updateRequired", "version"]);
  assert.equal(f.manifest.installer.filename, "HLSR-Setup-0.3.18-linux-x86_64.AppImage");
  const yml = fs.readFileSync(path.join(f.dir, "latest-linux.yml"), "utf8");
  assert.match(yml, /^version: 0\.3\.18\nfiles:\n  - url: HLSR-Setup-0\.3\.18-linux-x86_64\.AppImage\n/);
  assert.ok(yml.includes(`sha512: ${f.manifest.installer.sha512}`));
  assert.equal(channel.check(f.manifest).ok, false);
  // Испорченный вшитый архив ловится, хотя хеши самого AppImage совпали.
  fs.appendFileSync(f.file, "bad");
  await assert.rejects(verify(f.dir, [f.keys.publicKeyPem]), /размер/);
  fs.appendFileSync(f.appImage, "bad");
  await assert.rejects(verify(f.dir, [f.keys.publicKeyPem]), /размер/);
});
test("gate falls back and rejects unsigned, Windows installer, old archive schema and downgrade", { skip: NOT_LINUX }, async (t) => {
  const f = fixture(t); let calls = 0;
  const gate = gateFor(f, { httpGet: async (url) => { if (++calls === 1) throw new Error("HTTP 503"); return served(f, url); } });
  assert.equal((await gate.evaluate()).ok, true); assert.equal(calls, 3);
  const swapped = await gateFor(f, { httpGet: async (url) => url.endsWith(".yml") ? Buffer.from("version: 0.3.18\n") : served(f, url) }).evaluate();
  assert.equal(swapped.ok, false); assert.match(swapped.problems.join(";"), /latest\.yml не подтверждён/);
  const keys = { publicKeys: [f.keys.publicKeyPem] };
  assert.equal(channel.check(f.manifest, { ...keys, platform: "linux" }).ok, true);
  assert.equal(channel.check(f.manifest, { ...keys, platform: "win32" }).ok, false);
  for (const patch of [{ signature: null }, { schemaVersion: 2 },
    { installer: { ...f.manifest.installer, filename: "HLSR-Setup-0.3.18.exe" } },
    { installer: { ...f.manifest.installer, filename: "HLSR-0.3.18-linux-x86_64.tar.gz" } }])
    assert.equal(channel.check({ ...f.manifest, ...patch }, { ...keys, platform: "linux" }).ok, false);
  assert.equal((await gateFor(f, { currentVersion: "0.3.19" }).evaluate()).ok, false);
});
test("подписанный обязательный канал не новее установленного (повтор старого) не делает обновление обязательным", async () => {
  const keys = signing.generateKeyPair(), version = "0.3.18";
  const manifest = signing.attach({
    schemaVersion: 1, version, minimumSupportedVersion: "0.3.0", publishedAt: "2026-11-01T00:00:00.000Z", updateRequired: true,
    installer: { filename: process.platform === "linux" ? `HLSR-Setup-${version}-linux-x86_64.AppImage` : `HLSR-Setup-${version}.exe`,
      size: 1, sha256: "0".repeat(64), sha512: `${"A".repeat(86)}==`, urls: ["https://hlsr.pro/x"] },
  }, keys.privateKeyPem);
  const outcome = (currentVersion, expected) => new Gate({ currentVersion, publicKeys: [keys.publicKeyPem],
    httpGet: async () => Buffer.from(JSON.stringify(manifest)) }).evaluate(expected);
  assert.equal((await outcome("0.3.17")).updateRequired, true, "версия новее установленной — обязательное");
  for (const [current, expected] of [["0.3.18"], ["0.3.18", "0.3.19"], ["0.3.19"], ["0.3.19", "0.3.20"]])
    assert.equal((await outcome(current, expected)).updateRequired, false, `${current} → ${expected || "без latest.yml"}`);
});
test("initial installation, next version, stable path and rollback", { skip: NO_SYMLINKS || NO_EXEC_BIT }, async (t) => {
  const first = fixture(t), second = fixture(t, "0.3.19");
  const root = path.join(first.dir, "install with spaces");
  const initial = new Install(root); await initial.prepare(first.file, first.archive);
  assert.equal(Install.readCurrent(root), null);
  initial.commit();
  const old = Install.readCurrent(root);
  assert.equal(Install.managedRoot(path.join(root, "current/hlsr")), root);
  const update = new Install(root); await update.prepare(second.file, second.archive);
  assert.equal(Install.readCurrent(root), old);
  const target = update.commit();
  assert.notEqual(Install.readCurrent(root), old);
  assert.equal(fs.readlinkSync(path.join(root, "previous")), old);
  assert.equal(fs.statSync(target).mode & 0o777, 0o755);
  assert.ok(fs.readFileSync(path.join(root, "start"), "utf8").includes('current/hlsr'));
  update.rollback(); assert.equal(Install.readCurrent(root), old);
});
test("corrupted archive cannot alter an installed version", { skip: NO_SYMLINKS }, async (t) => {
  const f = fixture(t); const root = path.join(f.dir, "install");
  const install = new Install(root); await install.prepare(f.file, f.archive); install.commit();
  const old = Install.readCurrent(root); fs.appendFileSync(f.file, "bad");
  await assert.rejects(new Install(root).prepare(f.file, f.archive));
  assert.equal(Install.readCurrent(root), old);
  assert.equal(fs.existsSync(path.join(root, ".update-lock")), false);
});
test("tar traversal, links, truncated streams and expanded size limits are rejected", async (t) => {
  const f = fixture(t), dest = path.join(f.dir, "out"); fs.mkdirSync(dest);
  await assert.rejects(extract(f.file, dest, { maxBytes: 1 }), /размер/);
  if (NO_SYMLINKS) return t.skip(NO_SYMLINKS);
  fs.symlinkSync("../../outside", path.join(f.source, "link"));
  execFileSync(TAR, ["--format=ustar", "-czf", f.file, "-C", f.source, "."]);
  fs.rmSync(dest, { recursive: true }); fs.mkdirSync(dest);
  await assert.rejects(extract(f.file, dest), /Ссылки/);
  fs.unlinkSync(path.join(f.source, "link"));
  if (TAR !== "tar") return t.skip("--transform есть только у GNU tar, а на Windows tar — bsdtar");
  execFileSync("tar", ["--format=ustar", "--transform=s|^./hlsr$|../outside|", "-czf", f.file, "-C", f.source, "."]);
  fs.rmSync(dest, { recursive: true }); fs.mkdirSync(dest);
  await assert.rejects(extract(f.file, dest), /каталог/);
  assert.equal(fs.existsSync(path.join(f.dir, "outside")), false);
  fs.writeFileSync(f.file, fs.readFileSync(f.file).subarray(0, 30));
  fs.rmSync(dest, { recursive: true }); fs.mkdirSync(dest);
  await assert.rejects(extract(f.file, dest));
});
test("parallel installation is locked; cancelled preparation releases its lock", async (t) => {
  const f = fixture(t), root = path.join(f.dir, "install");
  const first = new Install(root); await first.prepare(f.file, f.archive);
  await assert.rejects(new Install(root).prepare(f.file, f.archive), /Другая установка/);
  first.discard();
  const controller = new AbortController(); controller.abort();
  await assert.rejects(new Install(root).prepare(f.file, f.archive, { signal: controller.signal }), /отменена/);
  assert.equal(fs.existsSync(path.join(root, ".update-lock")), false);
});
test("manager blocks installation during a game and relaunches the installed binary", { skip: NOT_LINUX }, async (t) => {
  const f = fixture(t), { manager, fakeApp, root } = managerFor(t, f);
  await ready(manager); manager.setGameRunningCheck(() => true);
  assert.equal(manager.installNow(), false); assert.equal(Install.readCurrent(root), null);
  manager.setGameRunningCheck(() => false); assert.equal(manager.installNow(), true);
  await new Promise(setImmediate);
  assert.equal(fakeApp.quitCalled, true);
  assert.equal(Install.managedRoot(fakeApp.relaunchOptions.execPath), root);
});

test("Linux: перезапуск после обновления сохраняет параметры окна", () => {
  const location = path.resolve(__dirname, "../src/main/linux/UpdateManager.js");
  const localRequire = createRequire(location), output = { exports: {} };
  vm.runInNewContext(fs.readFileSync(location, "utf8"), {
    module: output, process, AbortController,
    require: (name) => name === "electron" ? { app: {}, ipcMain: {} }
      : name === "electron-log" ? { info() {}, warn() {}, error() {} } : localRequire(name),
    setTimeout, clearTimeout, setImmediate, Date, URL,
  }, { filename: location });
  const args = output.exports.relaunchArguments(["/installation/current/hlsr", "--ozone-platform=wayland", "--no-sandbox", "--disable-gpu"]);
  assert.deepEqual(Array.from(args), ["--no-sandbox", "--ozone-platform=wayland", "--disable-gpu"]);
});

test("Linux: локальный канал и ключ не включаются переменными production-сборки", () => {
  const location = path.resolve(__dirname, "../src/main/launcher-update/local-qa.js");
  function load(build, feed) {
    const output = { exports: {} };
    vm.runInNewContext(fs.readFileSync(location, "utf8"), {
      module: output, require: createRequire(location), URL,
      process: { env: { HLSR_BUILD_CHANNEL: build, HLSR_UPDATE_FEED: feed, HLSR_UPDATE_CHANNEL_KEY: "untrusted" } },
    }, { filename: location });
    return output.exports.configuration("0.4.0", { info() {} });
  }
  assert.equal(load("production", "http://127.0.0.1:18899"), null);
  assert.equal(load("qa", "http://127.0.0.1.evil.test:18899"), null);
  assert.equal(load("qa", "http://user@localhost:18899"), null);
  assert.equal(load("qa", "https://hlsr.pro"), null);
  assert.equal(load("qa", "http://localhost:18899?token=x"), null);
  assert.ok(load("qa", "http://127.0.0.1:18899"));
});

test("manager installs only the launcher version the signed channel names", { skip: NOT_LINUX }, async (t) => {
  const f = fixture(t), other = fixture(t, "0.3.19");
  const { manager, root } = managerFor(t, f, { extractPayload: async () => ({ manifest: other.archive, archive: other.file }) });
  manager.checkUpdates("manual"); await manager.checkPromise;
  manager.download(); await manager.downloadPromise;
  assert.equal(manager.state, "error"); assert.match(manager.error, /0\.3\.19/);
  assert.equal(Install.readCurrent(root), null);
  assert.equal(fs.existsSync(path.join(root, ".downloads")) && fs.readdirSync(path.join(root, ".downloads")).length, 0);
});

test("manager: cancelling a launcher download leaves an available update and supports retry", { skip: NOT_LINUX }, async (t) => {
  const f = fixture(t);
  let attempts = 0;
  const { manager, root } = managerFor(t, f, { download: async (manifest, directory, { signal }) => {
    if (++attempts > 1) return fetchInstaller(f)(manifest, directory);
    fs.writeFileSync(path.join(directory, "partial"), "bytes");
    await new Promise((resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
  } });
  manager.checkUpdates("manual"); await manager.checkPromise;
  manager.download();
  assert.equal(manager.cancelDownload(), true);
  await manager.downloadPromise;
  assert.equal(manager.state, "available"); assert.equal(manager.error, null);
  assert.equal(Install.readCurrent(root), null);
  assert.equal(manager.cancelDownload(), false);
  manager.download(); await manager.downloadPromise;
  assert.equal(manager.state, "downloaded");
  manager.dispose();
});
test("installation on quit requires an explicit choice; mandatory boot errors block", { skip: NOT_LINUX }, async (t) => {
  const f = fixture(t), { manager, fakeApp, root } = managerFor(t, f);
  await ready(manager); fakeApp.emit("before-quit", { preventDefault() {} });
  assert.equal(Install.readCurrent(root), null);
  manager.installLater(); fakeApp.emit("before-quit", { preventDefault() {} });
  assert.ok(Install.readCurrent(root)); assert.equal(fakeApp.relaunchOptions, undefined);
  const other = managerFor(t, f);
  other.manager.gate.evaluate = async () => ({ ok: false, unsupported: true, problems: ["unsupported"] });
  assert.equal((await other.manager.runBootGate()).action, "blocked");
});
test("real HTTP installer download falls back, verifies bytes, and feeds installation", async (t) => {
  const http = require("http"), f = fixture(t); let attempts = 0;
  const server = http.createServer((req, res) => {
    attempts++;
    if (req.url === "/broken") { res.writeHead(503); res.end(); }
    else fs.createReadStream(f.appImage).pipe(res);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const manifest = { ...f.manifest, installer: { ...f.manifest.installer, urls: [`${base}/broken`, `${base}/installer`] } };
  const downloads = path.join(f.dir, "downloads");
  const target = await download(manifest, downloads, { transport: http, isAllowedUrl: (url) => url.startsWith(base + "/") });
  assert.equal(attempts, 2); assert.deepEqual(fs.readFileSync(target), fs.readFileSync(f.appImage));
  // Дальше заглушка AppImage (скрипт #!/bin/sh) отдаёт архив, а установка переключает ссылки версий.
  if (NO_SHELL_SCRIPTS || NO_SYMLINKS) return t.skip(NO_SHELL_SCRIPTS || NO_SYMLINKS);
  const payload = await embeddedPayload.extractFrom(target, downloads);
  assert.deepEqual(payload.manifest, f.archive);
  const install = new Install(path.join(f.dir, "install")); await install.prepare(payload.archive, payload.manifest); install.commit();
  assert.ok(Install.readCurrent(install.root));
});
test("launching from an AppImage removes installer-only environment", () => {
  const { launchEnvironment } = require("../src/main/launcher-update/BootstrapInstaller");
  const env = launchEnvironment({ APPIMAGE: "/tmp/setup", APPDIR: "/tmp/mount", LD_LIBRARY_PATH: "/tmp/mount/lib:/custom", DISPLAY: ":1" });
  assert.equal(env.APPIMAGE, undefined); assert.equal(env.APPDIR, undefined);
  assert.equal(env.LD_LIBRARY_PATH, "/custom"); assert.equal(env.DISPLAY, ":1");
});
test("installer offers uninstall, games only with explicit consent", async (t) => {
  const { openInstaller } = require("../src/main/launcher-update/BootstrapInstaller");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-installer-uninstall-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const root = path.join(dir, "opt", "hlsr");
  // Как в Linux: данные лаунчера в ~/.config/hlsr, установщик — в его подкаталоге,
  // журналы в ~/.local/state/hlsr (домашний каталог здесь — dir).
  const config = path.join(dir, "config", "hlsr");
  const userData = path.join(config, "installer");
  const logs = path.join(dir, ".local", "state", "hlsr");
  const library = path.join(dir, "Games", "HLSR");
  function fixture() {
    fs.mkdirSync(path.join(root, "versions", "0.0.1"), { recursive: true });
    fs.writeFileSync(path.join(root, ".hlsr-install.json"), JSON.stringify({ schema: 1 }));
    fs.mkdirSync(path.join(library, "Games", "ghosting-renewed"), { recursive: true });
    fs.writeFileSync(path.join(library, "Games", "ghosting-renewed", "installed.json"), JSON.stringify({ gameId: "ghosting-renewed" }));
    fs.mkdirSync(userData, { recursive: true });
    fs.writeFileSync(path.join(config, "library.json"), JSON.stringify({ libraryPath: library }));
    fs.mkdirSync(path.join(logs, "logs"), { recursive: true });
    fs.writeFileSync(path.join(logs, "logs", "main.log"), "log");
  }
  fixture();
  const handlers = {};
  const sent = [];
  const contents = { send: (...args) => sent.push(args) };
  const fakeWindow = {
    removeMenu() {}, loadFile() {}, minimize() {}, close() {}, destroy() {},
    on() {}, webContents: Object.assign({ setWindowOpenHandler() {}, setZoomFactor() {}, setVisualZoomLevelLimits() {}, on() {} }, contents),
  };
  const electron = {
    app: { getLocale: () => "ru-RU", getPath: (name) => (name === "userData" ? userData : dir) },
    BrowserWindow: function () { return fakeWindow; },
    ipcMain: { handle: (name, fn) => { handlers[name] = fn; }, removeHandler: (name) => { delete handlers[name]; } },
  };
  const event = { sender: fakeWindow.webContents };
  const previousResourcesPath = process.resourcesPath;
  process.resourcesPath = dir;
  t.after(() => { process.resourcesPath = previousResourcesPath; });
  // Удалению встроенный архив не нужен, но установщик без описания считается
  // повреждённым: кладём корректное описание.
  const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
  fs.mkdirSync(path.join(dir, embeddedPayload.DIRECTORY));
  fs.writeFileSync(path.join(dir, embeddedPayload.DIRECTORY, embeddedPayload.DESCRIPTOR), JSON.stringify(embeddedPayload.describe({
    version: "0.0.2", size: 1, sha256: "0".repeat(64), sha512: "A".repeat(86) + "==", unpackedSize: 1,
  })));
  openInstaller(electron, { root, assets: dir });
  const initial = await handlers["installer-get-state"](event);
  assert.equal(initial.phase, "idle");
  assert.equal(initial.canUninstall, true);
  const plan = await handlers["installer-uninstall-plan"](event);
  assert.equal(plan.libraryRoot, library);
  assert.ok(plan.libraryBytes > 0);
  assert.ok(await handlers["installer-uninstall"](event, false));
  assert.equal(fs.existsSync(root), false);
  assert.equal(fs.existsSync(path.join(library, "Games", "ghosting-renewed", "installed.json")), true);
  // Без галочки данные остаются: переустановка сохранит настройки и вход.
  assert.equal(fs.existsSync(path.join(config, "library.json")), true);
  assert.equal(fs.existsSync(path.join(logs, "logs", "main.log")), true);
  assert.equal(sent[sent.length - 1][0], "installer-state");
  assert.equal(sent[sent.length - 1][1].phase, "uninstalled");
  // «Назад» возвращает к установке в тот же, теперь пустой каталог.
  const back = await handlers["installer-back"](event);
  assert.equal(back.phase, "idle");
  assert.equal(back.canUninstall, false);
  assert.equal(back.directory, initial.directory);
  assert.equal(await handlers["installer-back"](event), false);
  // Повторное открытие установщика после удаления: снова можно удалять.
  // Без маркера библиотеки уходит только её содержимое, чужой корень цел.
  fixture();
  openInstaller(electron, { root, assets: dir });
  assert.ok(await handlers["installer-uninstall"](event, true));
  assert.equal(fs.existsSync(path.join(library, "Games")), false);
  assert.equal(fs.existsSync(path.join(library, "Games", "ghosting-renewed", "installed.json")), false);
  // Полное удаление уносит и данные лаунчера, но не чужое рядом с ними.
  assert.equal(fs.existsSync(config), false);
  assert.equal(fs.existsSync(logs), false);
  assert.equal(fs.existsSync(path.join(dir, "config")), true);
  // С маркером библиотека уходит целиком, если в ней нет чужого.
  const cleanup = require("../src/main/linux/library-cleanup");
  fixture();
  cleanup.writeMarker(library);
  openInstaller(electron, { root, assets: dir });
  assert.ok(await handlers["installer-uninstall"](event, true));
  assert.equal(fs.existsSync(library), false);
  // Библиотека, вписанная руками (~/Games): чужие файлы и папки остаются, окно о них говорит.
  fixture();
  cleanup.writeMarker(library);
  fs.writeFileSync(path.join(library, "notes.txt"), "mine");
  fs.mkdirSync(path.join(library, "Games", "MyMod"), { recursive: true });
  openInstaller(electron, { root, assets: dir });
  assert.ok(await handlers["installer-uninstall"](event, true));
  assert.deepEqual(fs.readdirSync(library).sort(), ["Games", "notes.txt"]);
  assert.deepEqual(fs.readdirSync(path.join(library, "Games")), ["MyMod"]);
  assert.match(sent[sent.length - 1][1].warning, /Оставлено в .* \(2\)/);
});

test("окно удаления Windows: «Назад» идёт в backend.back или спрятано", async () => {
  const { openInstallerWindow } = require("../src/main/launcher-update/installer-window");
  const handlers = {};
  const sent = [];
  const fakeWindow = {
    removeMenu() {}, loadFile() {}, minimize() {}, close() {}, destroy() {}, on() {},
    webContents: { setWindowOpenHandler() {}, setZoomFactor() {}, setVisualZoomLevelLimits() {}, on() {}, send: (...args) => sent.push(args) },
  };
  const electron = {
    BrowserWindow: function () { return fakeWindow; },
    ipcMain: { handle: (name, fn) => { handlers[name] = fn; }, removeHandler: (name) => { delete handlers[name]; } },
  };
  const event = { sender: fakeWindow.webContents };
  let backs = 0;
  const backend = {
    install: async () => false, launch: async () => false, uninstallPlan: () => ({}),
    uninstall: async (_, { send }) => (send({ phase: "uninstalled", canUninstall: false }), true),
    back: () => (backs += 1, true),
  };
  openInstallerWindow(electron, { assets: "/x", backend, state: { phase: "uninstall-confirm", canUninstall: true, canGoBack: true } });
  assert.equal(await handlers["installer-back"](event), false, "до удаления «Назад» не работает");
  assert.ok(await handlers["installer-uninstall"](event, false));
  assert.equal(await handlers["installer-back"](event), true);
  assert.equal(backs, 1);
  // Удаление из «Программ и компонентов»: установщика нет, возвращаться некуда.
  openInstallerWindow(electron, { assets: "/x", backend: { ...backend, back: null }, state: { phase: "uninstall-confirm", canUninstall: true, canGoBack: false } });
  assert.ok(await handlers["installer-uninstall"](event, false));
  assert.equal(await handlers["installer-back"](event), false);
  assert.equal(backs, 1);
});

test("окно установщика: платформа и код ошибки антивируса доходят до страницы", async () => {
  const { openInstallerWindow } = require("../src/main/launcher-update/installer-window");
  const handlers = {};
  const sent = [];
  const fakeWindow = {
    removeMenu() {}, loadFile() {}, minimize() {}, close() {}, destroy() {}, on() {},
    webContents: { setWindowOpenHandler() {}, setZoomFactor() {}, setVisualZoomLevelLimits() {}, on() {}, send: (_, state) => sent.push(state) },
  };
  const opened = [];
  const electron = {
    BrowserWindow: function () { return fakeWindow; },
    ipcMain: { handle: (name, fn) => { handlers[name] = fn; }, removeHandler: (name) => { delete handlers[name]; } },
    shell: { openExternal: async (url) => { opened.push(url); } },
  };
  const event = { sender: fakeWindow.webContents };
  let fail = Object.assign(new Error("Антивирус заблокировал или удалил файл resources/app.asar (UNKNOWN)"), { code: "ANTIVIRUS", file: "resources/app.asar" });
  const backend = { install: async () => { throw fail; }, launch: async () => false, uninstallPlan: () => false, uninstall: async () => false };
  openInstallerWindow(electron, { assets: "/x", backend, state: { phase: "idle" } });
  assert.equal((await handlers["installer-get-state"](event)).platform, process.platform);
  assert.equal(await handlers["installer-open-discord"](event, "https://evil.example"), false, "Discord только из ошибки");
  assert.equal(await handlers["installer-install"](event, true), false);
  const failed = sent[sent.length - 1];
  assert.equal(failed.phase, "error");
  assert.equal(failed.errorCode, "ANTIVIRUS");
  assert.equal(failed.errorFile, "resources/app.asar");
  // Кнопка Discord в ошибке открывает только ссылку HLSR, адрес из окна не берётся.
  assert.equal(await handlers["installer-open-discord"](event, "https://evil.example"), true);
  assert.deepEqual(opened, [require("../src/shared/links").LINKS.discord]);
  // Следующая ошибка без кода не наследует объяснение про антивирус.
  fail = new Error("Нет места");
  await handlers["installer-install"](event, true);
  assert.equal(sent[sent.length - 1].errorCode, null);
  assert.equal(sent[sent.length - 1].errorFile, null);
});

test("окно установщика: ошибка ОС уходит кодом, сырой текст — в журнал; масштаб ограничивается после загрузки", async () => {
  const { openInstallerWindow } = require("../src/main/launcher-update/installer-window");
  const { InstallError } = require("../src/main/windows-install/install");
  const handlers = {}, sent = [], listeners = {}, logged = [];
  // Как Electron до загрузки страницы: вызов отказывает, кадра ещё нет.
  const zoom = { calls: 0, handled: 0 };
  const fakeWindow = {
    removeMenu() {}, loadFile() {}, minimize() {}, close() {}, destroy() {}, on() {},
    webContents: {
      setWindowOpenHandler() {}, setZoomFactor() {}, on: (name, fn) => { listeners[name] = fn; }, send: (_, state) => sent.push(state),
      setVisualZoomLevelLimits() {
        zoom.calls += 1;
        return { then(resolve, reject) { if (typeof reject === "function") zoom.handled += 1; if (reject) reject(new Error("Render frame was disposed")); } };
      },
    },
  };
  const electron = {
    BrowserWindow: function () { return fakeWindow; },
    ipcMain: { handle: (name, fn) => { handlers[name] = fn; }, removeHandler: (name) => { delete handlers[name]; } },
  };
  const event = { sender: fakeWindow.webContents };
  let fail = null;
  const backend = { install: async () => { throw fail; }, launch: async () => false, uninstallPlan: () => false, uninstall: async () => false };
  openInstallerWindow(electron, { assets: "/x", backend, log: { error: (line) => logged.push(line) }, state: { phase: "idle" } });
  assert.equal(zoom.calls, 0, "до загрузки страницы не вызывается");
  listeners["did-finish-load"]();
  await new Promise(setImmediate);
  assert.deepEqual(zoom, { calls: 1, handled: 1 }, "после загрузки, отказ обработан");

  const cause = (code, message) => Object.assign(new Error(message), { code });
  const cases = [
    [Object.assign(new InstallError("COPY_FAILED", "Не удалось скопировать resources/app.asar: ENOSPC: no space left on device, write"),
      { cause: cause("ENOSPC", "ENOSPC: no space left on device, write") }), "ENOSPC"],
    [new InstallError("IN_USE", "Файлы HLSR заняты (EBUSY)", cause("EBUSY", "EBUSY: resource busy or locked, rename")), "EBUSY"],
    [cause("EACCES", "EACCES: permission denied, mkdir 'C:\\HLSR'"), "EACCES"],
    [Object.assign(new InstallError("ANTIVIRUS", "Антивирус"), { cause: cause("UNKNOWN", "UNKNOWN: unknown error") }), "ANTIVIRUS"],
    [new InstallError("BAD_DIR", "Нельзя установить HLSR сюда"), "BAD_DIR"],
  ];
  for (const [error, code] of cases) {
    fail = error;
    assert.equal(await handlers["installer-install"](event, true), false);
    assert.equal(sent[sent.length - 1].errorCode, code, error.message);
  }
  assert.ok(logged.some((line) => line.includes("ENOSPC: no space left on device, write")), "сырой текст ошибки в журнале");

  // Новая ошибка без кода (запуск удаления, выбор папки) не наследует прежний код.
  fail = cases[0][0];
  await handlers["installer-install"](event, true);
  assert.equal(sent[sent.length - 1].errorCode, "ENOSPC");
  backend.uninstallPlan = ({ send }) => { send({ error: "Не удалось запустить удаление: EACCES" }); return false; };
  await handlers["installer-uninstall-plan"](event);
  assert.equal(sent[sent.length - 1].errorCode, null);
  assert.equal(sent[sent.length - 1].errorFile, null);
});

// Страница установщика (src/installer/installer.js) в Node: вместо DOM —
// элементы-заглушки по id, состояние подаётся как из installer-window.js.
function installerPage(plan = null) {
  const elements = new Map();
  const element = (tag) => ({
    tag, hidden: false, textContent: "", value: "", title: "", disabled: false, checked: false, className: "",
    dataset: {}, attributes: {}, children: [], listeners: {},
    setAttribute(name, value) { this.attributes[name] = value; }, removeAttribute(name) { delete this.attributes[name]; },
    addEventListener(name, fn) { this.listeners[name] = fn; },
    append(...nodes) { this.children.push(...nodes); }, replaceChildren(...nodes) { this.children = nodes; },
  });
  const $ = (id) => { if (!elements.has(id)) elements.set(id, element(`#${id}`)); return elements.get(id); };
  const document = {
    getElementById: $, querySelectorAll: () => [], addEventListener() {},
    createElement: element, createElementNS: (_, tag) => element(tag), documentElement: {}, body: { dataset: {} },
  };
  const calls = [];
  let push = null;
  const installer = {
    onState: (fn) => { push = fn; }, state: () => new Promise(() => {}),
    uninstallPlan: async () => plan, uninstall: async (games) => { calls.push(["uninstall", games]); },
    install: async () => {}, launch: async () => {}, cancel: async () => {}, back: async () => {}, windowCommand() {},
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../src/installer/installer.js"), "utf8"), { document, window: { installer } });
  const texts = (node) => node.children.length ? node.children.flatMap(texts) : node.textContent ? [node.textContent] : [];
  return { $, calls, render: (state) => push(state), texts, document, installer };
}

test("страница установщика: обновление не выглядит новой установкой, описания по фазе", () => {
  const page = installerPage();
  const update = { language: "ru", platform: "win32", directory: "C:\\HLSR\\Launcher", version: "0.4.9", installedVersion: "0.4.8", update: true };
  page.render({ ...update, phase: "idle" });
  page.render({ ...update, phase: "installing", progress: 40 });
  assert.equal(page.$("title").textContent, "Обновление HLSR");
  assert.deepEqual(page.texts(page.$("meta")), ["0.4.8", "0.4.9"]);
  assert.equal(page.$("step").textContent, "Обновляем файлы лаунчера");
  assert.equal(page.$("action-label").textContent, "Обновление…");
  assert.match(page.$("description").textContent, /^Обновляем HLSR/);
  page.render({ ...update, phase: "error", error: "x", errorCode: "BAD_DIR" });
  assert.equal(page.$("title").textContent, "Не удалось обновить");

  const fresh = installerPage();
  const install = { language: "ru", platform: "linux", directory: "/opt/hlsr", version: "0.4.9" };
  fresh.render({ ...install, phase: "idle" });
  for (const phase of ["verifying", "installing"]) {
    fresh.render({ ...install, phase });
    assert.equal(fresh.$("title").textContent, "Установка HLSR");
    assert.equal(fresh.$("description").textContent, "Ставим HLSR, это недолго.", phase);
  }
  fresh.render({ ...install, phase: "uninstall-confirm", uninstallPlan: {} });
  assert.match(fresh.$("description").textContent, /^Будут удалены/);
  fresh.render({ ...install, phase: "uninstalling" });
  assert.equal(fresh.$("description").textContent, "Удаляем HLSR, это недолго.");
});

test("страница установщика: ошибка ОС понятными словами без сырого текста", () => {
  for (const [language, code, hint] of [["ru", "ENOSPC", /не хватает места/], ["en", "ENOSPC", /out of space/],
    ["ru", "EBUSY", /файлы заняты/], ["ru", "EACCES", /Нет доступа/], ["en", "EPERM", /in use/], ["ru", "EIO", /Система не дала/]]) {
    const page = installerPage();
    const raw = `${code}: no space left on device, write`;
    page.render({ language, platform: "win32", phase: "error", directory: "C:\\HLSR\\Launcher", version: "0.4.9", error: `Не удалось скопировать resources/app.asar: ${raw}`, errorCode: code });
    assert.match(page.$("description").textContent, hint, `${language} ${code}`);
    assert.equal(page.$("error").hidden, false);
    assert.doesNotMatch(page.$("error-text").textContent, /no space left|Не удалось скопировать/);
    assert.match(page.$("error-text").textContent, new RegExp(code));
  }
  // Свои тексты HLSR (без кода ОС) показываются как есть.
  const page = installerPage();
  page.render({ language: "ru", platform: "win32", phase: "error", directory: "C:\\X", error: "HLSR сейчас запущен. Закройте лаунчер.", errorCode: null });
  assert.equal(page.$("error-text").textContent, "HLSR сейчас запущен. Закройте лаунчер.");
  assert.equal(page.$("description").textContent, "Что-то помешало установке, подробности ниже.");
});

test("страница установщика: ошибка страницы после ошибки ОС показывает свой текст, а не прежний код", async () => {
  const page = installerPage();
  page.render({ language: "ru", platform: "win32", phase: "error", directory: "C:\\X", canUninstall: true, error: "ENOSPC: …", errorCode: "ENOSPC" });
  page.installer.uninstallPlan = async () => { throw new Error("Не удалось запустить удаление"); };
  await page.$("uninstall").listeners.click();
  assert.equal(page.$("error-text").textContent, "Не удалось запустить удаление");
  assert.equal(page.$("description").textContent, "Что-то помешало установке, подробности ниже.");
});

test("страница установщика: «Также удалить игры» включено, размер по языку окна", async () => {
  const plan = { libraryExists: true, libraryBytes: 12.4 * 1073741824, libraryRoot: "C:\\HLSR" };
  const page = installerPage(plan);
  const done = { language: "ru", platform: "win32", phase: "done", directory: "C:\\HLSR\\Launcher", version: "0.4.9", installedVersion: "0.4.9", canUninstall: true };
  page.render(done);
  await page.$("uninstall").listeners.click();
  assert.equal(page.$("games").checked, true, "полное удаление по умолчанию");
  assert.equal(page.$("games-option").hidden, false);
  assert.equal(page.$("games-hint").textContent, "Вместе с сейвами, демками, настройками и входом · 12,4\u00a0ГБ · C:\\HLSR");
  await page.$("action").listeners.click();
  assert.deepEqual(page.calls, [["uninstall", true]]);
  page.render({ ...done, language: "en", phase: "uninstall-confirm", uninstallPlan: plan });
  assert.match(page.$("games-hint").textContent, /12\.4\u00a0GB/);
  page.render({ ...done, phase: "uninstall-confirm", uninstallPlan: { ...plan, libraryBytes: 512 * 1048576 } });
  assert.match(page.$("games-hint").textContent, /512\u00a0МБ/);
  // Windows: все очищаемые папки — библиотека и папка HLSR вокруг программы.
  page.render({ ...done, phase: "uninstall-confirm", uninstallPlan: { ...plan, libraryRoots: ["D:\\Lib", "C:\\HLSR"] } });
  assert.match(page.$("games-hint").textContent, /· D:\\Lib, C:\\HLSR$/);
  // Удаление из «Программ и компонентов» открывается сразу на подтверждении.
  assert.match(fs.readFileSync(path.join(__dirname, "../src/installer/index.html"), "utf8"), /<input id="games" type="checkbox" checked>/);
});

test("страница установщика: переключателя игр нет из-за защиты — окно говорит почему", () => {
  const page = installerPage();
  const confirm = { language: "ru", platform: "win32", phase: "uninstall-confirm", directory: "C:\\HLSR-QA\\Launcher", canUninstall: true };
  page.render({ ...confirm, uninstallPlan: { libraryExists: false, libraryBlocked: true, libraryRoot: "C:\\HLSR" } });
  assert.equal(page.$("games-option").hidden, true);
  assert.equal(page.$("error").hidden, false);
  assert.equal(page.$("error").dataset.tone, "warning");
  assert.equal(page.$("error-text").textContent, "Игры и настройки останутся: библиотекой пользуется другая установка HLSR.");
  page.render({ ...confirm, language: "en", uninstallPlan: { libraryExists: false, libraryBlocked: true } });
  assert.match(page.$("error-text").textContent, /another HLSR installation uses this library/);
  // Причина из плана: тестовая сборка и неподтверждённая установка — свои тексты.
  page.render({ ...confirm, uninstallPlan: { libraryExists: false, libraryBlocked: "test" } });
  assert.equal(page.$("error-text").textContent, "Тестовая сборка не удаляет игры и настройки.");
  page.render({ ...confirm, uninstallPlan: { libraryExists: false, libraryBlocked: "unverified" } });
  assert.match(page.$("error-text").textContent, /не удалось проверить/);
  page.render({ ...confirm, uninstallPlan: { libraryExists: false, libraryBlocked: "other" } });
  assert.match(page.$("error-text").textContent, /библиотекой пользуется другая установка HLSR/);
  // Библиотеки просто нет: строки нет.
  page.render({ ...confirm, uninstallPlan: { libraryExists: false, libraryBlocked: false } });
  assert.equal(page.$("error").hidden, true);
});

test("Linux: во вписанной руками библиотеке (~/Games) чужие Downloads, Tools, Demos, Backups остаются", (t) => {
  const uninstaller = require("../src/main/uninstall/launcher-uninstall");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-linux-typed-"));
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = path.join(dir, "data");
  t.after(() => {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
    fs.rmSync(dir, { recursive: true, force: true });
  });
  const root = path.join(dir, "opt", "hlsr");
  fs.mkdirSync(path.join(root, "versions", "0.0.1"), { recursive: true });
  fs.writeFileSync(path.join(root, ".hlsr-install.json"), JSON.stringify({ schema: 1 }));
  const library = path.join(dir, "Games");
  const files = {
    "Games/hl2-4260/installed.json": JSON.stringify({ gameId: "hl2-4260" }), "Games/hl1-won/readme.txt": "x",
    "Downloads/firefox.tar.gz": "x", "Tools/7z/7z": "x", "Demos/csgo/a.dem": "x", "Backups/photos.zip": "x", ".hlsr/library.json": "{}",
  };
  for (const [relative, body] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(library, relative)), { recursive: true });
    fs.writeFileSync(path.join(library, relative), body);
  }
  const result = uninstaller.uninstallLauncher({ installRoot: root, libraryRoot: library, removeGames: true, home: dir });
  const alive = Object.keys(files).filter((relative) => fs.existsSync(path.join(library, relative)));
  assert.deepEqual(alive.sort(), ["Backups/photos.zip", "Demos/csgo/a.dem", "Downloads/firefox.tar.gz", "Games/hl1-won/readme.txt", "Tools/7z/7z"]);
  assert.ok(result.left.includes("Downloads") && result.left.includes(path.join("Games", "hl1-won")), result.left.join());

  // Своя папка «hlsr» (не ровно HLSR) — тоже чужая раскладка.
  const own = path.join(dir, "hlsr");
  fs.mkdirSync(path.join(root, "versions", "0.0.1"), { recursive: true });
  fs.writeFileSync(path.join(root, ".hlsr-install.json"), JSON.stringify({ schema: 1 }));
  fs.mkdirSync(path.join(own, "Downloads"), { recursive: true });
  fs.writeFileSync(path.join(own, "Downloads", "mine.zip"), "x");
  uninstaller.uninstallLauncher({ installRoot: root, libraryRoot: own, removeGames: true, home: dir });
  assert.equal(fs.existsSync(path.join(own, "Downloads", "mine.zip")), true);
});

test("страница установщика: бегущий отрезок прогресса всё время на дорожке", () => {
  const css = fs.readFileSync(path.join(__dirname, "../src/installer/style.css"), "utf8");
  const rule = /progress:indeterminate::after \{([^}]*)\}/.exec(css)[1];
  const width = Number(/width: ([\d.]+)%/.exec(rule)[1]);
  const keyframes = /@keyframes indeterminate \{\s*from \{ transform: translateX\(([-\d.]+)%?\); \}\s*to \{ transform: translateX\(([-\d.]+)%\); \}/.exec(css);
  const [from, to] = [Number(keyframes[1]), Number(keyframes[2])];
  // Сдвиг translateX — в долях ширины отрезка; отрезок целиком внутри [0, 100] %.
  assert.ok(from >= 0, `начало ${from}%`);
  assert.ok((to / 100) * width + width <= 100.001, `конец ${to}% при ширине ${width}%`);
});
test("autonomous installer installs the embedded launcher offline and updates older installs", { skip: NO_SYMLINKS || NO_EXEC_BIT }, async (t) => {
  const { openInstaller, versionOf } = require("../src/main/launcher-update/BootstrapInstaller");
  const embeddedPayload = require("../src/main/launcher-update/embedded-payload");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-installer-offline-"));
  const previousXdg = process.env.XDG_DATA_HOME;
  const previousResourcesPath = process.resourcesPath;
  process.env.XDG_DATA_HOME = path.join(dir, "data");
  process.resourcesPath = dir;
  t.after(() => {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
    process.resourcesPath = previousResourcesPath;
    fs.rmSync(dir, { recursive: true, force: true });
  });
  const root = path.join(dir, "opt", "hlsr");
  function payloadFor(f) {
    const directory = path.join(f.dir, "payload");
    fs.mkdirSync(directory, { recursive: true });
    fs.copyFileSync(f.file, path.join(directory, path.basename(f.file)));
    const { size, sha256, sha512, unpackedSize } = f.archive.installer;
    const descriptor = embeddedPayload.describe({ version: f.archive.version, size, sha256, sha512, unpackedSize });
    fs.writeFileSync(path.join(directory, embeddedPayload.DESCRIPTOR), JSON.stringify(descriptor));
    return directory;
  }
  function open(payload) {
    const handlers = {}, sent = [];
    const fakeWindow = {
      removeMenu() {}, loadFile() {}, minimize() {}, close() {}, destroy() {}, on() {},
      webContents: { setWindowOpenHandler() {}, setZoomFactor() {}, setVisualZoomLevelLimits() {}, on() {}, send: (...args) => sent.push(args) },
    };
    const electron = {
      app: {
        getLocale: () => "ru-RU", quit() {},
        getPath: (name) => path.join(dir, name === "userData" ? "config/installer" : name),
      },
      BrowserWindow: function () { return fakeWindow; },
      ipcMain: { handle: (name, fn) => { handlers[name] = fn; }, removeHandler: (name) => { delete handlers[name]; } },
    };
    openInstaller(electron, { root, assets: dir, payload });
    const event = { sender: fakeWindow.webContents };
    return {
      state: () => handlers["installer-get-state"](event),
      install: () => handlers["installer-install"](event, false),
      last: () => sent[sent.length - 1][1],
    };
  }
  // Первая установка: ни канала, ни скачивания, только встроенный архив.
  const first = fixture(t, "0.3.18");
  const fresh = open(payloadFor(first));
  const initial = await fresh.state();
  assert.equal(initial.phase, "idle");
  assert.equal(initial.version, "0.3.18");
  assert.equal(initial.update, false);
  assert.equal(await fresh.install(), true);
  assert.equal(fresh.last().phase, "done");
  assert.equal(versionOf(Install.readCurrent(root)), "0.3.18");
  assert.ok(fs.statSync(path.join(root, "start")).mode & 0o111);
  // Установщик новее установленной версии предлагает обновление.
  const second = fixture(t, "0.3.19");
  const secondPayload = payloadFor(second);
  const upgrade = open(secondPayload);
  const offered = await upgrade.state();
  assert.equal(offered.phase, "idle");
  assert.equal(offered.update, true);
  assert.equal(offered.installedVersion, "0.3.18");
  assert.equal(await upgrade.install(), true);
  assert.equal(versionOf(Install.readCurrent(root)), "0.3.19");
  // Та же версия уже стоит: установщик сразу предлагает открыть HLSR.
  assert.equal((await open(secondPayload).state()).phase, "done");
  // Испорченный встроенный архив не ставится, установленная версия цела.
  const third = fixture(t, "0.3.20");
  const brokenPayload = payloadFor(third);
  fs.appendFileSync(path.join(brokenPayload, path.basename(third.file)), "tampered");
  const broken = open(brokenPayload);
  assert.equal(await broken.install(), false);
  assert.equal(broken.last().phase, "error");
  assert.equal(versionOf(Install.readCurrent(root)), "0.3.19");
  // Без описания установщик сразу сообщает, что он повреждён.
  fs.unlinkSync(path.join(brokenPayload, embeddedPayload.DESCRIPTOR));
  const missing = await open(brokenPayload).state();
  assert.equal(missing.phase, "error");
  assert.match(missing.error, /Скачайте установщик заново/);
});
test("launcher extraction uses unpatched fs (Electron throws Invalid package on *.asar)", async (t) => {
  // Регресс: внутри Electron `fs` пропатчен под asar и бросает
  // `Invalid package .../resources/app.asar` на создании/стате частично
  // записанного app.asar. archive.js и ArchiveInstall обязаны идти через
  // fs-compat (original-fs в Electron, fs в Node).
  const sources = {
    archive: fs.readFileSync(path.join(__dirname, "../src/main/launcher-update/archive.js"), "utf8"),
    install: fs.readFileSync(path.join(__dirname, "../src/main/launcher-update/ArchiveInstall.js"), "utf8"),
  };
  for (const [name, text] of Object.entries(sources))
    assert.ok(!/require\(["']fs["']\)/.test(text), `${name}.js обязан использовать fs-compat вместо патченного fs`);
  assert.ok(sources.archive.includes('require("./fs-compat")'), "archive.js должен требовать fs-compat");
  assert.ok(sources.install.includes('require("./fs-compat")'), "ArchiveInstall.js должен требовать fs-compat");
  const compat = require("../src/main/launcher-update/fs-compat");
  assert.equal(typeof compat.openSync, "function");
  assert.equal(compat, require("fs"), "в чистом Node fs-compat обязан совпадать с fs");
  const { modules } = require("./build-bootstrap-app");
  assert.ok(modules.includes("src/main/launcher-update/fs-compat.js"), "fs-compat обязан попадать в bootstrap AppImage");
  // Сквозная проверка: tar с resources/app.asar ставится через ArchiveInstall.
  const f = fixture(t, "0.3.20");
  const root = path.join(f.dir, "asar-install");
  const install = new Install(root);
  await install.prepare(f.file, f.archive);
  assert.equal(fs.statSync(path.join(install.stage, "resources/app.asar")).size,
    fs.statSync(path.join(f.source, "resources/app.asar")).size);
  // Установка переключает версии символическими ссылками.
  if (NO_SYMLINKS) return t.skip(NO_SYMLINKS);
  install.commit();
  assert.ok(Install.readCurrent(root));
  // Брошенные .staging-* от упавших установок подчищаются следующим prepare.
  const orphan = fs.mkdtempSync(path.join(root, "versions", ".staging-"));
  fs.writeFileSync(path.join(orphan, "resources-app.asar-partial"), "garbage");
  const retry = new Install(root);
  const second = fixture(t, "0.3.21");
  await retry.prepare(second.file, second.archive);
  assert.equal(fs.existsSync(orphan), false);
  retry.discard();
});
