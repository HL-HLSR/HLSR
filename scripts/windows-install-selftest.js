"use strict";

// Как в официальной сборке: эти значения подставляет webpack (src/main/security/official.js).
process.env.HLSR_API_HOST = process.env.HLSR_API_HOST || "api.hlsr.pro";
process.env.HLSR_DISCORD_APP_ID = process.env.HLSR_DISCORD_APP_ID || "100000000000000001";
// Установка и удаление HLSR в Windows (src/main/windows-install). Логика
// проверяется и в Linux: реестр, ярлыки и cmd.exe подменяются, файлы —
// настоящие во временном каталоге.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), os = require("os"), path = require("path"), crypto = require("crypto");
const { Readable } = require("stream");
const fsCompat = require("../src/main/launcher-update/fs-compat");
const layout = require("../src/main/windows-install/layout");
const fileList = require("../src/main/windows-install/file-list");
const { install, existingKind, originOf } = require("../src/main/windows-install/install");
const uninstaller = require("../src/main/windows-install/uninstall");
const { toRegFile, parseRegFile, createRegistry } = require("../src/main/windows-install/registry");
const setup = require("../src/main/windows-install/setup");
const buildSetup = require("./build-windows-setup");

const PRODUCTION = layout.identity();
const quiet = { info() {}, warn() {}, error() {} };

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-win-install-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Распакованная сборка: файлы и install-files.json, как после build-windows-setup.
function source(root, version, files, app = layout.PRODUCTION) {
  fs.mkdirSync(path.join(root, "resources"), { recursive: true });
  for (const [name, body] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
    fs.writeFileSync(path.join(root, name), body);
  }
  const list = fileList.build(root, version, app);
  fs.writeFileSync(path.join(root, fileList.RELATIVE), JSON.stringify(list));
  return list;
}

function memoryRegistry(initial = {}) {
  const keys = JSON.parse(JSON.stringify(initial));
  return {
    keys,
    read: (key) => (keys[key] ? { ...keys[key] } : null),
    apply(changes) {
      for (const change of changes) {
        if (change.remove) { delete keys[change.key]; continue; }
        const values = (keys[change.key] = keys[change.key] || {});
        for (const [name, value] of Object.entries(change.values)) {
          if (value === null) delete values[name];
          else values[name] = typeof value === "object" ? value.dword : value;
        }
      }
    },
  };
}

function fakeShortcuts(dir, identity = PRODUCTION) {
  const startMenu = path.join(dir, "start", `${identity.shortcutName}.lnk`);
  const desktop = path.join(dir, "desktop", `${identity.shortcutName}.lnk`);
  const written = [];
  return {
    startMenu, desktop, written,
    write(file, exe) {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, exe);
      written.push(file);
    },
    exists: (file) => fs.existsSync(file),
    pointsTo: (file, exe) => fs.existsSync(file) && fs.readFileSync(file, "utf8") === exe,
  };
}

function fixture(t, version = "0.3.17", files = null) {
  const dir = tempDir(t);
  const src = path.join(dir, "setup-unpacked");
  source(src, version, files || { "HLSR.exe": `exe ${version}`, "resources/app.asar": `asar ${version}`, "locales/ru.pak": "ru" });
  return {
    dir, src,
    installDir: path.join(dir, "HLSR", "Launcher"),
    libraryRoot: path.join(dir, "Library"),
    registry: memoryRegistry(),
    shortcuts: fakeShortcuts(dir),
  };
}

function runInstall(f, extra = {}) {
  return install({
    sourceDir: f.src, installDir: f.installDir, registry: f.registry, shortcuts: f.shortcuts,
    libraryRoot: f.libraryRoot, env: {}, log: quiet, sleep: async () => {}, ...extra,
  });
}

test("identity: боевой HLSR получает ключи прежнего NSIS-установщика", () => {
  assert.equal(PRODUCTION.guid, "3f824379-6b41-5725-b424-59cb87f2acae");
  assert.equal(PRODUCTION.installKey, "HKCU\\Software\\3f824379-6b41-5725-b424-59cb87f2acae");
  assert.equal(PRODUCTION.uninstallKey, "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\3f824379-6b41-5725-b424-59cb87f2acae");
  assert.equal(PRODUCTION.defaultInstallDir, "C:\\HLSR\\Launcher");
  assert.equal(PRODUCTION.shortcutName, "HLSR");
  // Маркер владения, который записал NSIS 0.3.16 (байт в байт).
  assert.equal(PRODUCTION.ownerMarker("{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}"),
    '{"schemaVersion":1,"appId":"3f824379-6b41-5725-b424-59cb87f2acae","installId":"{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}"}');

  const qa = layout.identity({ appId: "com.hlsrteam.hlsr.qa", productName: "HLSR-QA" });
  assert.equal(qa.guid, "a4c160bc-6860-53ec-9475-5b670bd2800e");
  assert.equal(qa.defaultInstallDir, "C:\\HLSR-QA\\Launcher");
  assert.equal(qa.shortcutName, "HLSR-QA");
  assert.throws(() => layout.identity({ appId: "x", productName: "a\\b" }), /имя продукта/);
});

test("parseArgs: аргументы обновлятора и удаления, /D= до конца строки", () => {
  const update = layout.parseArgs(["HLSR.exe", "/S", "--updated", "/D=C:\\Games\\My HLSR"]);
  assert.equal(update.silent, true);
  assert.equal(update.updated, true);
  assert.equal(update.dir, "C:\\Games\\My HLSR");
  const quick = layout.parseArgs(["HLSR.exe", "--updated", "/s", "--force-run"]);
  assert.equal(quick.silent, true);
  assert.equal(quick.forceRun, true);
  assert.equal(quick.dir, null);
  const remove = layout.parseArgs(["HLSR.exe", "--hlsr-uninstall", "/S", "/HLSRDELETEGAMES", "/HLSRDELETESETTINGS"]);
  assert.equal(remove.uninstall, true);
  assert.equal(remove.deleteGames, true);
  assert.equal(remove.deleteSettings, true);
  assert.equal(remove.deleteProfile, false);
});

test("detect: установщик только из распакованного Setup или с флагом удаления", () => {
  assert.equal(setup.detect(["HLSR.exe"], {}), null);
  assert.equal(setup.detect(["HLSR.exe", "/S"], { PORTABLE_EXECUTABLE_FILE: "C:\\D\\HLSR-Setup-1.0.0.exe" }).mode, "setup");
  assert.equal(setup.detect(["HLSR.exe", "--hlsr-uninstall"], {}).mode, "uninstall");
  const env = setup.launchEnvironment({ PATH: "x", PORTABLE_EXECUTABLE_FILE: "a", PORTABLE_EXECUTABLE_DIR: "b" });
  assert.deepEqual(env, { PATH: "x" });
});

test("launchInstalled: EACCES свежего exe повторяется, флаг администратора — через оболочку", async () => {
  const { EventEmitter } = require("events");
  const app = { releaseSingleInstanceLock() {} };
  const failing = (code, okAfter) => {
    let calls = 0;
    const fn = (exe, args) => {
      calls += 1;
      fn.args = args;
      const child = new EventEmitter();
      child.unref = () => {};
      setImmediate(() => {
        if (okAfter && calls > okAfter) child.emit("spawn");
        else child.emit("error", Object.assign(new Error(`spawn HLSR.exe ${code}`), { code }));
      });
      return child;
    };
    fn.calls = () => calls;
    return fn;
  };
  const sleep = async () => {};
  const noFlag = () => ({ status: 1, stdout: "" });
  const dir = "C:\\HLSR\\Launcher";
  const shellCalls = [];
  const shellStart = async (exe, args) => { shellCalls.push({ exe, args }); return true; };

  // «Запускать от имени администратора» в свойствах exe: сразу оболочка,
  // без попыток CreateProcess, и с аргументами (удаление).
  const flagged = (args) => ({
    status: args[1].startsWith("HKCU") ? 0 : 1,
    stdout: `\r\n    ${args[3]}    REG_SZ    ~ RUNASADMIN\r\n`,
  });
  const neverCalled = failing("EACCES");
  await setup.launchInstalled(app, dir, { spawn: neverCalled, sleep, regQuery: flagged, shellStart }, [layout.UNINSTALL_FLAG]);
  assert.equal(neverCalled.calls(), 0);
  assert.equal(shellCalls.length, 1);
  assert.match(shellCalls[0].exe, /HLSR\.exe$/);
  assert.deepEqual(shellCalls[0].args, [layout.UNINSTALL_FLAG]);
  assert.equal(setup.runsAsAdmin("C:\\X\\HLSR.exe", () => ({ status: 0, stdout: "HLSR.exe REG_SZ ~ HIGHDPIAWARE" })), false);

  // Антивирус отпустил exe на третьей попытке: оболочка не нужна.
  const busy = failing("EACCES", 2);
  await setup.launchInstalled(app, dir, { spawn: busy, sleep, shellStart, regQuery: noFlag }, ["--x"]);
  assert.equal(busy.calls(), 3);
  assert.deepEqual(busy.args, ["--x"]);
  assert.equal(shellCalls.length, 1);

  // Флаг не виден в реестре, CreateProcess отказывает всегда: в конце оболочка.
  await setup.launchInstalled(app, dir, { spawn: failing("EACCES"), sleep, shellStart, regQuery: noFlag });
  assert.equal(shellCalls.length, 2);

  // Оболочка тоже не смогла: наружу уходит исходная ошибка, а не uncaughtException.
  await assert.rejects(setup.launchInstalled(app, dir,
    { spawn: failing("EACCES"), sleep, shellStart: async () => false, regQuery: noFlag }), { code: "EACCES" });

  // Другие ошибки не повторяются.
  const missing = failing("ENOENT");
  await assert.rejects(setup.launchInstalled(app, dir, { spawn: missing, sleep, shellStart, regQuery: noFlag }), { code: "ENOENT" });
  assert.equal(missing.calls(), 1);
});

test("resolveInstallDir: /D=, затем сохранённый каталог, затем каталог по умолчанию", () => {
  const dir = path.join(os.tmpdir(), "hlsr-somewhere");
  assert.equal(setup.resolveInstallDir({ dir }, { InstallLocation: "/elsewhere" }, PRODUCTION), dir);
  assert.equal(setup.resolveInstallDir({}, { InstallLocation: dir }, PRODUCTION), dir);
  // Каталог, который NSIS записал до завершения установки, не считается.
  assert.equal(setup.resolveInstallDir({}, { InstallLocation: dir, [layout.SEED_FLAG]: "1" }, PRODUCTION), PRODUCTION.defaultInstallDir);
  assert.equal(setup.resolveInstallDir({}, {}, PRODUCTION), PRODUCTION.defaultInstallDir);
});

test("dataCleanupArgs: флаги прежнего деинсталлятора", () => {
  assert.deepEqual(setup.dataCleanupArgs({}), []);
  assert.deepEqual(setup.dataCleanupArgs({ deleteSettings: true }), ["--settings", "--keep-profile"]);
  assert.deepEqual(setup.dataCleanupArgs({ deleteSettings: true, deleteProfile: true }), ["--settings"]);
  assert.deepEqual(setup.dataCleanupArgs({ deleteProfile: true }), ["--delete-profile"]);
});

test("refusedDir: системные, относительные и опасные пути", () => {
  const home = path.join(os.tmpdir(), "hlsr-user");
  assert.match(layout.refusedDir("relative/dir", {}), /не абсолютный/);
  assert.match(layout.refusedDir(path.parse(process.cwd()).root, {}), /корень/);
  assert.match(layout.refusedDir([os.tmpdir(), "a", "..", "b"].join(path.sep), {}), /«\.\.»/);
  assert.match(layout.refusedDir(path.join(os.tmpdir(), "a&b"), {}), /недопустимые/);
  assert.match(layout.refusedDir(home, { USERPROFILE: home }), /системный/);
  assert.match(layout.refusedDir(path.join(home, "Desktop"), { USERPROFILE: home }), /системный/);
  assert.equal(layout.refusedDir(path.join(home, "HLSR"), { USERPROFILE: home }), null);
});

test("registry: .reg-файл и разбор экспорта, кириллица и кавычки", (t) => {
  const text = toRegFile([
    { key: "HKCU\\Software\\X", values: { InstallLocation: "C:\\Игры\\HLSR", Quote: 'a "b"', Size: { dword: 0x5f5ff }, Gone: null } },
    { key: "HKCU\\Software\\Old", remove: true },
  ]);
  assert.match(text, /^Windows Registry Editor Version 5\.00\r\n/);
  assert.ok(text.includes('"InstallLocation"="C:\\\\Игры\\\\HLSR"'));
  assert.ok(text.includes('"Size"=dword:0005f5ff'));
  assert.ok(text.includes('"Gone"=-'));
  assert.ok(text.includes("[-HKEY_CURRENT_USER\\Software\\Old]"));
  const values = parseRegFile("\uFEFF" + text, "HKCU\\Software\\X");
  assert.deepEqual(values, { InstallLocation: "C:\\Игры\\HLSR", Quote: 'a "b"' });
  assert.throws(() => toRegFile([{ key: "HKLM\\Software\\X", values: {} }]), /Неподдерживаемый/);

  // reg.exe export пишет UTF-16LE с BOM; reg import получает такой же файл.
  const dir = tempDir(t);
  let imported = null;
  const registry = createRegistry({
    tempDir: dir,
    run(args) {
      if (args[0] === "export") {
        fs.writeFileSync(args[2], "\uFEFF" + text, "utf16le");
        return { status: 0 };
      }
      imported = fs.readFileSync(args[1], "utf16le");
      return { status: 0 };
    },
  });
  assert.equal(registry.read("HKCU\\Software\\X").InstallLocation, "C:\\Игры\\HLSR");
  registry.apply([{ key: "HKCU\\Software\\X", values: { A: "Б" } }]);
  assert.ok(imported.startsWith("\uFEFFWindows Registry Editor"));
  assert.deepEqual(fs.readdirSync(dir), [], "временные .reg-файлы удалены");
  const missing = createRegistry({ tempDir: dir, run: () => ({ status: 1 }) });
  assert.equal(missing.read("HKCU\\Software\\Nope"), null);
});

test("file-list: список сборки и его проверки", (t) => {
  const dir = tempDir(t);
  const list = source(dir, "1.2.3", { "HLSR.exe": "exe", "resources/app.asar": "asar" });
  assert.deepEqual(list.files.map((item) => item.path), ["HLSR.exe", "resources/app.asar"]);
  assert.deepEqual(list.app, layout.PRODUCTION);
  assert.deepEqual(fileList.read(dir), list);
  const broken = (patch) => fileList.problemsOf({ ...list, ...patch });
  assert.match(broken({ files: list.files.slice(1) }).join(), /нет HLSR\.exe/);
  assert.match(broken({ files: [...list.files, { ...list.files[0], path: "../x" }] }).join(), /недопустимый путь/);
  assert.match(broken({ files: [...list.files, { ...list.files[0], path: "hlsr.EXE" }] }).join(), /повтор/);
  assert.match(broken({ app: null }).join(), /appId/);
  assert.match(broken({ version: "1.2" }).join(), /версия/);
});

test("install: чистая установка — файлы, маркер, реестр, ярлыки, прогресс", async (t) => {
  const f = fixture(t);
  const progress = [];
  const result = await runInstall(f, { desktopShortcut: true, setupFile: "C:\\Downloads\\HLSR-Setup-0.3.17-ru.exe",
    onProgress: (done, total) => progress.push([done, total]) });

  assert.equal(result.version, "0.3.17");
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "exe 0.3.17");
  assert.equal(fs.readFileSync(path.join(f.installDir, "locales/ru.pak"), "utf8"), "ru");
  assert.ok(fs.existsSync(path.join(f.installDir, fileList.RELATIVE)), "список файлов едет в установку — по нему удаление узнаёт identity");
  assert.equal(fs.readFileSync(path.join(f.installDir, layout.OWNER_MARKER), "utf8"), PRODUCTION.ownerMarker(result.installId));
  assert.match(result.installId, layout.INSTALL_ID_RE);

  const settings = f.registry.keys[PRODUCTION.installKey];
  assert.equal(settings.InstallLocation, path.resolve(f.installDir));
  assert.equal(settings.HLSRInstallId, result.installId);
  assert.equal(settings[layout.DESKTOP_VALUE], "1");
  const entry = f.registry.keys[PRODUCTION.uninstallKey];
  assert.equal(entry.DisplayName, "HLSR 0.3.17");
  assert.equal(entry.DisplayVersion, "0.3.17");
  assert.equal(entry.UninstallString, `"${path.join(path.resolve(f.installDir), "HLSR.exe")}" --hlsr-uninstall`);
  assert.equal(f.registry.keys[layout.ORIGIN_KEY].InstallerOrigin, "ru");

  assert.deepEqual(f.shortcuts.written, [f.shortcuts.startMenu, f.shortcuts.desktop]);
  assert.ok(fs.existsSync(path.join(f.libraryRoot, ".hlsr", "library.json")));
  const total = progress[progress.length - 1][1];
  assert.deepEqual(progress[progress.length - 1], [total, total]);
  assert.deepEqual(fs.readdirSync(path.dirname(f.installDir)), ["Launcher"], "рядом не осталось временных каталогов");
});

test("install: обновление поверх — installId, выбор ярлыка и лишние файлы", async (t) => {
  const f = fixture(t, "0.3.18", { "HLSR.exe": "exe 0.3.18", "resources/app.asar": "asar 0.3.18" });
  // Установка прежней версии от NSIS: свои файлы, ярлык на рабочем столе убран.
  fs.mkdirSync(path.join(f.installDir, "locales"), { recursive: true });
  fs.writeFileSync(path.join(f.installDir, "HLSR.exe"), "old");
  fs.writeFileSync(path.join(f.installDir, "Uninstall HLSR.exe"), "nsis");
  fs.writeFileSync(path.join(f.installDir, "locales/ru.pak"), "old ru");
  const installId = "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}";
  f.registry.apply([{ key: PRODUCTION.installKey, values: { InstallLocation: f.installDir, HLSRInstallId: installId, [layout.DESKTOP_VALUE]: "0" } }]);
  f.shortcuts.write(f.shortcuts.startMenu, "custom");
  f.shortcuts.written.length = 0;

  const result = await runInstall(f, { update: true });
  assert.equal(result.installId, installId);
  // У NSIS нет списка файлов: неизвестное содержимое не удаляется.
  assert.deepEqual(fs.readdirSync(f.installDir).sort(), [layout.OWNER_MARKER, "HLSR.exe", "locales", "resources"].sort());
  assert.equal(fs.existsSync(path.join(f.installDir, "Uninstall HLSR.exe")), false);
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "exe 0.3.18");
  assert.equal(f.registry.keys[PRODUCTION.installKey][layout.DESKTOP_VALUE], "0");
  assert.deepEqual(f.shortcuts.written, [], "существующий ярлык не тронут, удалённый с рабочего стола не возвращён");
  assert.equal(f.registry.keys[PRODUCTION.uninstallKey].DisplayVersion, "0.3.18");
});

test("install: библиотека и личные файлы внутри прежней папки переживают обновление NSIS", async (t) => {
  const f = fixture(t, "0.4.0");
  f.libraryRoot = path.join(f.installDir, "Game Library");
  const config = path.join(f.libraryRoot, "Games/ghosting-renewed/cfg/autoexec.cfg");
  const demo = path.join(f.libraryRoot, "Games/ghosting-renewed/demos/run.dem");
  fs.mkdirSync(path.dirname(config), { recursive: true });
  fs.mkdirSync(path.dirname(demo), { recursive: true });
  fs.writeFileSync(path.join(f.installDir, "HLSR.exe"), "old 0.3.16");
  fs.writeFileSync(config, "sensitivity 2.71\n");
  fs.writeFileSync(demo, Buffer.from([0, 1, 2, 255]));
  fs.writeFileSync(path.join(f.installDir, "personal.txt"), "keep");
  await runInstall(f, { update: true });
  assert.equal(fs.readFileSync(config, "utf8"), "sensitivity 2.71\n");
  assert.deepEqual(fs.readFileSync(demo), Buffer.from([0, 1, 2, 255]));
  assert.equal(fs.readFileSync(path.join(f.installDir, "personal.txt"), "utf8"), "keep");
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "exe 0.4.0");
  const installed = fileList.read(f.installDir);
  assert.ok(!installed.files.some((item) => item.path.includes("Game Library") || item.path === "personal.txt"));
});

test("install: следующая версия удаляет только известные файлы программы", async (t) => {
  const f = fixture(t, "0.4.0", { "HLSR.exe": "0.4.0", "resources/app.asar": "asar", "obsolete.dll": "old dll" });
  await runInstall(f);
  const personal = path.join(f.installDir, "resources/user-settings.json");
  fs.writeFileSync(personal, '{"value":42}');
  source(f.src, "0.4.1", { "HLSR.exe": "0.4.1", "resources/app.asar": "new" });
  // build() тестового источника перечисляет и прежний DLL — убираем его.
  fs.unlinkSync(path.join(f.src, "obsolete.dll"));
  source(f.src, "0.4.1", { "HLSR.exe": "0.4.1", "resources/app.asar": "new" });
  await runInstall(f, { update: true });
  assert.equal(fs.existsSync(path.join(f.installDir, "obsolete.dll")), false);
  assert.equal(fs.readFileSync(personal, "utf8"), '{"value":42}');
});

test("install: конфликт пользовательского каталога не повреждает прежнюю установку", async (t) => {
  const f = fixture(t, "0.4.0", { "HLSR.exe": "new", "resources/app.asar": "asar", "User Data": "new managed file" });
  const personal = path.join(f.installDir, "User Data/demo.dem");
  fs.mkdirSync(path.dirname(personal), { recursive: true });
  fs.writeFileSync(path.join(f.installDir, "HLSR.exe"), "old");
  fs.writeFileSync(personal, "keep demo");
  await assert.rejects(runInstall(f, { update: true }), (error) => error.code === "PRESERVE_FAILED");
  assert.equal(fs.readFileSync(personal, "utf8"), "keep demo");
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "old");
});

test("install: личная папка вместо устаревшего файла программы сохраняется", async (t) => {
  const f = fixture(t, "0.4.0", { "HLSR.exe": "0.4.0", "resources/app.asar": "asar", "obsolete.dll": "old dll" });
  await runInstall(f);
  fs.unlinkSync(path.join(f.installDir, "obsolete.dll"));
  const personal = path.join(f.installDir, "obsolete.dll/run.dem");
  fs.mkdirSync(path.dirname(personal));
  fs.writeFileSync(personal, "keep demo");
  fs.unlinkSync(path.join(f.src, "obsolete.dll"));
  source(f.src, "0.4.1", { "HLSR.exe": "0.4.1", "resources/app.asar": "new" });
  await runInstall(f, { update: true });
  assert.equal(fs.readFileSync(personal, "utf8"), "keep demo");
});

test("install: старая резервная папка с данными не удаляется", async (t) => {
  const f = fixture(t);
  const old = path.join(path.dirname(f.installDir), ".Launcher.hlsr-old-0123abcd");
  fs.mkdirSync(old, { recursive: true });
  fs.writeFileSync(path.join(old, "demo.dem"), "only copy");
  await assert.rejects(runInstall(f), (error) => error.code === "RECOVERY_REQUIRED");
  assert.equal(fs.readFileSync(path.join(old, "demo.dem"), "utf8"), "only copy");
  fs.mkdirSync(f.installDir);
  fs.writeFileSync(path.join(f.installDir, "HLSR.exe"), "working");
  await runInstall(f, { update: true });
  assert.equal(fs.readFileSync(path.join(old, "demo.dem"), "utf8"), "only copy");
});

test("install: папка пользователя на месте install-files.json не стирается", async (t) => {
  const f = fixture(t);
  const personal = path.join(f.installDir, fileList.RELATIVE, "run.dem");
  fs.mkdirSync(path.dirname(personal), { recursive: true });
  fs.writeFileSync(path.join(f.installDir, "HLSR.exe"), "old");
  fs.writeFileSync(personal, "keep");
  await assert.rejects(runInstall(f, { update: true }), error => error.code === "PRESERVE_FAILED");
  assert.equal(fs.readFileSync(personal, "utf8"), "keep");
});

test("install: повреждённый файл установщика не трогает рабочую установку", async (t) => {
  const f = fixture(t);
  await runInstall(f);
  const before = fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8");
  source(f.src, "0.3.18", { "HLSR.exe": "exe 0.3.18", "resources/app.asar": "asar 0.3.18" });
  fs.writeFileSync(path.join(f.src, "resources/app.asar"), "tampered!!!!");
  await assert.rejects(runInstall(f, { update: true }), (error) => error.code === "CORRUPT");
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), before);
  assert.equal(f.registry.keys[PRODUCTION.uninstallKey].DisplayVersion, "0.3.17");
  assert.deepEqual(fs.readdirSync(path.dirname(f.installDir)), ["Launcher"]);
});

test("install: файл оверлея, который унёс антивирус, не останавливает установку", async (t) => {
  const f = fixture(t, "0.3.17", {
    "HLSR.exe": "exe", "resources/app.asar": "asar",
    "resources/livesplit-overlay/proxy-d3d9.dll": "d3d9",
    "resources/livesplit-overlay/proxy-opengl32.dll": "opengl",
  });
  fs.rmSync(path.join(f.src, "resources/livesplit-overlay/proxy-d3d9.dll"));
  const progress = [];
  const warned = [];
  const result = await runInstall(f, {
    onProgress: (done, total) => progress.push([done, total]),
    log: { ...quiet, warn: (line) => warned.push(line) },
  });
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "exe");
  assert.ok(!fs.existsSync(path.join(f.installDir, "resources/livesplit-overlay/proxy-d3d9.dll")));
  assert.equal(fs.readFileSync(path.join(f.installDir, "resources/livesplit-overlay/proxy-opengl32.dll"), "utf8"), "opengl");
  assert.match(result.warnings.join(" "), /Оверлей LiveSplit/);
  assert.match(result.warnings.join(" "), /proxy-d3d9\.dll/);
  assert.ok(warned.some((line) => /proxy-d3d9\.dll/.test(line)), "причина остаётся в журнале");
  const [done, total] = progress[progress.length - 1];
  assert.equal(done, total, "полоса доходит до конца");
});

test("install: обязательный файл, который унёс антивирус, не трогает рабочую установку", async (t) => {
  const f = fixture(t);
  await runInstall(f);
  const before = fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8");
  source(f.src, "0.3.18", { "HLSR.exe": "exe 0.3.18", "resources/app.asar": "asar 0.3.18" });
  fs.rmSync(path.join(f.src, "resources/app.asar"));
  await assert.rejects(runInstall(f, { update: true }), (error) => {
    // Файл из списка пропал после распаковки — его унёс антивирус.
    assert.equal(error.code, "ANTIVIRUS");
    assert.equal(error.file, "resources/app.asar");
    return true;
  });
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), before);
  assert.deepEqual(fs.readdirSync(path.dirname(f.installDir)), ["Launcher"]);
});

test("install: повреждённый файл оверлея — всё равно ошибка, это не антивирус", async (t) => {
  const f = fixture(t, "0.3.17", { "HLSR.exe": "exe", "resources/app.asar": "asar", "resources/livesplit-overlay/proxy-d3d9.dll": "d3d9" });
  fs.writeFileSync(path.join(f.src, "resources/livesplit-overlay/proxy-d3d9.dll"), "tampered!!!!");
  await assert.rejects(runInstall(f), (error) => error.code === "CORRUPT");
});

// Поток чтения, который падает при открытии, как у Windows, когда файл держит
// или удалил антивирус.
function failingOpen(t, pattern, code, message) {
  const original = fsCompat.createReadStream;
  t.mock.method(fsCompat, "createReadStream", (file, ...rest) => (
    pattern.test(String(file))
      ? new Readable({ read() { this.destroy(Object.assign(new Error(`${message}, open '${file}'`), { code, syscall: "open" })); } })
      : original.call(fsCompat, file, ...rest)
  ));
}

for (const [code, message] of [
  ["UNKNOWN", "UNKNOWN: unknown error"],
  ["EPERM", "EPERM: operation not permitted"],
  ["EACCES", "EACCES: permission denied"],
  ["EBUSY", "EBUSY: resource busy or locked"],
]) {
  test(`install: ${code} при открытии файла — ошибка антивируса без сырого текста`, async (t) => {
    const f = fixture(t);
    failingOpen(t, /app\.asar$/, code, message);
    const warned = [];
    await assert.rejects(runInstall(f, { log: { ...quiet, warn: (line) => warned.push(line) } }), (error) => {
      assert.equal(error.code, "ANTIVIRUS");
      assert.equal(error.file, "resources/app.asar");
      assert.match(error.message, /^Антивирус заблокировал или удалил файл resources\/app\.asar/);
      assert.doesNotMatch(error.message, /unknown error|operation not permitted|open '/);
      return true;
    });
    assert.ok(warned.some((line) => line.includes(message)), "сырой текст ошибки остаётся в журнале");
    assert.equal(fs.existsSync(f.installDir), false);
  });
}

test("install: «virus» в тексте ошибки — тоже антивирус", async (t) => {
  const f = fixture(t);
  failingOpen(t, /HLSR\.exe$/, "EINVAL", "Operation did not complete successfully because the file contains a virus");
  await assert.rejects(runInstall(f), (error) => error.code === "ANTIVIRUS" && error.file === "HLSR.exe");
});

test("install: файл оверлея, который держит антивирус, не останавливает установку", async (t) => {
  const f = fixture(t, "0.3.17", { "HLSR.exe": "exe", "resources/app.asar": "asar", "resources/livesplit-overlay/proxy-d3d9.dll": "d3d9" });
  failingOpen(t, /proxy-d3d9\.dll$/, "UNKNOWN", "UNKNOWN: unknown error");
  const result = await runInstall(f);
  assert.match(result.warnings.join(" "), /proxy-d3d9\.dll/);
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), "exe");
});

test("install: нет места на диске — обычная ошибка копирования, не антивирус", async (t) => {
  const f = fixture(t);
  const original = fsCompat.createWriteStream;
  t.mock.method(fsCompat, "createWriteStream", (file, ...rest) => {
    if (!/app\.asar$/.test(String(file))) return original.call(fsCompat, file, ...rest);
    const stream = original.call(fsCompat, file, ...rest);
    stream._write = (_chunk, _encoding, callback) => callback(Object.assign(new Error("ENOSPC: no space left on device, write"), { code: "ENOSPC" }));
    return stream;
  });
  await assert.rejects(runInstall(f), (error) => {
    assert.equal(error.code, "COPY_FAILED");
    assert.match(error.message, /ENOSPC/);
    assert.doesNotMatch(error.message, /нтивирус/);
    return true;
  });
});

test("install: чужой каталог, отмена и занятые файлы", async (t) => {
  const f = fixture(t);
  fs.mkdirSync(f.installDir, { recursive: true });
  fs.writeFileSync(path.join(f.installDir, "notes.txt"), "mine");
  assert.equal(existingKind(f.installDir), "foreign");
  await assert.rejects(runInstall(f), (error) => error.code === "BAD_DIR");
  assert.equal(fs.readFileSync(path.join(f.installDir, "notes.txt"), "utf8"), "mine");

  const g = fixture(t);
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(runInstall(g, { signal: controller.signal }), (error) => error.code === "ABORTED");
  assert.equal(fs.existsSync(g.installDir), false);

  await assert.rejects(runInstall({ ...g, installDir: path.join(g.src, "sub") }), (error) => error.code === "BAD_DIR");
});

test("install: каталог в корне диска (D:\\hlsr) — родитель не создаётся заново", async (t) => {
  const f = fixture(t);
  const parent = path.dirname(f.installDir);
  fs.mkdirSync(parent, { recursive: true });
  // Как Windows для корня диска: mkdirSync("D:\\", { recursive: true }) → EPERM.
  const mkdirSync = fs.mkdirSync;
  t.mock.method(fs, "mkdirSync", function (dir, options) {
    if (path.resolve(String(dir)) === path.resolve(parent)) {
      const error = new Error(`EPERM: operation not permitted, mkdir '${dir}'`);
      error.code = "EPERM";
      throw error;
    }
    return mkdirSync.call(this, dir, options);
  });
  await runInstall(f);
  assert.ok(fs.existsSync(path.join(f.installDir, fileList.RELATIVE)));
});

test("install: остатки прерванной установки убираются", async (t) => {
  const f = fixture(t);
  const parent = path.dirname(f.installDir);
  fs.mkdirSync(path.join(parent, ".Launcher.hlsr-new-0123abcd"), { recursive: true });
  fs.mkdirSync(path.join(parent, ".Other.hlsr-new-0123abcd"), { recursive: true });
  await runInstall(f);
  assert.deepEqual(fs.readdirSync(parent).sort(), [".Other.hlsr-new-0123abcd", "Launcher"]);
});

test("install: отмена после последнего файла сохраняет предыдущую установку", async (t) => {
  const f = fixture(t);
  await runInstall(f);
  const before = fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8");
  source(f.src, "0.3.18", { "HLSR.exe": "new", "resources/app.asar": "new asar" });
  const controller = new AbortController();
  let committed = false;
  await assert.rejects(runInstall(f, {
    signal: controller.signal,
    onProgress: (done, total) => { if (done === total) controller.abort(); },
    onCommit: () => { committed = true; },
  }), error => error.code === "ABORTED");
  assert.equal(committed, false);
  assert.equal(fs.readFileSync(path.join(f.installDir, "HLSR.exe"), "utf8"), before);
  assert.equal(f.registry.read(PRODUCTION.uninstallKey).DisplayVersion, "0.3.17");
  assert.deepEqual(fs.readdirSync(path.dirname(f.installDir)), ["Launcher"]);
});

test("originOf: метка страницы загрузки в имени установщика", () => {
  assert.equal(originOf("C:\\D\\HLSR-Setup-0.3.17-ru.exe"), "ru");
  assert.equal(originOf("HLSR-Setup-0.3.17-EN.exe"), "en");
  assert.equal(originOf("HLSR-Setup-0.3.17.exe"), null);
  assert.equal(originOf(null), null);
});

test("uninstall: реестр, свои ярлыки, игры по выбору, каталог после проверки владения", async (t) => {
  const f = fixture(t);
  await runInstall(f, { desktopShortcut: true });
  // Чужой ярлык с тем же именем на рабочем столе.
  fs.writeFileSync(f.shortcuts.desktop, "C:\\Other\\HLSR.exe");
  const calls = [];
  const cleanup = { CODES: { REFUSED_RUNNING: 13, PARTIAL: 14 }, readMarker: () => null,
    cleanupLibrary: (root, options) => (calls.push([root, options]), { ok: true }) };
  const plan = uninstaller.plan({ installDir: f.installDir, libraryRoot: f.libraryRoot });
  assert.equal(plan.libraryExists, true);
  assert.ok(plan.installBytes > 0);

  const result = uninstaller.uninstall({ installDir: path.resolve(f.installDir), libraryRoot: f.libraryRoot, removeGames: false,
    registry: f.registry, shortcuts: f.shortcuts, cleanup, identity: PRODUCTION, env: {}, log: quiet });
  assert.equal(result.status, "ok");
  assert.equal(result.removeProgram, path.resolve(f.installDir));
  assert.deepEqual(calls, [], "игры без галочки не трогаются");
  assert.equal(f.registry.keys[PRODUCTION.installKey], undefined);
  assert.equal(f.registry.keys[PRODUCTION.uninstallKey], undefined);
  assert.equal(fs.existsSync(f.shortcuts.startMenu), false);
  assert.equal(fs.existsSync(f.shortcuts.desktop), true, "чужой ярлык остался");

  const g = fixture(t);
  await runInstall(g);
  const removed = uninstaller.uninstall({ installDir: path.resolve(g.installDir), libraryRoot: g.libraryRoot, removeGames: true,
    registry: g.registry, shortcuts: g.shortcuts, cleanup, identity: PRODUCTION, env: {}, log: quiet });
  assert.equal(removed.gamesRemoved, true);
  assert.equal(calls[0][0], g.libraryRoot);
  assert.equal(calls[0][1].purge, true);
});

test("uninstall: без совпадения маркера владения каталог не удаляется", async (t) => {
  const f = fixture(t);
  const { installId } = await runInstall(f);
  const dir = path.resolve(f.installDir);
  const own = { InstallLocation: dir };
  assert.equal(uninstaller.ownershipProblem(dir, { ...own, HLSRInstallId: installId }, {}, PRODUCTION), null);
  assert.match(uninstaller.ownershipProblem(dir, own, {}, PRODUCTION), /installId/);
  assert.match(uninstaller.ownershipProblem(dir, { ...own, HLSRInstallId: "{00000000-0000-0000-0000-000000000000}" }, {}, PRODUCTION), /не совпадает/);
  const qa = layout.identity({ appId: "com.hlsrteam.hlsr.qa", productName: "HLSR-QA" });
  assert.match(uninstaller.ownershipProblem(dir, { ...own, HLSRInstallId: installId }, {}, qa), /не совпадает/, "маркер другой сборки");
  // Реестр указывает на другой каталог: это копия (проба o).
  assert.match(uninstaller.ownershipProblem(dir, { InstallLocation: path.join(f.dir, "elsewhere"), HLSRInstallId: installId }, {}, PRODUCTION), /копия/);
  assert.match(uninstaller.ownershipProblem(dir, { HLSRInstallId: installId }, {}, PRODUCTION), /копия/, "каталог в реестре не указан");

  f.registry.apply([{ key: PRODUCTION.installKey, values: { HLSRInstallId: "{00000000-0000-0000-0000-000000000000}" } }]);
  const result = uninstaller.uninstall({ installDir: dir, libraryRoot: null, removeGames: false, registry: f.registry,
    shortcuts: f.shortcuts, cleanup: {}, identity: PRODUCTION, env: {}, log: quiet });
  assert.equal(result.removeProgram, null);
  assert.equal(result.status, "partial");
  assert.match(result.warnings.join(), /Каталог программы не удалён/);
});

test("uninstall: сохранение игр запрещает рекурсивное удаление вложенной библиотеки", async (t) => {
  const f = fixture(t);
  f.libraryRoot = path.join(f.installDir, "Game Library");
  await runInstall(f);
  const demo = path.join(f.libraryRoot, "run.dem");
  fs.writeFileSync(demo, "keep");
  const result = uninstaller.uninstall({ installDir: f.installDir, libraryRoot: f.libraryRoot, removeGames: false,
    registry: f.registry, shortcuts: f.shortcuts, identity: PRODUCTION, env: {}, log: quiet, cleanup: {} });
  assert.equal(result.removeProgram, null);
  assert.equal(result.gamesRemoved, false);
  assert.match(result.warnings.join(" "), /вложенную библиотеку/);
  assert.equal(fs.readFileSync(demo, "utf8"), "keep");
});

test("removalCommand: PowerShell ждёт выхода без консольных окон, путь не попадает в текст команды", () => {
  const dir = 'C:\\a" & del C:\\x %TEMP%';
  const command = uninstaller.removalCommand(dir, { SystemRoot: "C:\\Windows" });
  assert.equal(command.env.HLSR_REMOVE_DIR, dir);
  assert.ok(!command.args.join(" ").includes("del C:"));
  const script = command.args[command.args.length - 1];
  assert.match(script, /Start-Sleep/);
  // Удаление без перехода по ссылкам: в junction и symlink скрипт не заходит,
  // каталог с маркером другой установки не трогает.
  assert.match(script, /if \(\$i\.PSIsContainer -and -not \$link\) \{ \$m = Join-Path \$p '\.hlsr-install-owner\.json'/);
  // Нечитаемый маркер — чужой: исключение чтения тоже ведёт к return.
  assert.match(script, /try \{ if \(\[IO\.File\]::ReadAllText\(\$m\)\.TrimEnd\(\) -ne \$env:HLSR_OWN_MARKER\) \{ return \} \} catch \{ return \} \}; Get-ChildItem -LiteralPath \$p -Force \| ForEach-Object \{ Remove-NoFollow/);
  assert.doesNotMatch(script, /-Recurse/);
  // Удалённый путь из списка уходит: появившийся заново (новая установка) цикл не трогает.
  assert.match(script, /\$all = @\(\$all \| Where-Object \{ Test-Path -LiteralPath \$_ \}\)/);
  // Никаких внешних программ для паузы: из процесса без консоли они
  // открывались бы в отдельных окнах.
  assert.doesNotMatch(script, /ping|timeout|cmd/i);
  assert.throws(() => uninstaller.removalCommand("relative\\dir"), /Недопустимый/);
  assert.throws(() => uninstaller.removalCommand("C:\\a\nb"), /Недопустимый/);
  assert.throws(() => uninstaller.removalCommand(null), /Недопустимый/);
  // Данные — списком через «|», которого в путях Windows не бывает.
  const data = uninstaller.removalCommand(null, { SystemRoot: "C:\\Windows" }, null,
    { data: ["C:\\U\\AppData\\Roaming\\hlsr", "C:\\U\\AppData\\Local\\hlsr"], parent: "C:\\HLSR", pid: 42 });
  assert.equal(data.env.HLSR_REMOVE_DIR, "");
  assert.equal(data.env.HLSR_REMOVE_DATA, "C:\\U\\AppData\\Roaming\\hlsr|C:\\U\\AppData\\Local\\hlsr");
  assert.equal(data.env.HLSR_REMOVE_PARENT, "C:\\HLSR");
  assert.equal(data.env.HLSR_WAIT_PID, "42");
  assert.equal(data.env.HLSR_OWN_MARKER, "");
  assert.equal(uninstaller.removalCommand("C:\\HLSR\\Launcher", {}, null, { ownMarker: "{\"a\":1}" }).env.HLSR_OWN_MARKER, "{\"a\":1}");
  assert.throws(() => uninstaller.removalCommand(null, {}, null, { data: ["C:\\a|C:\\b"] }), /Недопустимый/);
  assert.throws(() => uninstaller.removalCommand(null, {}, null, { data: ["relative"] }), /Недопустимый/);
  let spawned = null;
  uninstaller.scheduleRemoval("C:\\HLSR\\Launcher", { env: { SystemRoot: "C:\\Windows" }, spawnSync: (...args) => (spawned = args, { status: 0 }) });
  const powershell = path.join("C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  assert.equal(spawned[0], powershell);
  assert.equal(spawned[2].windowsHide, true);
  // С DETACHED_PROCESS PowerShell выходит, не выполнив -Command: скрипт
  // запускает Start-Process из обычного дочернего PowerShell.
  assert.equal(spawned[2].detached, undefined);
  assert.equal(spawned[2].env.HLSR_REMOVE_DIR, "C:\\HLSR\\Launcher");
  assert.equal(spawned[2].env.HLSR_POWERSHELL, powershell);
  assert.equal(spawned[2].env.HLSR_RELAUNCH, undefined);
  const launch = spawned[1][spawned[1].length - 1];
  assert.match(launch, /^Start-Process -FilePath \$env:HLSR_POWERSHELL -WindowStyle Hidden /);
  assert.ok(!launch.includes("HLSR\\Launcher"), "путь только в окружении");
  const encoded = /'-EncodedCommand','([A-Za-z0-9+/=]+)'$/.exec(launch)[1];
  assert.equal(Buffer.from(encoded, "base64").toString("utf16le"), script);
  assert.throws(() => uninstaller.scheduleRemoval("C:\\HLSR\\Launcher", { env: { SystemRoot: "C:\\Windows" }, spawnSync: () => ({ status: 1 }) }), /не запущен/);
});

// Настоящий PowerShell: скрипт удаления переживает выход того, кто его
// запустил (на Windows; на Linux PowerShell нет).
test("scheduleRemoval: каталог удаляется и после выхода запустившего процесса", { skip: process.platform !== "win32" }, async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-remove-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.mkdirSync(path.join(dir, "resources"));
  fs.writeFileSync(path.join(dir, "resources", "app.asar"), "x");
  const child = require("child_process").spawnSync(process.execPath, ["-e",
    `require(${JSON.stringify(require.resolve("../src/main/windows-install/uninstall"))}).scheduleRemoval(${JSON.stringify(dir)})`],
  { stdio: "ignore", timeout: 60000 });
  assert.equal(child.status, 0);
  for (let i = 0; i < 40 && fs.existsSync(dir); i++) await new Promise((resolve) => setTimeout(resolve, 250));
  assert.equal(fs.existsSync(dir), false);
});

test("удаление из установщика: «Назад» открывает установщик только после удаления каталога", () => {
  const args = layout.parseArgs(["HLSR.exe", layout.UNINSTALL_FLAG, layout.RETURN_SETUP_FLAG, "C:\\Users\\a b\\HLSR-Setup.exe"]);
  assert.equal(args.uninstall, true);
  assert.equal(args.returnSetup, "C:\\Users\\a b\\HLSR-Setup.exe");
  assert.equal(layout.parseArgs(["HLSR.exe", layout.UNINSTALL_FLAG]).returnSetup, null);
  const command = uninstaller.removalCommand("C:\\HLSR\\Launcher", { SystemRoot: "C:\\Windows" }, "C:\\x\\HLSR-Setup.exe");
  assert.equal(command.env.HLSR_RELAUNCH, "C:\\x\\HLSR-Setup.exe");
  const script = command.args[command.args.length - 1];
  // Запуск — после цикла удаления и только если каталога уже нет.
  assert.ok(script.indexOf("Start-Process") > script.lastIndexOf("Remove-NoFollow $_"));
  assert.match(script, /-not \(\$d -and \(Test-Path -LiteralPath \$d\)\)\) \{ Start-Process -FilePath \$env:HLSR_RELAUNCH/);
  assert.ok(!script.includes("HLSR-Setup"));
  assert.equal(setup.returnSetupOf("relative\\HLSR-Setup.exe"), null);
  assert.equal(setup.returnSetupOf("C:\\nope\\HLSR-Setup.txt"), null);
  assert.equal(setup.returnSetupOf(null), null);
});

test("build-windows-setup: QA-сборка получает свои appId, имя и каталог вывода", () => {
  const options = buildSetup.parseArgs(["-c.extraMetadata.version=9.9.9", "-c.appId=com.hlsrteam.hlsr.qa",
    "-c.productName=HLSR-QA", "-c.directories.output=build-qa/9.9.9", "--dir"]);
  assert.equal(options.dirOnly, true);
  assert.equal(options.version, "9.9.9");
  assert.deepEqual(options.app, { appId: "com.hlsrteam.hlsr.qa", productName: "HLSR-QA" });
  assert.equal(options.setupName, "HLSR-Setup-9.9.9.exe");
  assert.ok(options.unpacked.endsWith(path.join("build-qa", "9.9.9", "win-unpacked")));
  assert.deepEqual(buildSetup.parseArgs([]).app, layout.PRODUCTION);
  assert.throws(() => buildSetup.parseArgs(["--publish"]), /Неизвестные/);
  assert.match(buildSetup.appUpdateYml(), /^owner: HL-HLSR\nrepo: HLSR-Releases\nprovider: github\nreleaseType: release\nupdaterCacheDirName: hlsr-updater\n$/);
  assert.equal(crypto.createHash("sha256").update(buildSetup.appUpdateYml()).digest("hex").length, 64);
  // Кеш electron-updater QA-сборки свой: боевой hlsr-updater она не делит.
  assert.equal(buildSetup.parseArgs(["-c.extraMetadata.name=hlsr-qa"]).name, "hlsr-qa");
  assert.equal(buildSetup.parseArgs([]).name, "hlsr");
  assert.match(buildSetup.appUpdateYml("hlsr-qa"), /\nupdaterCacheDirName: hlsr-qa-updater\n$/);
});

test("uninstall: QA-сборка стирает только свой кеш обновлений, не hlsr-updater", async (t) => {
  const f = fixture(t);
  await runInstall(f);
  const local = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-local-"));
  t.after(() => fs.rmSync(local, { recursive: true, force: true }));
  for (const name of ["hlsr-updater", "hlsr-qa-updater"]) {
    fs.mkdirSync(path.join(local, name));
    fs.writeFileSync(path.join(local, name, "installer.exe"), "x");
  }
  uninstaller.uninstall({ installDir: path.resolve(f.installDir), libraryRoot: null, removeGames: false,
    registry: f.registry, shortcuts: f.shortcuts, cleanup: {}, identity: PRODUCTION, env: { LOCALAPPDATA: local },
    log: quiet, updaterCacheName: "hlsr-qa-updater" });
  assert.equal(fs.existsSync(path.join(local, "hlsr-qa-updater")), false);
  assert.equal(fs.existsSync(path.join(local, "hlsr-updater", "installer.exe")), true, "кеш боевого HLSR остался");
});

// Дерево каталога без перехода по ссылкам: «a/», «a/b.txt».
function listTree(root, relative = "") {
  const out = [];
  for (const name of fs.readdirSync(path.join(root, relative)).sort()) {
    const child = relative ? `${relative}/${name}` : name;
    const stat = fs.lstatSync(path.join(root, child));
    if (stat.isDirectory() && !stat.isSymbolicLink()) out.push(`${child}/`, ...listTree(root, child));
    else out.push(child);
  }
  return out;
}

// Настоящий скрипт удаления (PowerShell) из отдельного процесса: он ждёт
// выхода этого процесса, как ждёт выхода HLSR.exe.
async function runRemoval(program, options, until) {
  const child = require("child_process").spawnSync(process.execPath, ["-e",
    `require(${JSON.stringify(require.resolve("../src/main/windows-install/uninstall"))}).scheduleRemoval(${JSON.stringify(program)}, ${JSON.stringify(options)})`],
  { stdio: "ignore", timeout: 60000 });
  assert.equal(child.status, 0);
  for (let i = 0; i < 80 && !until(); i++) await new Promise((resolve) => setTimeout(resolve, 250));
}

// Песочница пользователя Windows: свои %APPDATA%, %LOCALAPPDATA%, %TEMP% и
// профиль, реестр и ярлыки подставные. Настоящие install() (установка и
// обновление) и uninstall(); следы первого запуска лежат там, куда их пишет
// лаунчер (userData, журналы, протокол Discord, кеш и копия апдейтера).
async function sandboxLifecycle(t, removeGames) {
  const root = tempDir(t);
  const env = {
    APPDATA: path.join(root, "AppData", "Roaming"), LOCALAPPDATA: path.join(root, "AppData", "Local"),
    TEMP: path.join(root, "Temp"), USERPROFILE: path.join(root, "home"),
  };
  for (const dir of Object.values(env)) fs.mkdirSync(dir, { recursive: true });
  const src = path.join(root, "setup-unpacked");
  source(src, "0.4.8", { "HLSR.exe": "exe 0.4.8", "resources/app.asar": "asar 0.4.8", "locales/ru.pak": "ru" });
  const installDir = path.join(root, "HLSR", "Launcher");
  const libraryRoot = path.join(root, "HLSR");
  const registry = memoryRegistry();
  const shortcuts = fakeShortcuts(env.USERPROFILE);
  // Папки меню «Пуск» и рабочего стола есть у пользователя всегда.
  for (const file of [shortcuts.startMenu, shortcuts.desktop]) fs.mkdirSync(path.dirname(file), { recursive: true });
  const skeleton = listTree(root);
  const base = { sourceDir: src, installDir, registry, shortcuts, libraryRoot, env, log: quiet, sleep: async () => {} };

  await install({ ...base, desktopShortcut: true, setupFile: "C:\\Downloads\\HLSR-Setup-0.4.8-ru.exe" });
  // Первый запуск: настройки, вход, журналы, игры, протокол Discord.
  const userData = path.join(env.APPDATA, "hlsr");
  const logs = path.join(env.LOCALAPPDATA, "hlsr");
  for (const [file, body] of [
    [path.join(userData, "library.json"), JSON.stringify({ libraryPath: libraryRoot })],
    [path.join(userData, "game-channel-freshness.json"), "{}"],
    [path.join(userData, "player-config-backups.json"), "{}"],
    [path.join(userData, "online-session.v2.json"), "{}"],
    [path.join(userData, "Local Storage", "leveldb", "000003.log"), "x"],
    [path.join(logs, "logs", "main.log"), "log"],
    [path.join(libraryRoot, "Games", "ghosting-renewed", "SAVE", "quick.sav"), "save"],
    [path.join(libraryRoot, "Games", "ghosting-renewed", "demos", "run.dem"), "demo"],
  ]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, body);
  }
  const protocolKey = `HKCU\\Software\\Classes\\${uninstaller.DISCORD_PROTOCOL}`;
  registry.apply([{ key: protocolKey, values: { "URL Protocol": "" } }]);
  // Обновление: кеш electron-updater, копия HLSRUpdater.exe в TEMP, его журнал
  // и профиль Chromium, затем настоящая установка новой версии.
  const updaterLaunch = require("../src/main/launcher-update/updater-launch");
  const staged = updaterLaunch.stage({ appDir: installDir, resourcesPath: path.join(installDir, "resources"), tempRoot: env.TEMP });
  updaterLaunch.writePlan(staged.planFile, { targetVersion: "0.4.9" });
  for (const file of [path.join(env.LOCALAPPDATA, "hlsr-updater", "pending", "HLSR-Setup-0.4.9.exe"),
    path.join(env.LOCALAPPDATA, "HLSR", "logs", "updater.log"), path.join(env.APPDATA, "HLSR Updater", "Local State"),
    path.join(userData, "version-state.json")]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "x");
  }
  source(src, "0.4.9", { "HLSR.exe": "exe 0.4.9", "resources/app.asar": "asar 0.4.9", "locales/ru.pak": "ru" });
  await install({ ...base, update: true });
  const created = listTree(root).filter((item) => !skeleton.includes(item));

  const protocols = [];
  const result = uninstaller.uninstall({
    installDir, libraryRoot, removeGames, registry, shortcuts, cleanup: require("../src/main/uninstall/library-cleanup"),
    identity: PRODUCTION, env, log: quiet, steamLibraries: [], tempRoot: env.TEMP, dataDirs: [userData, logs],
    removeProtocolClient: (name) => (protocols.push(name), registry.apply([{ key: `HKCU\\Software\\Classes\\${name}`, remove: true }]), true),
  });
  return { root, env, installDir, libraryRoot, userData, logs, registry, shortcuts, skeleton, created, result, protocols, staged };
}

test("песочница: полное удаление не оставляет следов установки, первого запуска и обновления", async (t) => {
  const s = await sandboxLifecycle(t, true);
  t.diagnostic(`создано: ${s.created.join(", ")}`);
  assert.equal(s.result.status, "ok", s.result.warnings.join("; "));
  assert.deepEqual(Object.keys(s.registry.keys), [], "Uninstall, ключ установки, InstallerOrigin и протокол Discord удалены");
  assert.deepEqual(s.protocols, [uninstaller.DISCORD_PROTOCOL]);
  assert.equal(fs.existsSync(s.shortcuts.startMenu) || fs.existsSync(s.shortcuts.desktop), false);
  assert.equal(fs.existsSync(s.staged.dir), false, "копия HLSRUpdater.exe в TEMP");
  assert.equal(fs.existsSync(path.join(s.env.LOCALAPPDATA, "hlsr-updater")), false);
  assert.deepEqual(fs.readdirSync(s.libraryRoot), ["Launcher"], "библиотека очищена, остался только каталог программы");
  assert.equal(s.result.removeProgram, s.installDir);
  assert.equal(s.result.removeParent, s.libraryRoot);
  assert.deepEqual(s.result.removeData, [s.userData, s.logs, path.join(s.env.APPDATA, "HLSR Updater")]);
  if (process.platform !== "win32") return;
  await runRemoval(s.result.removeProgram, { data: s.result.removeData, parent: s.result.removeParent, ownMarker: s.result.ownMarker },
    () => !fs.existsSync(s.libraryRoot) && s.result.removeData.every((dir) => !fs.existsSync(dir)));
  const left = listTree(s.root).filter((item) => !s.skeleton.includes(item));
  t.diagnostic(`осталось: ${left.join(", ") || "ничего"}`);
  assert.deepEqual(left, []);
});

test("песочница: удаление без игр оставляет только игры и данные для переустановки", async (t) => {
  const s = await sandboxLifecycle(t, false);
  assert.equal(s.result.status, "ok", s.result.warnings.join("; "));
  assert.deepEqual(Object.keys(s.registry.keys), []);
  assert.deepEqual(s.result.removeData, [path.join(s.env.APPDATA, "HLSR Updater")], "данные пользователя не в списке");
  assert.ok(fs.existsSync(path.join(s.libraryRoot, "Games", "ghosting-renewed", "SAVE", "quick.sav")));
  if (process.platform !== "win32") return;
  await runRemoval(s.result.removeProgram, { data: s.result.removeData, parent: s.result.removeParent, ownMarker: s.result.ownMarker },
    () => !fs.existsSync(s.installDir) && s.result.removeData.every((dir) => !fs.existsSync(dir)));
  const left = listTree(s.root).filter((item) => !s.skeleton.includes(item));
  t.diagnostic(`осталось: ${left.join(", ")}`);
  assert.ok(left.every((item) => /^(HLSR\/(?!Launcher)|AppData\/Roaming\/hlsr\/|AppData\/Local\/(hlsr|HLSR)\/)/.test(item) ||
    ["HLSR/", "AppData/Roaming/hlsr/", "AppData/Local/hlsr/", "AppData/Local/HLSR/"].includes(item)), left.join(", "));
  assert.ok(left.includes("AppData/Roaming/hlsr/online-session.v2.json"), "вход сохранён");
  assert.ok(left.includes("HLSR/Games/ghosting-renewed/demos/run.dem"), "демки сохранены");
});

test("uninstall: данные только свои — в %APPDATA%/%LOCALAPPDATA%, с именем HLSR, без ссылок", (t) => {
  const root = tempDir(t);
  const env = { APPDATA: path.join(root, "Roaming"), LOCALAPPDATA: path.join(root, "Local") };
  const outside = path.join(root, "outside");
  for (const dir of [path.join(env.APPDATA, "hlsr"), path.join(env.LOCALAPPDATA, "HLSR"), path.join(env.APPDATA, "Other"), path.join(root, "hlsr"), outside])
    fs.mkdirSync(dir, { recursive: true });
  fs.symlinkSync(outside, path.join(env.APPDATA, "hlsr-qa"), "junction");
  assert.equal(uninstaller.ownDataDir(path.join(env.APPDATA, "hlsr"), env), true);
  assert.equal(uninstaller.ownDataDir(path.join(env.LOCALAPPDATA, "HLSR"), env), true);
  assert.equal(uninstaller.ownDataDir(path.join(env.APPDATA, "Other"), env), false, "чужая папка");
  assert.equal(uninstaller.ownDataDir(path.join(root, "hlsr"), env), false, "не в %APPDATA%");
  assert.equal(uninstaller.ownDataDir(env.APPDATA, env), false);
  assert.equal(uninstaller.ownDataDir(path.join(env.APPDATA, "hlsr-qa"), env), false, "ссылка наружу");
  assert.equal(uninstaller.ownDataDir(path.join(env.APPDATA, "hlsr-нет"), env), false, "нет каталога");
});

test("uninstall: полное удаление — вложенная библиотека уходит с программой, папка HLSR вокруг программы очищается", async (t) => {
  const cleanup = require("../src/main/uninstall/library-cleanup");
  // Библиотека прежних версий внутри каталога программы.
  const f = fixture(t);
  f.libraryRoot = path.join(f.installDir, "Game Library");
  await runInstall(f);
  fs.writeFileSync(path.join(f.libraryRoot, "run.dem"), "x");
  let running = [];
  const fake = { CODES: cleanup.CODES, readMarker: () => null, runningGames: () => running, cleanupLibrary: () => assert.fail("не вызывается") };
  const options = { installDir: path.resolve(f.installDir), libraryRoot: f.libraryRoot, removeGames: true, registry: f.registry,
    shortcuts: f.shortcuts, cleanup: fake, identity: PRODUCTION, env: {}, log: quiet };
  running = [{ gameId: "hl1", pid: 1 }];
  const kept = uninstaller.uninstall(options);
  assert.equal(kept.removeProgram, null, "игра запущена — каталог программы остаётся");
  assert.match(kept.warnings.join(" "), /игра ещё запущена/);
  running = [];
  f.registry = memoryRegistry();
  await runInstall(f, { update: true });
  const removed = uninstaller.uninstall({ ...options, registry: f.registry });
  assert.equal(removed.gamesRemoved, true);
  assert.equal(removed.removeProgram, path.resolve(f.installDir));

  // Игры переехали в другую библиотеку, а в папке HLSR осталась раскладка установщика.
  const g = fixture(t);
  const env = { USERPROFILE: path.join(g.dir, "home") };
  const hlsr = path.dirname(g.installDir);
  await runInstall(g, { libraryRoot: hlsr });
  cleanup.writeMarker(g.libraryRoot);
  fs.mkdirSync(path.join(g.libraryRoot, "Games"), { recursive: true });
  const result = uninstaller.uninstall({ installDir: path.resolve(g.installDir), libraryRoot: g.libraryRoot, removeGames: true,
    registry: g.registry, shortcuts: g.shortcuts, cleanup, identity: PRODUCTION, env, log: quiet, steamLibraries: [] });
  assert.equal(result.status, "ok", result.warnings.join("; "));
  assert.equal(fs.existsSync(g.libraryRoot), false, "библиотека удалена целиком");
  assert.deepEqual(fs.readdirSync(hlsr), ["Launcher"], "раскладка библиотеки в папке HLSR удалена");
  assert.equal(result.removeParent, hlsr);
});

test("scheduleRemoval: junction внутри каталогов не уводит удаление наружу, непустая папка остаётся", { skip: process.platform !== "win32" }, async (t) => {
  const root = tempDir(t);
  const outside = path.join(root, "outside");
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(outside, "keep.txt"), "keep");
  const parent = path.join(root, "HLSR");
  const program = path.join(parent, "Launcher");
  const data = path.join(root, "AppData", "hlsr");
  for (const dir of [program, data]) {
    fs.mkdirSync(path.join(dir, "sub"), { recursive: true });
    fs.writeFileSync(path.join(dir, "sub", "own.txt"), "x");
    fs.symlinkSync(outside, path.join(dir, "sub", "link"), "junction");
  }
  fs.writeFileSync(path.join(program, "readonly.txt"), "x");
  fs.chmodSync(path.join(program, "readonly.txt"), 0o444);
  await runRemoval(program, { data: [data], parent }, () => !fs.existsSync(parent) && !fs.existsSync(data));
  assert.equal(fs.existsSync(parent), false, "опустевшая папка удалена");
  assert.equal(fs.existsSync(data), false);
  assert.equal(fs.readFileSync(path.join(outside, "keep.txt"), "utf8"), "keep", "цель junction цела");

  const busy = path.join(root, "Busy");
  fs.mkdirSync(path.join(busy, "Launcher"), { recursive: true });
  fs.writeFileSync(path.join(busy, "notes.txt"), "mine");
  await runRemoval(path.join(busy, "Launcher"), { parent: busy }, () => !fs.existsSync(path.join(busy, "Launcher")));
  assert.deepEqual(fs.readdirSync(busy), ["notes.txt"], "непустая папка вокруг программы остаётся");
});

// Раунд 1 ревью.
const QA_APP = { appId: "com.hlsrteam.hlsr.qa", productName: "HLSR-QA" };
const QA = layout.identity(QA_APP);

// Боевой HLSR в C:\HLSR\Launcher и QA-сборка в C:\HLSR-QA\Launcher с общей
// библиотекой по умолчанию C:\HLSR (проба ревьюера probe-qa.js).
async function sharedLibrary(t) {
  const root = tempDir(t);
  const env = { APPDATA: path.join(root, "AppData", "Roaming"), LOCALAPPDATA: path.join(root, "AppData", "Local"),
    TEMP: path.join(root, "Temp"), USERPROFILE: path.join(root, "home") };
  for (const dir of Object.values(env)) fs.mkdirSync(dir, { recursive: true });
  const library = path.join(root, "HLSR");
  const registry = memoryRegistry();
  const base = { registry, libraryRoot: library, env, log: quiet, sleep: async () => {} };
  const prodSrc = path.join(root, "src-prod"), qaSrc = path.join(root, "src-qa");
  source(prodSrc, "0.4.9", { "HLSR.exe": "exe prod", "resources/app.asar": "asar" });
  source(qaSrc, "0.4.9", { "HLSR.exe": "exe qa", "resources/app.asar": "asar" }, QA_APP);
  const prodDir = path.join(library, "Launcher"), qaDir = path.join(root, "HLSR-QA", "Launcher");
  await install({ ...base, sourceDir: prodSrc, installDir: prodDir, shortcuts: fakeShortcuts(path.join(root, "prod")) });
  const save = path.join(library, "Games", "hl1-won", "valve", "SAVE", "run.sav");
  fs.mkdirSync(path.dirname(save), { recursive: true });
  fs.writeFileSync(save, "prod save");
  await install({ ...base, sourceDir: qaSrc, installDir: qaDir, shortcuts: fakeShortcuts(path.join(root, "qa"), QA) });
  return { root, env, library, registry, prodDir, qaDir, save };
}

test("раунд 1: удаление QA с галочкой не трогает боевой HLSR в общей библиотеке", async (t) => {
  const s = await sharedLibrary(t);
  assert.equal(uninstaller.plan({ installDir: s.qaDir, libraryRoot: s.library }).libraryExists, false, "окно не предлагает удалить чужую библиотеку");
  const result = uninstaller.uninstall({ installDir: s.qaDir, libraryRoot: s.library, removeGames: true, registry: s.registry,
    shortcuts: fakeShortcuts(path.join(s.root, "qa"), QA), cleanup: require("../src/main/uninstall/library-cleanup"),
    identity: QA, env: s.env, log: quiet, steamLibraries: [], dataDirs: [] });
  assert.equal(fs.readFileSync(path.join(s.prodDir, "HLSR.exe"), "utf8"), "exe prod");
  assert.equal(fs.readFileSync(s.save, "utf8"), "prod save");
  assert.ok(s.registry.keys[PRODUCTION.uninstallKey], "записи боевой копии на месте");
  assert.equal(s.registry.keys[QA.uninstallKey], undefined);
  assert.match(result.warnings.join(" "), /Игры не удалены: тестовая сборка HLSR-QA не удаляет библиотеку/);
  assert.equal(result.removeProgram, path.resolve(s.qaDir));
  // Боевой удаляет себя, а в библиотеке стоит QA: библиотека не трогается, окно говорит почему.
  const prodPlan = uninstaller.plan({ installDir: s.prodDir, libraryRoot: s.library, identity: PRODUCTION });
  assert.deepEqual([prodPlan.libraryExists, prodPlan.libraryBlocked], [true, null], "QA не в библиотеке");
  fs.mkdirSync(s.qaDir, { recursive: true });
  fs.renameSync(s.qaDir, path.join(s.library, "QA"));
  fs.writeFileSync(path.join(s.library, "QA", layout.OWNER_MARKER), QA.ownerMarker("{11111111-2222-3333-4444-555555555555}"));
  const blocked = uninstaller.plan({ installDir: s.prodDir, libraryRoot: s.library, identity: PRODUCTION });
  assert.deepEqual([blocked.libraryExists, blocked.libraryBlocked], [false, "other"]);
});

test("раунд 2: QA не чистит библиотеку боевого, даже если боевой стоит не в ней (проба a_prod_outside_library)", async (t) => {
  const root = tempDir(t);
  const env = { APPDATA: path.join(root, "AppData", "Roaming"), LOCALAPPDATA: path.join(root, "AppData", "Local"),
    TEMP: path.join(root, "Temp"), USERPROFILE: path.join(root, "home") };
  const library = path.join(root, "HLSR");
  const registry = memoryRegistry();
  const base = { registry, libraryRoot: library, env, log: quiet, sleep: async () => {} };
  const prodSrc = path.join(root, "src-prod"), qaSrc = path.join(root, "src-qa");
  source(prodSrc, "0.4.9", { "HLSR.exe": "exe prod", "resources/app.asar": "asar" });
  source(qaSrc, "0.4.9", { "HLSR.exe": "exe qa", "resources/app.asar": "asar" }, QA_APP);
  const prodDir = path.join(root, "Apps", "HLSR", "Launcher"), qaDir = path.join(root, "HLSR-QA", "Launcher");
  await install({ ...base, sourceDir: prodSrc, installDir: prodDir, shortcuts: fakeShortcuts(path.join(root, "prod")) });
  const save = path.join(library, "Games", "hl1-won", "valve", "SAVE", "prod.sav");
  fs.mkdirSync(path.dirname(save), { recursive: true });
  fs.writeFileSync(path.join(library, "Games", "hl1-won", "installed.json"), JSON.stringify({ gameId: "hl1-won" }));
  fs.writeFileSync(save, "prod save");
  await install({ ...base, sourceDir: qaSrc, installDir: qaDir, shortcuts: fakeShortcuts(path.join(root, "qa"), QA) });
  const plan = uninstaller.plan({ installDir: qaDir, libraryRoot: library, identity: QA });
  assert.deepEqual([plan.libraryExists, plan.libraryBlocked], [false, "test"], "переключателя нет, окно говорит почему");
  // Без явной identity сборка берётся из списка файлов каталога программы.
  assert.equal(uninstaller.plan({ installDir: qaDir, libraryRoot: library }).libraryExists, false);
  const result = uninstaller.uninstall({ installDir: qaDir, libraryRoot: library, removeGames: true, registry,
    shortcuts: fakeShortcuts(path.join(root, "qa"), QA), cleanup: require("../src/main/uninstall/library-cleanup"),
    identity: QA, env, log: quiet, steamLibraries: [] });
  assert.equal(fs.readFileSync(save, "utf8"), "prod save");
  assert.match(result.warnings.join(" "), /тестовая сборка HLSR-QA не удаляет библиотеку/);
});

test("раунд 1: каталог чужой установки внутри библиотеки не удаляется ни на какой глубине", async (t) => {
  const s = await sharedLibrary(t);
  const cleanup = require("../src/main/uninstall/library-cleanup");
  // QA-установка глубже, чем ищет uninstall (Tools\a\b\HLSR-QA): её бережёт library-cleanup.
  const deep = path.join(s.library, "Tools", "a", "b", "HLSR-QA");
  fs.mkdirSync(deep, { recursive: true });
  fs.copyFileSync(path.join(s.qaDir, layout.OWNER_MARKER), path.join(deep, layout.OWNER_MARKER));
  fs.writeFileSync(path.join(deep, "HLSR.exe"), "qa");
  fs.writeFileSync(path.join(s.library, "Tools", "a", "tool.exe"), "x");
  const result = cleanup.cleanupLibrary(s.library, { purge: true, installDir: s.prodDir, env: s.env, steamLibraries: [],
    ownMarker: fs.readFileSync(path.join(s.prodDir, layout.OWNER_MARKER), "utf8") });
  assert.equal(fs.readFileSync(path.join(deep, "HLSR.exe"), "utf8"), "qa");
  assert.equal(fs.existsSync(path.join(s.library, "Tools", "a", "tool.exe")), false, "своё рядом удалено");
  assert.equal(fs.existsSync(s.save), false);
  assert.ok(result.left.includes(path.join("Tools", "a", "b", "HLSR-QA")), result.left.join());
  assert.equal(result.ok, true);
});

// Проба h: QA без install-files.json или с повреждённым, боевой вне общей библиотеки.
for (const variant of ["missing", "corrupt"]) {
  test(`раунд 3: QA с ${variant === "missing" ? "удалённым" : "повреждённым"} списком файлов не считается боевой (проба h)`, async (t) => {
    const root = tempDir(t);
    const env = { APPDATA: path.join(root, "AppData", "Roaming"), LOCALAPPDATA: path.join(root, "AppData", "Local"),
      TEMP: path.join(root, "Temp"), USERPROFILE: path.join(root, "home") };
    for (const dir of [env.APPDATA, path.join(env.APPDATA, "HLSR Updater"), path.join(env.APPDATA, "hlsr-qa")]) fs.mkdirSync(dir, { recursive: true });
    const library = path.join(root, "HLSR");
    const keys = memoryRegistry();
    const base = { libraryRoot: library, env, log: quiet, sleep: async () => {} };
    const prodSrc = path.join(root, "src-prod"), qaSrc = path.join(root, "src-qa");
    source(prodSrc, "0.4.9", { "HLSR.exe": "exe prod", "resources/app.asar": "asar" });
    source(qaSrc, "0.4.9", { "HLSR.exe": "exe qa", "resources/app.asar": "asar" }, QA_APP);
    const prodDir = path.join(root, "Apps", "HLSR", "Launcher"), qaDir = path.join(root, "HLSR-QA", "Launcher");
    await install({ ...base, registry: keys, sourceDir: prodSrc, installDir: prodDir, shortcuts: fakeShortcuts(path.join(root, "prod")),
      setupFile: "C:\\D\\HLSR-Setup-0.4.9-ru.exe" });
    const save = path.join(library, "Games", "hl1-won", "valve", "SAVE", "prod.sav");
    fs.mkdirSync(path.dirname(save), { recursive: true });
    fs.writeFileSync(path.join(library, "Games", "hl1-won", "installed.json"), JSON.stringify({ gameId: "hl1-won" }));
    fs.writeFileSync(save, "prod save");
    await install({ ...base, registry: keys, sourceDir: qaSrc, installDir: qaDir, shortcuts: fakeShortcuts(path.join(root, "qa"), QA) });
    const list = path.join(qaDir, fileList.RELATIVE);
    if (variant === "missing") fs.rmSync(list); else fs.writeFileSync(list, "{");
    const snapshot = JSON.stringify(keys.keys);
    const run = (identity) => {
      const registry = memoryRegistry(JSON.parse(snapshot));
      const options = { installDir: qaDir, libraryRoot: library, removeGames: true, registry, shortcuts: fakeShortcuts(path.join(root, "qa"), identity),
        cleanup: require("../src/main/uninstall/library-cleanup"), identity, env, log: quiet, steamLibraries: [], tempRoot: env.TEMP,
        dataDirs: [path.join(env.APPDATA, "hlsr-qa")] };
      return { plan: uninstaller.plan({ ...options, registry }), result: uninstaller.uninstall(options), registry };
    };

    // Как setup.js: сборка — по GUID маркера (QA), не боевая.
    const id = uninstaller.identityOf(qaDir);
    assert.equal(id.guid, QA.guid);
    assert.notEqual(id.appId, layout.PRODUCTION.appId);
    const own = run(id);
    assert.deepEqual([own.plan.libraryExists, own.plan.libraryBlocked], [false, "test"]);
    assert.equal(fs.readFileSync(save, "utf8"), "prod save");
    assert.ok(own.registry.keys[PRODUCTION.uninstallKey] && own.registry.keys[PRODUCTION.installKey], "ключи боевого на месте");
    assert.equal(own.registry.keys[QA.uninstallKey], undefined, "свои ключи QA удалены");
    assert.deepEqual(own.registry.keys[layout.ORIGIN_KEY], { InstallerOrigin: "ru" }, "уборка только для боевой не делается");
    assert.ok(!own.result.removeData.includes(path.join(env.APPDATA, "HLSR Updater")));

    // Если сборку всё же сочли боевой (как раньше setup.js): маркер не совпал с реестром — ничего чужого.
    const wrong = run(PRODUCTION);
    assert.deepEqual([wrong.plan.libraryExists, wrong.plan.libraryBlocked], [false, "unverified"]);
    assert.equal(fs.readFileSync(save, "utf8"), "prod save");
    assert.ok(wrong.registry.keys[PRODUCTION.uninstallKey] && wrong.registry.keys[PRODUCTION.installKey], "ключи боевого на месте");
    assert.deepEqual(wrong.registry.keys[layout.ORIGIN_KEY], { InstallerOrigin: "ru" });
    assert.deepEqual(wrong.result.removeData, [], "ни данных, ни профиля апдейтера");
    assert.match(wrong.result.warnings.join(" "), /Игры и данные не удалены: установку не удалось проверить/);
    assert.match(wrong.result.warnings.join(" "), /Запись реестра другой установки .* не тронута/);
  });
}

// Раунд 4. Мир: боевой HLSR в своей папке, библиотека задаётся сценарием.
async function prodWorld(t, libraryRoot) {
  const f = fixture(t);
  f.libraryRoot = libraryRoot(f.dir);
  await runInstall(f);
  const remove = (extra = {}) => {
    const options = { installDir: path.resolve(f.installDir), libraryRoot: f.libraryRoot, removeGames: true, registry: f.registry,
      shortcuts: f.shortcuts, cleanup: require("../src/main/uninstall/library-cleanup"), identity: uninstaller.identityOf(f.installDir),
      env: { USERPROFILE: path.join(f.dir, "home") }, log: quiet, steamLibraries: [], ...extra };
    return { plan: uninstaller.plan(options), result: uninstaller.uninstall(options) };
  };
  return { f, remove };
}

function putFiles(root, files) {
  for (const relative of files) {
    fs.mkdirSync(path.dirname(path.join(root, relative)), { recursive: true });
    fs.writeFileSync(path.join(root, relative), relative.endsWith("installed.json") ? JSON.stringify({ gameId: path.basename(path.dirname(relative)) }) : "x");
  }
}

test("раунд 4: установщик не ставит «создано установщиком» в существующую папку человека (проба q)", async (t) => {
  const mine = [path.join("Demos", "my-own-runs", "hl2-sub15.dem"), path.join("Tools", "my-tool", "t.exe"), path.join("Downloads", "some.zip")];
  const { f, remove } = await prodWorld(t, (dir) => {
    const lib = path.join(dir, "D", "HLSR");
    putFiles(lib, mine); // папка человека с его файлами, маркера ещё нет
    return lib;
  });
  const marker = require("../src/main/uninstall/library-cleanup").readMarker(f.libraryRoot);
  assert.ok(marker, "маркер библиотеки есть");
  assert.equal(marker.createdBy, undefined, "без отметки «создано установщиком»");
  const { result } = remove();
  for (const relative of mine) assert.ok(fs.existsSync(path.join(f.libraryRoot, relative)), relative);
  assert.match(result.warnings.join(" "), /Оставлено в .*Demos.*Tools/);
  // Папку, которой не было, установщик создаёт сам — отметка ставится.
  const fresh = await prodWorld(t, (dir) => path.join(dir, "E", "HLSR"));
  assert.equal(require("../src/main/uninstall/library-cleanup").readMarker(fresh.f.libraryRoot).createdBy, "installer");
});

test("раунд 4: папку HLSR, созданную лаунчером, полное удаление уносит целиком (проба r)", async (t) => {
  const { resolveLibrary, ensureLayout } = require("../src/main/game-delivery/paths");
  const cleanup = require("../src/main/uninstall/library-cleanup");
  const { f, remove } = await prodWorld(t, (dir) => path.join(dir, "C-lib"));
  const lib = path.join(f.dir, "D", "HLSR"); // подпапка HLSR из диалога, её ещё нет
  ensureLayout(resolveLibrary(lib));
  assert.equal(cleanup.readMarker(lib).createdBy, "launcher");
  putFiles(lib, [path.join("Games", "hl2-4260", "installed.json"), path.join("Games", "hl1-won", "partial.bin"),
    path.join("Tools", "LiveSplit", "LiveSplit.exe"), path.join("Downloads", "hl2-4260.7z.part"), path.join("Backups", "cfg.zip"), path.join("Demos", "run.zip")]);
  const { result } = remove({ libraryRoot: lib });
  assert.equal(fs.existsSync(lib), false, "своя папка HLSR удалена целиком");
  assert.doesNotMatch(result.warnings.join(" "), /Оставлено/);
  // В существующую папку лаунчер отметку не ставит.
  const existing = path.join(f.dir, "Mine", "HLSR");
  fs.mkdirSync(existing, { recursive: true });
  ensureLayout(resolveLibrary(existing));
  assert.equal(cleanup.readMarker(existing).createdBy, undefined);
});

test("раунд 5: папка, созданная выбором библиотеки в лаунчере, получает отметку и уходит целиком (проба r, настоящий путь)", async (t) => {
  const deliveryPaths = require("../src/main/game-delivery/paths");
  const cleanup = require("../src/main/uninstall/library-cleanup");
  // AppManager: validateInstallPath создаёт папку через prepareLibraryRoot, set-library-path сохраняет выбор и зовёт markCreatedLibrary.
  const appManager = fs.readFileSync(path.join(__dirname, "../src/main/AppManager.js"), "utf8");
  const validate = appManager.slice(appManager.indexOf("validateInstallPath(rawPath) {"), appManager.indexOf("freeSpaceMb:", appManager.indexOf("validateInstallPath(rawPath) {")));
  assert.match(validate, /prepareLibraryRoot\(target\)/);
  assert.doesNotMatch(validate, /fs\.mkdirSync\(target/);
  const setPath = appManager.slice(appManager.indexOf('ipcMain.handle("set-library-path"'), appManager.indexOf('ipcMain.handle("open-logs-folder"'));
  assert.match(setPath, /LibraryStore\.set\("libraryPath", check\.path\);[\s\S]*markCreatedLibrary\(check\.path\)/);

  const { f, remove } = await prodWorld(t, (dir) => path.join(dir, "C-lib"));
  const lib = path.join(f.dir, "D", "HLSR"); // подпапка HLSR из диалога, её ещё нет
  deliveryPaths.prepareLibraryRoot(lib); // validate-install-path
  deliveryPaths.prepareLibraryRoot(lib); // set-library-path проверяет ещё раз: папка уже есть
  assert.equal(deliveryPaths.markCreatedLibrary(lib), true);
  assert.equal(cleanup.readMarker(lib).createdBy, "launcher", "отметка сразу при сохранении выбора");
  deliveryPaths.ensureLayout(deliveryPaths.resolveLibrary(lib)); // первая установка игры
  assert.equal(cleanup.readMarker(lib).createdBy, "launcher");
  putFiles(lib, [path.join("Games", "hl1-won", "partial.bin"), path.join("Tools", "LiveSplit", "LiveSplit.exe"),
    path.join("Downloads", "x.7z.part"), path.join("Backups", "cfg.zip"), path.join("Demos", "run.zip")]);
  remove({ libraryRoot: lib });
  assert.equal(fs.existsSync(lib), false, "своя папка HLSR удалена целиком");

  // Существующая папка человека: отметки нет.
  const mine = path.join(f.dir, "Mine", "HLSR");
  fs.mkdirSync(mine, { recursive: true });
  deliveryPaths.prepareLibraryRoot(mine);
  assert.equal(deliveryPaths.markCreatedLibrary(mine), false);
  deliveryPaths.ensureLayout(deliveryPaths.resolveLibrary(mine));
  assert.equal(cleanup.readMarker(mine).createdBy, undefined);
});

// Короткое имя 8.3 каталога (Scripting.FileSystemObject), если они включены на томе.
function shortPath(dir) {
  const result = require("child_process").spawnSync(path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    ["-NoProfile", "-NonInteractive", "-Command", "(New-Object -ComObject Scripting.FileSystemObject).GetFolder($env:HLSR_TEST_DIR).ShortPath"],
    { encoding: "utf8", windowsHide: true, env: { ...process.env, HLSR_TEST_DIR: dir } });
  return String(result.stdout || "").trim();
}

test("раунд 5: короткое имя 8.3 в InstallLocation или в пути удаления — та же установка", { skip: process.platform !== "win32" }, async (t) => {
  for (const variant of ["registry", "installDir"]) {
    const { f } = await prodWorld(t, (dir) => path.join(dir, "Program Files Test", "HLSR"));
    // Каталог программы под папкой с пробелами — у неё есть короткое имя.
    const long = path.join(f.dir, "Program Files Test", "HLSR", "Launcher");
    fs.cpSync(f.installDir, long, { recursive: true });
    f.registry.apply([{ key: PRODUCTION.installKey, values: { InstallLocation: long } }, { key: PRODUCTION.uninstallKey, values: { InstallLocation: long } }]);
    const short = shortPath(long);
    if (!short || short.toLowerCase() === long.toLowerCase()) return t.skip("короткие имена 8.3 на этом томе выключены");
    if (variant === "registry") f.registry.apply([{ key: PRODUCTION.installKey, values: { InstallLocation: short } }]);
    const installDir = variant === "installDir" ? short : long;
    const options = { installDir, libraryRoot: f.libraryRoot, removeGames: true, registry: f.registry, shortcuts: f.shortcuts,
      cleanup: require("../src/main/uninstall/library-cleanup"), identity: uninstaller.identityOf(installDir),
      env: { USERPROFILE: path.join(f.dir, "home") }, log: quiet, steamLibraries: [] };
    const plan = uninstaller.plan({ ...options, registry: f.registry });
    assert.deepEqual([plan.libraryExists, plan.libraryBlocked], [true, null], `${variant}: своя установка`);
    const result = uninstaller.uninstall(options);
    assert.equal(result.removeProgram, installDir, variant);
    assert.deepEqual(Object.keys(f.registry.keys), [], `${variant}: ключи удалены`);
    assert.doesNotMatch(result.warnings.join(" "), /копия|другой установки|Оставлено/, variant);
  }
});

test("раунд 4: удаление из копии каталога не трогает настоящую установку (проба o)", async (t) => {
  const { f } = await prodWorld(t, (dir) => path.join(dir, "HLSR"));
  const lib = f.libraryRoot;
  putFiles(lib, [path.join("Games", "hl1-won", "installed.json"), path.join("Games", "hl1-won", "SAVE", "a.sav")]);
  const copy = path.join(f.dir, "Backup", "Launcher");
  fs.cpSync(f.installDir, copy, { recursive: true });
  const options = { installDir: copy, libraryRoot: lib, removeGames: true, registry: f.registry, shortcuts: f.shortcuts,
    cleanup: require("../src/main/uninstall/library-cleanup"), identity: uninstaller.identityOf(copy), env: {}, log: quiet, steamLibraries: [] };
  const plan = uninstaller.plan({ ...options, registry: f.registry });
  assert.deepEqual([plan.libraryExists, plan.libraryBlocked], [false, "unverified"]);
  const result = uninstaller.uninstall(options);
  assert.ok(f.registry.keys[PRODUCTION.uninstallKey] && f.registry.keys[PRODUCTION.installKey], "ключи настоящей установки на месте");
  assert.ok(fs.existsSync(path.join(lib, "Games", "hl1-won", "SAVE", "a.sav")), "игры на месте");
  assert.equal(result.removeProgram, null);
  assert.match(result.warnings.join(" "), /это копия, а не установка/);
});

test("раунд 4: QA с маркером и списком боевого не проходит проверку владения (проба n)", async (t) => {
  const s = await sharedLibrary(t);
  for (const file of [fileList.RELATIVE, layout.OWNER_MARKER]) fs.copyFileSync(path.join(s.prodDir, file), path.join(s.qaDir, file));
  const id = uninstaller.identityOf(s.qaDir);
  const options = { installDir: s.qaDir, libraryRoot: s.library, removeGames: true, registry: s.registry,
    shortcuts: fakeShortcuts(path.join(s.root, "qa"), id), cleanup: require("../src/main/uninstall/library-cleanup"), identity: id,
    env: s.env, log: quiet, steamLibraries: [] };
  assert.equal(uninstaller.plan({ ...options, registry: s.registry }).libraryExists, false);
  uninstaller.uninstall(options);
  assert.equal(fs.readFileSync(s.save, "utf8"), "prod save");
  assert.ok(s.registry.keys[PRODUCTION.uninstallKey] && s.registry.keys[PRODUCTION.installKey], "ключи боевого на месте");
  assert.equal(fs.readFileSync(path.join(s.prodDir, "HLSR.exe"), "utf8"), "exe prod");
});

test("раунд 4: маркер библиотеки за ссылкой (.hlsr — junction) не считается (проба k2)", async (t) => {
  const cleanup = require("../src/main/uninstall/library-cleanup");
  const { f, remove } = await prodWorld(t, (dir) => path.join(dir, "C-lib"));
  const lib = path.join(f.dir, "D", "HLSR");
  putFiles(lib, [path.join("Demos", "mine.dem")]);
  const stamp = path.join(f.dir, "stamp");
  cleanup.writeMarker(stamp, { createdBy: "installer" });
  fs.symlinkSync(path.join(stamp, ".hlsr"), path.join(lib, ".hlsr"), "junction");
  assert.equal(cleanup.readMarker(lib), null);
  assert.throws(() => cleanup.writeMarker(lib), /ссылка/, "через ссылку маркер не пишется");
  remove({ libraryRoot: lib });
  assert.ok(fs.existsSync(path.join(lib, "Demos", "mine.dem")));
  assert.ok(fs.existsSync(path.join(stamp, ".hlsr", "library.json")));
});

// Одинаковое содержимое в папке HLSR и во вписанной руками библиотеке (D:\Games).
const LIBRARY_CONTENT = {
  [path.join("Games", "hl2-4260", "installed.json")]: JSON.stringify({ gameId: "hl2-4260" }),
  [path.join("Games", "hl2-4260", "hl2", "save", "own.sav")]: "own",
  [path.join("Games", "hl1-won", "my-own-hl1", "readme.txt")]: "x", // известный id без installed.json
  [path.join("Downloads", "Firefox Setup.exe")]: "x", [path.join("Tools", "7-Zip", "7z.exe")]: "x",
  [path.join("Demos", "csgo", "match.dem")]: "x", [path.join("Backups", "photos", "2024.zip")]: "x",
  [path.join("hl2-2707", "installed.json")]: JSON.stringify({ gameId: "hl2-2707" }), // старая раскладка
  [path.join("Games", "MyMod", "mod.txt")]: "x", [path.join("Witcher3", "bin", "witcher3.exe")]: "x", "notes.txt": "x",
};

async function libraryRemoval(t, name, launcherMarker = false) {
  const f = fixture(t);
  f.libraryRoot = path.join(f.dir, "Disk", name);
  if (launcherMarker) require("../src/main/uninstall/library-cleanup").writeMarker(f.libraryRoot); // папку завёл человек, маркер — лаунчер
  await runInstall(f);
  for (const [relative, body] of Object.entries(LIBRARY_CONTENT)) {
    fs.mkdirSync(path.dirname(path.join(f.libraryRoot, relative)), { recursive: true });
    fs.writeFileSync(path.join(f.libraryRoot, relative), body);
  }
  const options = { installDir: path.resolve(f.installDir), libraryRoot: f.libraryRoot, removeGames: true, registry: f.registry,
    shortcuts: f.shortcuts, cleanup: require("../src/main/uninstall/library-cleanup"), identity: PRODUCTION,
    env: { USERPROFILE: path.join(f.dir, "home") }, log: quiet, steamLibraries: [] };
  const plan = uninstaller.plan(options);
  const result = uninstaller.uninstall(options);
  const alive = Object.keys(LIBRARY_CONTENT).filter((relative) => fs.existsSync(path.join(f.libraryRoot, relative)));
  return { f, plan, result, alive };
}

test("раунд 2: во вписанной руками библиотеке уходят только игры с installed.json и .hlsr (проба b_typed_library_foreign_names)", async (t) => {
  const { f, result, alive } = await libraryRemoval(t, "Games");
  assert.deepEqual(alive.sort(), Object.keys(LIBRARY_CONTENT).filter((relative) => !relative.startsWith(path.join("Games", "hl2-4260"))).sort());
  assert.equal(fs.existsSync(path.join(f.libraryRoot, ".hlsr")), false);
  const warning = result.warnings.join(" ");
  for (const name of ["Downloads", "Tools", "Demos", "Backups", path.join("Games", "hl1-won")])
    assert.ok(warning.includes(name), `${name} назван в предупреждении: ${warning}`);
  assert.match(warning, /Оставлено в .* \(9\)/);
  assert.doesNotMatch(warning, /Чужие/, "свои оставшиеся файлы чужими не называются");
});

test("раунд 2: в папке HLSR свои папки уходят целиком, чужое остаётся", async (t) => {
  const { f, alive } = await libraryRemoval(t, "HLSR");
  assert.deepEqual(alive.sort(), ["notes.txt", path.join("Games", "MyMod", "mod.txt"), path.join("Witcher3", "bin", "witcher3.exe")].sort());
  assert.deepEqual(fs.readdirSync(f.libraryRoot).sort(), ["Games", "Witcher3", "notes.txt"]);
});

test("раунд 3: папка HLSR без маркера установщика (завёл человек, любой регистр) — как чужая библиотека", async (t) => {
  for (const name of ["HLSR", "hlsr", "HlSr"]) {
    const { alive } = await libraryRemoval(t, name, true);
    assert.deepEqual(alive.sort(), Object.keys(LIBRARY_CONTENT).filter((relative) => !relative.startsWith(path.join("Games", "hl2-4260"))).sort(), name);
  }
});

test("раунд 2: размер в окне — только то, что удалится (проба g_moved_library_plan)", async (t) => {
  const typed = await libraryRemoval(t, "Games");
  const { treeSize } = require("../src/main/uninstall/launcher-uninstall");
  // План снят до удаления: свои — Games\hl2-4260 и .hlsr; 100 КБ чужого не в счёт.
  assert.ok(typed.plan.libraryBytes > 0);
  assert.ok(typed.plan.libraryBytes < 1024, `${typed.plan.libraryBytes} байт`);
  const f = fixture(t);
  f.libraryRoot = path.join(f.dir, "Disk", "Games");
  await runInstall(f);
  fs.mkdirSync(path.join(f.libraryRoot, "Witcher3"), { recursive: true });
  fs.writeFileSync(path.join(f.libraryRoot, "Witcher3", "w.exe"), Buffer.alloc(100000));
  const plan = uninstaller.plan({ installDir: f.installDir, libraryRoot: f.libraryRoot });
  assert.equal(plan.libraryBytes, treeSize(path.join(f.libraryRoot, ".hlsr")));
});

// Маркер чужой установки открыт другим процессом без общего доступа: не читается.
function lockFile(t, file, seconds) {
  const holder = require("child_process").spawn(path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    ["-NoProfile", "-NonInteractive", "-Command", `$f = [IO.File]::Open($env:HLSR_TEST_FILE, 'Open', 'ReadWrite', 'None'); Start-Sleep -Seconds ${seconds}; $f.Close()`],
    { stdio: "ignore", windowsHide: true, env: { ...process.env, HLSR_TEST_FILE: file } });
  const exited = new Promise((resolve) => holder.once("exit", resolve));
  t.after(() => holder.kill());
  return {
    async ready() {
      for (let i = 0; i < 50; i++) {
        try { fs.closeSync(fs.openSync(file, "r")); } catch (_) { return; }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.fail("файл не заняли");
    },
    async release() { holder.kill(); await exited; },
  };
}

test("раунд 2: нечитаемый маркер чужой установки — чужой, в Node (проба d_locked_foreign_marker_node)", { skip: process.platform !== "win32" }, async (t) => {
  const s = await sharedLibrary(t);
  const cleanup = require("../src/main/uninstall/library-cleanup");
  const foreign = path.join(s.library, "Tools", "a", "b", "c", "QA-copy");
  fs.mkdirSync(path.join(foreign, "resources"), { recursive: true });
  fs.writeFileSync(path.join(foreign, layout.OWNER_MARKER), QA.ownerMarker("{11111111-2222-3333-4444-555555555555}"));
  fs.writeFileSync(path.join(foreign, "resources", "app.asar"), "qa asar");
  const lock = lockFile(t, path.join(foreign, layout.OWNER_MARKER), 10);
  await lock.ready();
  const result = cleanup.cleanupLibrary(s.library, { purge: true, installDir: s.prodDir, env: s.env, steamLibraries: [],
    ownMarker: fs.readFileSync(path.join(s.prodDir, layout.OWNER_MARKER), "utf8") });
  assert.equal(fs.readFileSync(path.join(foreign, "resources", "app.asar"), "utf8"), "qa asar");
  assert.ok(result.left.includes(path.relative(s.library, foreign)), result.left.join());
  await lock.release();
});

test("раунд 2: нечитаемый маркер чужой установки — чужой, в скрипте PowerShell (проба probe-ps-marker)", { skip: process.platform !== "win32" }, async (t) => {
  const root = tempDir(t);
  const program = path.join(root, "HLSR", "Launcher");
  const own = '{"schemaVersion":1,"appId":"own","installId":"{A}"}';
  for (const name of ["free-foreign", "locked-foreign"]) {
    fs.mkdirSync(path.join(program, "sub", name), { recursive: true });
    fs.writeFileSync(path.join(program, "sub", name, layout.OWNER_MARKER), '{"schemaVersion":1,"appId":"other","installId":"{B}"}');
    fs.writeFileSync(path.join(program, "sub", name, "data.txt"), name);
  }
  fs.writeFileSync(path.join(program, layout.OWNER_MARKER), own);
  fs.writeFileSync(path.join(program, "HLSR.exe"), "exe");
  const lock = lockFile(t, path.join(program, "sub", "locked-foreign", layout.OWNER_MARKER), 12);
  await lock.ready();
  await runRemoval(program, { ownMarker: own }, () => !fs.existsSync(path.join(program, "HLSR.exe")));
  await new Promise((resolve) => setTimeout(resolve, 1500));
  assert.equal(fs.existsSync(path.join(program, "HLSR.exe")), false, "своё удалено");
  assert.equal(fs.readFileSync(path.join(program, "sub", "free-foreign", "data.txt"), "utf8"), "free-foreign");
  assert.equal(fs.readFileSync(path.join(program, "sub", "locked-foreign", "data.txt"), "utf8"), "locked-foreign");
  await lock.release();
});

test("раунд 1: старая библиотека в %APPDATA%\\hlsr\\library уходит с данными, без «Игры не удалены»", async (t) => {
  const f = fixture(t);
  const env = { APPDATA: path.join(f.dir, "Roaming"), USERPROFILE: path.join(f.dir, "home") };
  const userData = path.join(env.APPDATA, "hlsr");
  f.libraryRoot = path.join(userData, "library");
  await runInstall(f, { env });
  const options = { installDir: path.resolve(f.installDir), libraryRoot: f.libraryRoot, removeGames: true, registry: f.registry,
    shortcuts: f.shortcuts, cleanup: require("../src/main/uninstall/library-cleanup"), identity: PRODUCTION, env, log: quiet,
    steamLibraries: [], dataDirs: [userData] };
  assert.equal(uninstaller.plan(options).libraryExists, true);
  const keys = JSON.parse(JSON.stringify(f.registry.keys));
  const result = uninstaller.uninstall(options);
  assert.doesNotMatch(result.warnings.join(" "), /Игры не удалены/);
  assert.equal(result.gamesRemoved, true);
  assert.ok(result.removeData.includes(userData));
  // Игра запущена: данные вместе с библиотекой остаются.
  const state = path.join(f.libraryRoot, ".hlsr", "state");
  fs.mkdirSync(state, { recursive: true });
  fs.writeFileSync(path.join(state, "hl1-won.process.json"), JSON.stringify({ gameId: "hl1-won", pid: process.pid, state: "running" }));
  const running = uninstaller.uninstall({ ...options, registry: memoryRegistry(keys) });
  assert.match(running.warnings.join(" "), /игра ещё запущена/);
  assert.equal(running.removeData.includes(userData), false);
});

test("раунд 1: окно показывает все очищаемые папки, размер без каталога программы", async (t) => {
  const f = fixture(t);
  const hlsr = path.dirname(f.installDir);
  await runInstall(f, { libraryRoot: hlsr }); // раскладка библиотеки в папке HLSR
  require("../src/main/uninstall/library-cleanup").writeMarker(f.libraryRoot);
  fs.writeFileSync(path.join(f.libraryRoot, "game.bin"), Buffer.alloc(2048));
  const plan = uninstaller.plan({ installDir: f.installDir, libraryRoot: f.libraryRoot });
  assert.deepEqual(plan.libraryRoots, [f.libraryRoot, hlsr]);
  const { treeSize } = require("../src/main/uninstall/launcher-uninstall");
  // Свои — только раскладка (.hlsr); чужой game.bin и каталог программы не в счёт.
  assert.equal(plan.libraryBytes, treeSize(path.join(f.libraryRoot, ".hlsr")) + treeSize(path.join(hlsr, ".hlsr")), "размер без каталога программы");
});

test("раунд 1: QA и боевой не трогают следы друг друга (InstallerOrigin, копии апдейтера, HLSR Updater)", async (t) => {
  const root = tempDir(t);
  const env = { APPDATA: path.join(root, "Roaming"), LOCALAPPDATA: path.join(root, "Local") };
  const temp = path.join(root, "Temp");
  fs.mkdirSync(path.join(env.APPDATA, "HLSR Updater"), { recursive: true });
  const stage = (name, app) => {
    const dir = path.join(temp, name);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "HLSRUpdater.exe"), "exe");
    if (app) fs.writeFileSync(path.join(dir, layout.OWNER_MARKER), layout.identity(app).ownerMarker("{00000000-0000-0000-0000-000000000000}"));
    return dir;
  };
  const prodStage = stage("HLSR-Updater-aaa-000000000001", layout.PRODUCTION);
  const qaStage = stage("HLSR-Updater-aaa-000000000002", QA_APP);
  const unknownStage = stage("HLSR-Updater-aaa-000000000003", null);
  // Настоящие установки обеих сборок: уборка за сборкой — только после проверки владения.
  const installed = async (app, identity, origin) => {
    const src = path.join(root, `src-${identity.guid}`);
    source(src, "0.4.9", { "HLSR.exe": "exe", "resources/app.asar": "asar" }, app);
    const registry = memoryRegistry({ [layout.ORIGIN_KEY]: origin });
    const installDir = path.join(root, identity.productName, "Launcher");
    await install({ sourceDir: src, installDir, registry, shortcuts: fakeShortcuts(root, identity), libraryRoot: path.join(root, "lib"),
      env, log: quiet, sleep: async () => {} });
    return { registry, run: () => uninstaller.uninstall({ installDir, libraryRoot: null, removeGames: false, registry,
      shortcuts: fakeShortcuts(root, identity), cleanup: {}, identity, env, log: quiet, tempRoot: temp }) };
  };

  const shared = await installed(QA_APP, QA, { InstallerOrigin: "ru" });
  const qa = shared.run();
  assert.deepEqual(shared.registry.keys[layout.ORIGIN_KEY], { InstallerOrigin: "ru" }, "QA не трогает общий InstallerOrigin");
  assert.deepEqual(qa.removeData, [], "QA не трогает профиль апдейтера боевой сборки");
  assert.deepEqual([fs.existsSync(prodStage), fs.existsSync(qaStage), fs.existsSync(unknownStage)], [true, false, true]);

  const other = await installed(layout.PRODUCTION, PRODUCTION, { InstallerOrigin: "ru", Other: "x" });
  const prod = other.run();
  assert.deepEqual(other.registry.keys[layout.ORIGIN_KEY], { Other: "x" }, "чужое значение в ключе остаётся");
  assert.deepEqual(prod.removeData, [path.join(env.APPDATA, "HLSR Updater")]);
  assert.equal(fs.existsSync(prodStage), false);
  assert.equal(fs.existsSync(unknownStage), true, "копия без маркера — чья, неизвестно");
});

test("раунд 1: работающая копия апдейтера в TEMP не трогается", { skip: process.platform !== "win32" }, async (t) => {
  const root = tempDir(t);
  const dir = path.join(root, "HLSR-Updater-aaa-000000000001");
  fs.mkdirSync(dir);
  fs.writeFileSync(path.join(dir, layout.OWNER_MARKER), PRODUCTION.ownerMarker("{00000000-0000-0000-0000-000000000000}"));
  const exe = path.join(dir, "HLSRUpdater.exe");
  fs.writeFileSync(exe, "exe");
  // Как запущенный exe: файл открыт без общего доступа.
  const holder = require("child_process").spawn(path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    ["-NoProfile", "-NonInteractive", "-Command", "$f = [IO.File]::Open($env:HLSR_TEST_FILE, 'Open', 'ReadWrite', 'None'); Start-Sleep -Seconds 15; $f.Close()"],
    { stdio: "ignore", windowsHide: true, env: { ...process.env, HLSR_TEST_FILE: exe } });
  t.after(() => holder.kill());
  for (let i = 0; i < 40; i++) {
    try { fs.closeSync(fs.openSync(exe, "r+")); } catch (_) { break; }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  // Своя настоящая установка: уборка копий идёт, только занятая остаётся.
  const f = fixture(t);
  await runInstall(f);
  const free = path.join(root, "HLSR-Updater-aaa-000000000002");
  fs.mkdirSync(free);
  fs.copyFileSync(path.join(dir, layout.OWNER_MARKER), path.join(free, layout.OWNER_MARKER));
  uninstaller.uninstall({ installDir: path.resolve(f.installDir), libraryRoot: null, removeGames: false, registry: f.registry,
    shortcuts: f.shortcuts, cleanup: {}, identity: PRODUCTION, env: {}, log: quiet, tempRoot: root });
  assert.equal(fs.readFileSync(path.join(dir, layout.OWNER_MARKER), "utf8").length > 0, true, "копия цела");
  assert.equal(fs.existsSync(free), false, "незанятая своя копия убрана");
  const exited = new Promise((resolve) => holder.once("exit", resolve));
  holder.kill();
  await exited;
});

test("раунд 1: цикл удаления не трогает каталог программы, созданный заново (проба probe-loop)", { skip: process.platform !== "win32" }, async (t) => {
  const root = tempDir(t);
  const program = path.join(root, "HLSR", "Launcher");
  const data = path.join(root, "AppData", "Roaming", "hlsr");
  fs.mkdirSync(program, { recursive: true });
  fs.mkdirSync(data, { recursive: true });
  fs.writeFileSync(path.join(program, "HLSR.exe"), "old");
  const locked = path.join(data, "lockfile");
  fs.writeFileSync(locked, "x");
  const powershell = path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  const holder = require("child_process").spawn(powershell,
    ["-NoProfile", "-NonInteractive", "-Command", "$f = [IO.File]::Open($env:HLSR_TEST_FILE, 'Open', 'ReadWrite', 'None'); Start-Sleep -Seconds 6; $f.Close()"],
    { stdio: "ignore", windowsHide: true, env: { ...process.env, HLSR_TEST_FILE: locked } });
  t.after(() => holder.kill());
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const command = uninstaller.removalCommand(program, process.env, null, { data: [data], parent: path.dirname(program) });
  const script = require("child_process").spawn(command.file, command.args, { env: command.env, stdio: "ignore", windowsHide: true });
  const exited = new Promise((resolve) => script.once("exit", resolve));
  for (let i = 0; i < 100 && fs.existsSync(program); i++) await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(fs.existsSync(program), false);
  // Новая установка в тот же каталог, пока данные ещё заняты.
  await new Promise((resolve) => setTimeout(resolve, 500));
  fs.mkdirSync(program, { recursive: true });
  fs.writeFileSync(path.join(program, "HLSR.exe"), "new install");
  await exited;
  assert.equal(fs.readFileSync(path.join(program, "HLSR.exe"), "utf8"), "new install");
  assert.equal(fs.existsSync(data), false, "данные удалены, когда освободились");
});

test("раунд 1: read-only у файла с жёсткой ссылкой наружу не снимается, read-only junction удаляется", { skip: process.platform !== "win32" }, async (t) => {
  const root = tempDir(t);
  const outside = path.join(root, "outside");
  fs.mkdirSync(path.join(outside, "dir"), { recursive: true });
  fs.writeFileSync(path.join(outside, "ro.txt"), "keep");
  fs.chmodSync(path.join(outside, "ro.txt"), 0o444);
  t.after(() => { try { fs.chmodSync(path.join(outside, "ro.txt"), 0o666); } catch (_) { /* удалён */ } });
  const data = path.join(root, "hlsr");
  fs.mkdirSync(data);
  fs.linkSync(path.join(outside, "ro.txt"), path.join(data, "hard.txt"));
  fs.symlinkSync(path.join(outside, "dir"), path.join(data, "link"), "junction");
  fs.writeFileSync(path.join(data, "own.txt"), "x");
  require("child_process").spawnSync(path.join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    ["-NoProfile", "-NonInteractive", "-Command", "$j = Get-Item -LiteralPath $env:HLSR_TEST_FILE -Force; $j.Attributes = $j.Attributes -bor 1"],
    { stdio: "ignore", windowsHide: true, env: { ...process.env, HLSR_TEST_FILE: path.join(data, "link") } });
  await runRemoval(null, { data: [data] }, () => !fs.existsSync(path.join(data, "link")) && !fs.existsSync(path.join(data, "own.txt")));
  assert.equal(fs.existsSync(path.join(data, "link")), false, "read-only junction удалён");
  assert.equal(fs.existsSync(path.join(outside, "dir")), true);
  assert.equal(fs.statSync(path.join(outside, "ro.txt")).mode & 0o200, 0, "файл снаружи остался read-only");
  assert.equal(fs.readFileSync(path.join(outside, "ro.txt"), "utf8"), "keep");
});

// Апдейтер так, как его раскладывает сборка в dist/electron/updater
// (scripts/lib/updater-bundle.js): src/updater без shared/ и модули src/main
// на месте shared/*.
function packagedUpdater(root) {
  return require("./lib/updater-bundle").stage(path.join(root, "updater"));
}

test("updater JS is minified in place and keeps working", async (t) => {
  const { minifyDir, jsFiles } = require("./minify-js");
  const dir = packagedUpdater(tempDir(t));
  const plain = jsFiles(dir);
  // Модули без electron: их можно загрузить в обычном Node до и после.
  const loadable = plain.filter((name) => !/^(main|window\/preload)\.js$/.test(name) && !name.startsWith("window/"));
  const exportsOf = (name) => {
    const file = require.resolve(path.join(dir, name));
    delete require.cache[file];
    const value = require(file);
    return typeof value === "function" ? ["<function>"] : Object.keys(value).sort();
  };
  const before = Object.fromEntries(loadable.map((name) => [name, exportsOf(name)]));
  for (const name of Object.keys(require.cache)) if (name.startsWith(dir)) delete require.cache[name];

  const results = await minifyDir(dir, { browserScripts: ["window/updater.js"] });
  assert.deepEqual(results.map((item) => item.relative), plain);
  assert.ok(plain.includes("shared/launcher-update/launcher-channel.js"));
  for (const item of results) {
    const code = fs.readFileSync(path.join(dir, item.relative), "utf8");
    assert.ok(item.after < item.before, `${item.relative} не уменьшился`);
    assert.doesNotMatch(code, /\/\/ |\/\*/, `${item.relative}: остались комментарии`);
  }
  for (const name of loadable) assert.deepEqual(exportsOf(name), before[name], name);

  // Скрипт окна: верхнеуровневые имена не сокращены, preload отдаёт тот же мост.
  assert.match(fs.readFileSync(path.join(dir, "window/updater.js"), "utf8"), /window\.hlsrUpdaterIpc/);
  assert.match(fs.readFileSync(path.join(dir, "window/preload.js"), "utf8"), /exposeInMainWorld\("hlsrUpdaterIpc"/);
  // Класс ошибки сохраняет имя: по нему isDeliveryError узнаёт ошибки.
  const errors = require(path.join(dir, "shared/game-delivery/errors.js"));
  assert.equal(new errors.DeliveryError("X", "y").name, "DeliveryError");
});

// Установщик: окно (build/installer) и модули AppImage-установщика
// (build-bootstrap-app.js) раскладываются как в сборке и сжимаются на месте.
test("installer window and bootstrap modules are minified and keep working", async (t) => {
  const { minifyDir } = require("./minify-js");
  const { modules } = require("./build-bootstrap-app");
  const repo = path.resolve(__dirname, "..");
  const root = tempDir(t);

  const window = path.join(root, "installer");
  fs.mkdirSync(window);
  for (const name of ["installer.js", "preload.js"]) fs.copyFileSync(path.join(repo, "src/installer", name), path.join(window, name));
  const windowResults = await minifyDir(window, { browserScripts: ["installer.js"] });
  assert.deepEqual(windowResults.map((item) => item.relative), ["installer.js", "preload.js"]);
  const installerJs = fs.readFileSync(path.join(window, "installer.js"), "utf8");
  assert.match(installerJs, /window\.installer\b/);
  assert.match(fs.readFileSync(path.join(window, "preload.js"), "utf8"), /exposeInMainWorld\("installer"/);

  const stage = path.join(root, "bootstrap");
  for (const relative of modules) {
    fs.mkdirSync(path.dirname(path.join(stage, relative)), { recursive: true });
    fs.copyFileSync(path.join(repo, relative), path.join(stage, relative));
  }
  // Модули без electron можно загрузить в обычном Node до и после сжатия.
  const loadable = modules.filter((name) => !fs.readFileSync(path.join(repo, name), "utf8").includes('require("electron")'));
  const exportsOf = (name) => {
    for (const key of Object.keys(require.cache)) if (key.startsWith(stage)) delete require.cache[key];
    const value = require(path.join(stage, name));
    return typeof value === "function" ? ["<function>"] : Object.keys(value).sort();
  };
  const before = Object.fromEntries(loadable.map((name) => [name, exportsOf(name)]));
  const results = await minifyDir(path.join(stage, "src"));
  assert.deepEqual(results.map((item) => `src/${item.relative}`).sort(), [...modules].sort());
  for (const item of [...windowResults, ...results]) {
    assert.ok(item.after < item.before, `${item.relative} не уменьшился`);
  }
  for (const name of modules)
    assert.doesNotMatch(fs.readFileSync(path.join(stage, name), "utf8"), /\/\/ |\/\*/, `${name}: остались комментарии`);
  for (const name of loadable) assert.deepEqual(exportsOf(name), before[name], name);
});

// Внешний апдейтер с fuses onlyLoadAppFromAsar и проверкой целостности asar:
// копия exe запускается с копией app.asar лаунчера и флагом режима
// (src/main/launcher-update/updater-mode.js).
test("updater mode: only the flag on Windows switches the entry point", () => {
  const updaterMode = require("../src/main/launcher-update/updater-mode");
  assert.equal(updaterMode.isUpdaterInvocation(["HLSRUpdater.exe", updaterMode.FLAG, "--plan=C:\\p.json"], "win32"), true);
  assert.equal(updaterMode.isUpdaterInvocation(["HLSR.exe", "--plan=C:\\p.json"], "win32"), false);
  assert.equal(updaterMode.isUpdaterInvocation(["hlsr", updaterMode.FLAG], "linux"), false);
  // argv[0] — сам exe, флагом не считается.
  assert.equal(updaterMode.isUpdaterInvocation([updaterMode.FLAG], "win32"), false);

  const calls = [];
  const app = {
    setName: (name) => calls.push(["name", name]),
    setPath: (key, value) => calls.push(["path", key, value]),
    getPath: (key) => (key === "appData" ? path.join("C:", "AppData") : null),
  };
  const loaded = updaterMode.run({ electron: { app }, bundleDir: path.join("A", "dist", "electron"), require: (file) => ({ file }) });
  assert.equal(loaded.file, path.join("A", "dist", "electron", "updater", "main.js"));
  assert.deepEqual(calls, [["name", "HLSR Updater"], ["path", "userData", path.join("C:", "AppData", "HLSR Updater")]]);

  const entry = fs.readFileSync(path.join(__dirname, "../src/main/entry.js"), "utf8");
  assert.match(entry, /isUpdaterInvocation\(process\.argv\)/);
  assert.match(fs.readFileSync(path.join(__dirname, "../.electron-vue/webpack.main.config.js"), "utf8"), /src\/main\/entry\.js/);
});

test("updater stage: exe copy runs with the launcher's own app.asar", (t) => {
  const launch = require("../src/main/launcher-update/updater-launch");
  const updaterMode = require("../src/main/launcher-update/updater-mode");
  const root = tempDir(t);
  const appDir = path.join(root, "HLSR");
  const resources = path.join(appDir, "resources");
  fs.mkdirSync(path.join(resources, "app.asar.unpacked"), { recursive: true });
  fs.writeFileSync(path.join(appDir, "HLSR.exe"), "exe");
  fs.writeFileSync(path.join(appDir, "ffmpeg.dll"), "dll");
  fs.writeFileSync(path.join(resources, "app.asar"), "asar of the launcher");
  fs.writeFileSync(path.join(resources, "QA_TRUST"), "{}");

  const staged = launch.stage({ appDir, resourcesPath: resources, dir: path.join(root, "stage") });
  assert.equal(staged.exe, path.join(root, "stage", launch.UPDATER_EXE));
  assert.equal(fs.readFileSync(staged.exe, "utf8"), "exe");
  assert.equal(fs.readFileSync(path.join(root, "stage", "ffmpeg.dll"), "utf8"), "dll");
  // Байт-в-байт тот же asar: заголовок сверяется с хешем, зашитым в exe.
  assert.equal(fs.readFileSync(path.join(root, "stage", "resources", "app.asar"), "utf8"), "asar of the launcher");
  assert.equal(fs.existsSync(path.join(root, "stage", "resources", "QA_TRUST")), true);
  // resources/app fuse onlyLoadAppFromAsar всё равно не загрузит.
  assert.deepEqual(fs.readdirSync(path.join(root, "stage", "resources")).sort(), ["QA_TRUST", "app.asar"]);

  fs.rmSync(path.join(resources, "app.asar"));
  assert.throws(() => launch.stage({ appDir, resourcesPath: resources, dir: path.join(root, "stage2") }), /app\.asar/);

  const spawned = [];
  launch.spawnUpdater(staged.exe, staged.planFile, { spawn: (exe, args) => (spawned.push(args), { pid: 42 }) });
  assert.deepEqual(spawned, [[updaterMode.FLAG, `--plan=${staged.planFile}`]]);
});

test("updater: подброшенный QA_TRUST действует только в QA-сборке", (t) => {
  const bundle = require("./lib/updater-bundle");
  const root = tempDir(t);
  const resources = path.join(root, "resources");
  fs.mkdirSync(resources);
  fs.writeFileSync(path.join(resources, "QA_TRUST"), JSON.stringify({ publicKeys: ["attacker"], downloadBase: "https://attacker.example/d" }));

  const production = require(path.join(bundle.stage(path.join(root, "production"), { channel: "production" }), "core", "qa-trust.js"));
  assert.equal(production.read(resources), null);
  const qa = require(path.join(bundle.stage(path.join(root, "qa"), { channel: "qa" }), "core", "qa-trust.js"));
  assert.deepEqual(qa.read(resources), { publicKeys: ["attacker"], downloadBase: "https://attacker.example/d" });
  assert.equal(qa.read(path.join(root, "нет такой папки")), null);
  // Без явного канала в сборку идёт боевой, а не QA: HLSR_BUILD_CHANNEL решает только при сборке.
  const byDefault = JSON.parse(fs.readFileSync(path.join(bundle.stage(path.join(root, "default")), "build-channel.json"), "utf8"));
  assert.equal(byDefault.channel, process.env.HLSR_BUILD_CHANNEL === "qa" ? "qa" : "production");
});

test("updater bundle: every shared module and window file is in place", (t) => {
  const bundle = require("./lib/updater-bundle");
  const dir = bundle.stage(path.join(tempDir(t), "updater"));
  for (const name of ["main.js", "locales.js", "core/session.js", "window/index.html", "window/preload.js", "window/updater.js", "window/updater.css"])
    assert.ok(fs.existsSync(path.join(dir, name)), name);
  const shared = bundle.sharedModules();
  assert.ok(shared.length >= 8);
  for (const item of shared) {
    const code = fs.readFileSync(path.join(dir, item.to), "utf8");
    assert.equal(code, fs.readFileSync(item.from, "utf8"), item.to);
    // Реальный модуль, а не ссылка в src/main: в asar её не по чему разрешить.
    assert.doesNotMatch(code, /require\("\.\.\/\.\.\/\.\.\/main\//, item.to);
  }
  // Модули апдейтера подключаются из одной папки, без src/main рядом.
  const session = require(path.join(dir, "core", "session.js"));
  assert.equal(typeof session, "function");
  const qa = fs.readFileSync(path.join(dir, "main.js"), "utf8");
  assert.match(qa, /qaTrustOptions\(resourcesPath = process\.resourcesPath\)/);
});

test("апдейтер читает версию HLSR.exe, путь с ‘ не становится кодом PowerShell", { skip: process.platform !== "win32" }, (t) => {
  const { readExeProductVersion } = require("../src/updater/core/install");
  const original = path.join(process.env.SystemRoot || "C:\\Windows", "System32", "whoami.exe");
  const dir = path.join(tempDir(t), "x‘);Write-Output 9.9.9;#");
  fs.mkdirSync(dir);
  const exe = path.join(dir, "HLSR.exe");
  fs.copyFileSync(original, exe);
  // Сверяем X.Y.Z: у файла в System32 последнюю часть дополняет его .mui.
  const triple = (value) => String(value).split(".").slice(0, 3).join(".");
  const expected = triple(readExeProductVersion(original));
  assert.match(expected, /^\d+\.\d+\.\d+$/);
  assert.equal(triple(readExeProductVersion(exe)), expected);
});
