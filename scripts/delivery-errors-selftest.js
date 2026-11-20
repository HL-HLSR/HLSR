"use strict";

// Ошибки доставки: блокировка файла антивирусом на Windows, смена папки
// библиотеки во время работы, файлы игрока при обновлении.
// node --test scripts/delivery-errors-selftest.js

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { PassThrough } = require("stream");
const { isAntivirusBlock, toDeliveryError } = require("../src/main/game-delivery/errors");

const ARCHIVE = "C:\\Games\\Downloads\\hl1-won\\hl1-won-1.0.0.7z";

function unknownOpen() {
  // Так Node отдаёт ERROR_VIRUS_INFECTED: libuv этот код не знает.
  return Object.assign(new Error(`UNKNOWN: unknown error, open '${ARCHIVE}'`), { code: "UNKNOWN", errno: -4094, syscall: "open", path: ARCHIVE });
}

test("UNKNOWN при открытии файла на Windows — блокировка антивирусом", () => {
  assert.equal(isAntivirusBlock(unknownOpen(), "win32"), true);
  assert.equal(isAntivirusBlock(unknownOpen(), "linux"), false, "на Linux UNKNOWN — не антивирус");
  assert.equal(isAntivirusBlock(Object.assign(new Error("EBUSY"), { code: "EBUSY", syscall: "open" }), "win32"), false, "занятый файл — не антивирус");
  assert.equal(isAntivirusBlock({ stderr: "ERROR: hl1-won.7z\nOperation did not complete successfully because the file contains a virus or potentially unwanted software." }, "win32"), true);
  assert.equal(isAntivirusBlock({ stderr: "Операция не была успешно завершена, так как файл содержит вирус или потенциально нежелательную программу." }, "win32"), true);
  assert.equal(isAntivirusBlock({ stderr: "ERROR: Data Error : hl1/valve/pak0.pak" }, "win32"), false, "битый архив — не антивирус");
});

test("чтение скачанного архива: вместо «unknown error» — понятная ошибка с именем файла", async (t) => {
  const platform = Object.getOwnPropertyDescriptor(process, "platform");
  Object.defineProperty(process, "platform", { value: "win32" });
  t.after(() => Object.defineProperty(process, "platform", platform));
  t.mock.method(fs, "createReadStream", () => {
    const stream = new PassThrough();
    process.nextTick(() => stream.emit("error", unknownOpen()));
    return stream;
  });

  const InstallService = require("../src/main/game-delivery/InstallService");
  const error = await InstallService.prototype.hashFile.call({}, { cancelRequested: false }, ARCHIVE).then(() => null, (e) => e);
  assert.equal(error.code, "ANTIVIRUS_BLOCKED");
  assert.match(error.message, /«hl1-won-1\.0\.0\.7z»/);
  assert.match(error.message, /исключения антивируса/);
  assert.equal(error.details.path, ARCHIVE);

  assert.equal(toDeliveryError(unknownOpen()).code, "ANTIVIRUS_BLOCKED", "toDeliveryError не узнал блокировку");
  assert.equal(toDeliveryError(new Error("boom")).code, "INSTALL_FAILED");
});

const localeHas = (key) =>
  ["ru", "en"].every((lang) =>
    fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes(`"${key}":`)
  );

test("папка библиотеки не меняется, пока идёт загрузка, установка или игра", () => {
  const { setLibraryPath } = require("../src/main/game-delivery").prototype;
  const idle = (dispose) => ({
    libraryPath: "C:\\HLSR",
    service: { isBusy: () => false, processService: { anyRunning: () => null }, silentVersionEvents: new Set(), dispose },
    tools: { busyWith: null, running: new Map() },
    launchService: { launching: new Set() },
  });
  const busy = {
    "загрузка или установка": (c) => (c.service.isBusy = () => true),
    "игра запущена": (c) => (c.service.processService.anyRunning = () => "hl1-won"),
    "база стоит, патчи ещё нет": (c) => c.service.silentVersionEvents.add("hl1-won"),
    "ставится LiveSplit": (c) => (c.tools.busyWith = "livesplit"),
    "LiveSplit запущен": (c) => c.tools.running.set("livesplit", {}),
    "готовится запуск игры": (c) => c.launchService.launching.add("hl1-won"),
  };

  for (const [name, makeBusy] of Object.entries(busy)) {
    const context = Object.assign(idle(() => assert.fail(`${name}: прежнюю службу остановили`)), {
      ensureService: () => assert.fail(`${name}: создана новая служба`),
    });
    makeBusy(context);
    const service = context.service;
    assert.deepEqual(setLibraryPath.call(context, "D:\\HLSR"), { ok: false, error: "busy" }, name);
    assert.equal(context.libraryPath, "C:\\HLSR", name);
    assert.equal(context.service, service, name);
  }

  // Без работы — как раньше: прежняя служба остановлена, новая смотрит в новую папку.
  let disposed = false;
  const context = Object.assign(idle(() => (disposed = true)), {
    ensureService() {
      this.createdFor = this.libraryPath;
    },
  });
  assert.equal(setLibraryPath.call(context, "D:\\HLSR"), undefined);
  assert.equal(disposed, true);
  assert.equal(context.createdFor, "D:\\HLSR");
  assert.ok(localeHas("#L_S_PATH_BUSY"), "текст для окна");
});

test("файл игрока не скопировался — обновление прервано до подмены, прежняя сборка цела", async (t) => {
  const InstallService = require("../src/main/game-delivery/InstallService");
  const { resolveLibrary, gameInstallPath } = require("../src/main/game-delivery/paths");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-preserve-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = resolveLibrary(root);
  fs.mkdirSync(paths.state, { recursive: true });
  const installPath = gameInstallPath(paths, "hl1-won");
  const old = {
    "hl.exe": "старая сборка",
    "valve/first.dem": "демо 1",
    "valve/second.dem": "демо 2",
    "installed.json": JSON.stringify({ state: "ready", version: "1.0.0" }),
  };
  for (const [relative, text] of Object.entries(old)) {
    fs.mkdirSync(path.dirname(path.join(installPath, relative)), { recursive: true });
    fs.writeFileSync(path.join(installPath, relative), text);
  }

  const service = new InstallService({
    paths,
    spaceMargin: 0,
    archiveService: {
      inspect: async () => ({ listing: { entries: [] }, report: { uncompressedSize: 3 } }),
      extract: (archive, staging) => ({
        cancel() {},
        promise: Promise.resolve().then(() => fs.writeFileSync(path.join(staging, "hl.exe"), "новая")),
      }),
    },
  });
  t.mock.method(service, "verifyArchive", async () => "sha");
  t.mock.method(service, "checkSpace");
  const copyFileSync = fs.copyFileSync;
  t.mock.method(fs, "copyFileSync", (from, to, mode) => {
    if (from.endsWith("second.dem"))
      throw Object.assign(new Error("EBUSY: resource busy or locked, copyfile"), { code: "EBUSY", path: from });
    return copyFileSync(from, to, mode);
  });

  const manifest = { gameId: "hl1-won", version: "1.0.1", expectedInstalledSize: 3, preservePaths: [], baseArchive: {}, filesManifest: { requiredFiles: ["hl.exe"] } };
  const update = { fromVersion: "1.0.0", filesManifest: { version: "1.0.1" }, integrityService: { check: async () => ({ ok: true, extra: [] }) } };
  await assert.rejects(service.install({ manifest, update, archivePath: path.join(root, "hl1-won.7z") }), (error) => {
    assert.equal(error.code, "PRESERVE_FAILED");
    assert.match(error.message, /second\.dem».*Закрой программу/, "в тексте файл и что делать");
    return true;
  });

  assert.equal(fs.readFileSync(path.join(installPath, "hl.exe"), "utf8"), "старая сборка", "новая сборка не подменила старую");
  assert.equal(fs.readFileSync(path.join(installPath, "valve", "second.dem"), "utf8"), "демо 2", "файл игрока на месте");
  assert.deepEqual(fs.readdirSync(paths.staging), [], "распакованная новая сборка убрана");
  assert.equal(
    service.checkSpace.mock.calls[0].arguments[1],
    Buffer.byteLength(old["valve/first.dem"]) + Buffer.byteLength(old["valve/second.dem"]),
    "место под копии файлов игрока учтено"
  );
  assert.ok(localeHas("#L_D_ERR_PRESERVE_FAILED"), "текст для окна");
});

test("сборка из депотов Steam: файл игрока не скопировался — установка прервана, прежняя сборка цела", async (t) => {
  const DepotInstaller = require("../src/main/game-delivery/depot-install");
  const { resolveLibrary, gameInstallPath } = require("../src/main/game-delivery/paths");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-depot-preserve-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = resolveLibrary(root);
  const installPath = gameInstallPath(paths, "hl2-5288");
  const write = (dir, files) => {
    for (const [relative, text] of Object.entries(files)) {
      fs.mkdirSync(path.dirname(path.join(dir, relative)), { recursive: true });
      fs.writeFileSync(path.join(dir, relative), text);
    }
  };
  write(installPath, { "hl2.exe": "старая сборка", "hl2/gameinfo.txt": "старая", "hl2/SAVE/quick.sav": "сейв" });

  const installer = new DepotInstaller({
    paths,
    depotService: {
      download: async (gameId, staging) => {
        const dir = path.join(staging, "depot");
        write(dir, { "hl2.exe": "новая", "hl2/gameinfo.txt": "новая" });
        return { depots: [{ dir }] };
      },
    },
  });
  const copyFileSync = fs.copyFileSync;
  t.mock.method(fs, "copyFileSync", (from, to, mode) => {
    if (from.endsWith("quick.sav"))
      throw Object.assign(new Error("EBUSY: resource busy or locked, copyfile"), { code: "EBUSY", path: from });
    return copyFileSync(from, to, mode);
  });

  await assert.rejects(installer.install("hl2-5288"), (error) => {
    assert.equal(error.code, "PRESERVE_FAILED");
    assert.match(error.message, /quick\.sav».*Закрой программу/, "в тексте файл и что делать");
    return true;
  });
  assert.equal(fs.readFileSync(path.join(installPath, "hl2.exe"), "utf8"), "старая сборка", "новая сборка не подменила старую");
  assert.equal(fs.readFileSync(path.join(installPath, "hl2", "SAVE", "quick.sav"), "utf8"), "сейв", "сейв на месте");
  assert.equal(fs.existsSync(`${installPath}.hlsr-backup`), false);
  assert.deepEqual(fs.readdirSync(paths.staging), [], "скачанная сборка убрана");
});

test("файл игрока не скопировался: антивирус, нет места или файл занят — у каждого свой код и совет", (t) => {
  const { preserveError, DeliveryError } = require("../src/main/game-delivery/errors");
  const platform = Object.getOwnPropertyDescriptor(process, "platform");
  Object.defineProperty(process, "platform", { value: "win32" });
  t.after(() => Object.defineProperty(process, "platform", platform));
  const demo = "C:\\HLSR\\Games\\hl1-won\\valve\\run.dem";
  const fsError = (code) => Object.assign(new Error(`${code}: copyfile '${demo}'`), { code, syscall: "copyfile", path: demo });

  assert.equal(preserveError(fsError("UNKNOWN")).code, "ANTIVIRUS_BLOCKED", "блокировка антивирусом — совет про исключения");
  assert.equal(preserveError(fsError("ENOSPC")).code, "NO_SPACE");
  const busy = preserveError(fsError("EBUSY"));
  assert.equal(busy.code, "PRESERVE_FAILED");
  assert.match(busy.message, /«C:\\HLSR\\Games\\hl1-won\\valve\\run\.dem» \(EBUSY\)\. Закрой программу, которая его держит, и повтори/);
  assert.equal(busy.details.path, demo);
  const own = new DeliveryError("PATH_INVALID", "выход за папку");
  assert.equal(preserveError(own), own);

  for (const lang of ["ru", "en"]) {
    const locale = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    assert.match(locale, lang === "ru" ? /"#L_D_ERR_PRESERVE_FAILED": "[^"]*Закрой программу/ : /"#L_D_ERR_PRESERVE_FAILED": "[^"]*Close any program/, lang);
  }
});

test("PRESERVE_FAILED доходит до центра уведомлений", () => {
  const { notify } = require("../src/main/game-delivery").prototype;
  const added = [];
  notify.call({ notifications: { add: (item) => added.push(item) } }, "PRESERVE_FAILED", "демка занята");
  assert.deepEqual(added, [{ level: "error", code: "PRESERVE_FAILED", detail: "демка занята" }]);
});

test("полное обновление: место под копии файлов игрока проверяется до загрузки", async (t) => {
  const GameDeliveryService = require("../src/main/game-delivery/GameDeliveryService");
  const InstallService = require("../src/main/game-delivery/InstallService");
  const { resolveLibrary, gameInstallPath } = require("../src/main/game-delivery/paths");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-update-space-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const paths = resolveLibrary(root);
  const installPath = gameInstallPath(paths, "hl1-won");
  fs.mkdirSync(path.join(installPath, "valve"), { recursive: true });
  fs.writeFileSync(path.join(installPath, "installed.json"), JSON.stringify({ schemaVersion: 1, state: "ready", version: "1.0.0" }));
  fs.writeFileSync(path.join(installPath, "valve", "run.dem"), Buffer.alloc(1000));
  // Места хватает на архив (10), сборку (20) и запас, но не на копию демки.
  t.mock.method(fs, "statfsSync", () => ({ bsize: 1, bavail: 10 + 20 + 512 * 1024 * 1024 + 999 }));

  let downloaded = false;
  const context = {
    paths,
    env: {},
    installService: new InstallService({ paths }),
    manifestFor: () => ({ manifest: { expectedInstalledSize: 20, preservePaths: [] } }),
    targetFor: () => ({ version: "1.0.1", source: "channel", fullArchive: { version: "1.0.1", size: 10, sha256: "a".repeat(64), providers: [] } }),
    resolveFilesManifest: async () => ({ totalSize: 20 }),
    findLocalArchive: () => null,
    downloadWithFallback: async () => {
      downloaded = true;
      throw new Error("загрузка до проверки места");
    },
    checkFullUpdateSpace: GameDeliveryService.prototype.checkFullUpdateSpace,
    setOperation() {},
    setStatus() {},
    statusOf: () => ({ state: "error" }),
    refresh() {},
  };
  await assert.rejects(GameDeliveryService.prototype.applyFullUpdate.call(context, "hl1-won", { kind: "full" }), { code: "NO_SPACE" });
  assert.equal(downloaded, false, "архив не качался");
});
