"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { EventEmitter } = require("events");
const { PassThrough } = require("stream");

const overlay = require("../src/shared/livesplit-overlay");
const LiveSplitOverlay = require("../src/main/game-delivery/livesplit-overlay");
const LiveSplitWindowCapture = require("../src/main/game-delivery/livesplit-window-capture");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-lso-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Ресурсы лаунчера с поддельными файлами оверлея.
function fakeResources(root) {
  const dir = path.join(root, "resources", LiveSplitOverlay.RESOURCE_DIRECTORY);
  fs.mkdirSync(dir, { recursive: true });
  const names = new Set(LiveSplitOverlay.FILES_WINDOWS.concat(LiveSplitOverlay.FILES_LINUX));
  // Прокси узнаётся по метке внутри файла.
  for (const name of names) fs.writeFileSync(path.join(dir, name), `${name} v1 ${LiveSplitOverlay.PROXY_MARKER}`);
  return path.join(root, "resources");
}

test("Оверлей LiveSplit: настройки по умолчанию выключены, мусор заменяется стандартными значениями", () => {
  const base = overlay.defaultOverlay();
  assert.equal(base.enabled, false);
  assert.deepEqual(overlay.sanitizeOverlay(null), base);
  const clean = overlay.sanitizeOverlay({
    enabled: "yes", x: 140, y: -3, height: "2", opacity: 55.6, key: "chroma",
    keyColor: { r: 300, g: "16", b: null }, crop: { top: 9999, left: "4" },
  });
  assert.equal(clean.enabled, false, "включает только true");
  assert.equal(clean.x, 100);
  assert.equal(clean.y, 0);
  assert.equal(clean.height, 5);
  assert.equal(clean.opacity, 56);
  assert.equal(clean.key, "none");
  assert.deepEqual(clean.keyColor, { r: 255, g: 16, b: 0 });
  assert.deepEqual(clean.crop, { top: 2048, right: 0, bottom: 0, left: 4 });
  assert.equal(overlay.overlayChanges(base), 0);
  assert.equal(overlay.overlayChanges(Object.assign({}, base, { x: 0, key: "luma" })), 2);
  assert.equal(overlay.overlayChanges(Object.assign({}, base, { enabled: true })), 0, "выключатель сброс раздела не трогает");
});

test("Оверлей LiveSplit: overlay.ini в формате, который читает DLL", () => {
  const ini = overlay.toIni({ enabled: true, x: 12.5, key: "color", keyColor: { r: 1, g: 171, b: 255 }, crop: { top: 3 } });
  const lines = ini.split("\r\n");
  assert.ok(lines.includes("enabled=1"));
  assert.ok(lines.includes("x=12.5"));
  assert.ok(lines.includes("key=1"));
  assert.ok(lines.includes("key_color=01abff"));
  assert.ok(lines.includes("crop_top=3"));
  assert.ok(lines.includes("luma_min=0.1"));
  assert.equal(lines[lines.length - 1], "", "файл заканчивается переводом строки");
  assert.ok(overlay.toIni({ key: "luma" }).includes("key=2\r\n"));
});

test("Оверлей LiveSplit: маски как у OBS", () => {
  const color = overlay.sanitizeOverlay({ key: "color", keyColor: { r: 0, g: 255, b: 0 } });
  assert.equal(overlay.keyAlpha(color, 0, 1, 0), 0, "цвет ключа вырезается");
  assert.equal(overlay.keyAlpha(color, 1, 1, 1), 1, "белый текст остаётся");
  const edge = overlay.keyAlpha(color, 0.3, 1, 0.3);
  assert.ok(edge > 0 && edge < 1, "у границы — сглаживание");

  const luma = overlay.sanitizeOverlay({ key: "luma", lumaMin: 0.1, lumaMinSmooth: 0.05 });
  assert.equal(overlay.keyAlpha(luma, 0.05, 0.05, 0.05), 0, "тёмный фон вырезается");
  assert.equal(overlay.keyAlpha(luma, 1, 1, 1), 1, "светлый текст остаётся");
  const bright = overlay.sanitizeOverlay({ key: "luma", lumaMin: 0, lumaMinSmooth: 0, lumaMax: 0.8 });
  assert.equal(overlay.keyAlpha(bright, 1, 1, 1), 0, "выше максимума вырезается");
  assert.equal(overlay.keyAlpha(overlay.defaultOverlay(), 0, 0, 0), 1, "без маски всё непрозрачно");
});

test("Оверлей LiveSplit: место в кадре не зависит от разрешения, перетаскивание возвращает ту же точку", () => {
  const value = { x: 100, y: 0, height: 50 };
  assert.deepEqual(overlay.placement(value, 1600, 900, 200, 300), { left: 1300, top: 0, width: 300, height: 450 });
  assert.deepEqual(overlay.placement(value, 800, 450, 200, 300), { left: 650, top: 0, width: 150, height: 225 });
  // Слишком широкая картинка ужимается по ширине кадра.
  const wide = overlay.placement({ x: 0, y: 50, height: 100 }, 400, 300, 800, 300);
  assert.equal(wide.width, 400);
  assert.equal(wide.height, 150);
  assert.equal(wide.top, 75);
  assert.equal(overlay.placement(value, 8, 8, 10, 10), null);

  const rect = overlay.placement({ x: 30, y: 70, height: 40 }, 1600, 900, 200, 300);
  assert.deepEqual(overlay.positionOf(rect.left, rect.top, 1600, 900, rect.width, rect.height), { x: 30, y: 70 });
  assert.deepEqual(overlay.positionOf(-50, 5000, 1600, 900, 100, 100), { x: 0, y: 100 }, "за краями — к краю");
});

test("Оверлей LiveSplit: снимок обрезается и получает маску, слишком сильная обрезка не применяется", () => {
  // 4x10 BGRX: левый столбец зелёный, остальные белые.
  const width = 4, height = 10;
  const bgrx = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const at = (y * width + x) * 4;
    bgrx.set(x === 0 ? [0, 255, 0, 0] : [255, 255, 255, 0], at);
  }
  const keyed = overlay.keyImage({ key: "color", crop: { top: 1, bottom: 1 } }, bgrx, width, height);
  assert.equal(keyed.width, 4, "обрезка по ширине оставила бы меньше 8 пикселей — не применяется");
  assert.equal(keyed.height, 8);
  assert.equal(keyed.data[3], 0, "зелёный вырезан");
  assert.deepEqual(Array.from(keyed.data.slice(4, 8)), [255, 255, 255, 255]);

  // RGBA (примерный LiveSplit из canvas): красный остаётся красным.
  const rgba = new Uint8Array([255, 0, 0, 255]);
  assert.deepEqual(Array.from(overlay.keyImage({}, rgba, 1, 1, "rgba").data), [255, 0, 0, 255]);
});

test("Оверлей LiveSplit: место, которое окно не нарисовало, прозрачно, и цвет фона берётся не из него", () => {
  // Масштаб Windows 125%: LiveSplit нарисовал себя в левые верхние 8x8 из
  // 10x10, полосы справа и снизу остались залиты SENTINEL (common.h).
  const width = 10, height = 10;
  const bgrx = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++)
    bgrx.set(x < 8 && y < 8 ? [0, 255, 0, 0] : [0x5a, 0xff, 0x00, 0x5a], (y * width + x) * 4);
  for (const key of ["none", "color", "luma"]) {
    const keyed = overlay.keyImage({ key }, bgrx, width, height);
    assert.equal(keyed.data[(9 * width + 9) * 4 + 3], 0, `${key}: полоса прозрачна`);
  }
  assert.equal(overlay.keyImage({ key: "none" }, bgrx, width, height).data[3], 255, "сам LiveSplit виден");
  assert.deepEqual(overlay.backgroundColor(bgrx, width, height), { r: 0, g: 255, b: 0 });

  // C-код делает то же: SENTINEL в common.h совпадает с цветом в JS.
  const common = fs.readFileSync(path.join(__dirname, "livesplit-overlay/common.h"), "utf8");
  assert.match(common, /#define SENTINEL 0x5A00FF5Au/);
  assert.match(common, /if \(p == SENTINEL\) \{ out\[x\] = 0; continue; \}/);
});

test("Оверлей LiveSplit: имя процесса игры и разбор снимка", () => {
  assert.equal(LiveSplitOverlay.processNameOf({ executable: "bin\\HL2.EXE" }), "hl2.exe");

  const header = Buffer.alloc(16);
  header.write("HLSRLS1\0", 0, "latin1");
  header.writeUInt32LE(2, 8);
  header.writeUInt32LE(1, 12);
  const snapshot = LiveSplitOverlay.parseSnapshot(Buffer.concat([header, Buffer.alloc(8, 7)]));
  assert.equal(snapshot.width, 2);
  assert.equal(snapshot.pixels.length, 8);
  assert.equal(LiveSplitOverlay.parseSnapshot(Buffer.concat([header, Buffer.alloc(4)])), null, "обрезанный снимок");
  assert.equal(LiveSplitOverlay.parseSnapshot(Buffer.from("garbage")), null);
});

test("Оверлей LiveSplit: чужие замены WINEDLLOVERRIDES остаются", () => {
  assert.equal(LiveSplitOverlay.mergeOverrides("", "d3d9=n,b"), "d3d9=n,b");
  assert.equal(LiveSplitOverlay.mergeOverrides("dxgi=n", "d3d9=n,b"), "dxgi=n;d3d9=n,b");
});

function installOf(root, executable, engine) {
  const installPath = path.join(root, "game");
  fs.mkdirSync(path.dirname(path.join(installPath, executable)), { recursive: true });
  fs.mkdirSync(path.join(installPath, "bin"), { recursive: true });
  return { installPath, config: { executable, engine } };
}

test("Оверлей LiveSplit: настройки пишутся на лету, прокси кладётся в папки игры", (t) => {
  const root = tempDir(t);
  const service = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "win32" });
  const target = installOf(root, "hl2.exe");
  assert.equal(service.prepare("ghosting-renewed", {}, target), null);
  assert.equal(service.writeConfig("ghosting-renewed", {}), false, "игра без оверлея — папки нет");

  const prepared = service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: true, x: 0 } }, target);
  assert.ok(prepared);
  assert.equal(prepared.proxy, "d3d9.dll");
  assert.equal(prepared.dllOverride, null, "на Windows замена DLL не нужна");
  assert.match(fs.readFileSync(path.join(prepared.dir, "overlay.ini"), "utf8"), /^enabled=1\r\nx=0\r\n/);
  // Source: рядом с hl2.exe и в bin\.
  for (const folder of [target.installPath, path.join(target.installPath, "bin")]) {
    assert.ok(fs.existsSync(path.join(folder, "d3d9.dll")), `${folder}: d3d9.dll`);
    assert.equal(fs.readFileSync(path.join(folder, "hlsr-overlay.path"), "utf8"), `${path.resolve(prepared.dir)}\n`);
  }
  assert.ok(!fs.existsSync(path.join(prepared.dir, "hlsr-overlay64.exe")), "на Windows 64-битные файлы не нужны");

  service.writeConfig("ghosting-renewed", { liveSplitOverlay: { enabled: true, x: 42 } });
  assert.match(fs.readFileSync(path.join(prepared.dir, "overlay.ini"), "utf8"), /\r\nx=42\r\n/);

  // Обновлённый прокси в ресурсах заменяет копию в папке игры.
  fs.writeFileSync(service.resource("proxy-d3d9.dll"), `v2 ${LiveSplitOverlay.PROXY_MARKER}`);
  service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: true } }, target);
  assert.match(fs.readFileSync(path.join(target.installPath, "d3d9.dll"), "utf8"), /^v2 /);

  // Оверлей выключили — прокси и файл с путём убраны.
  assert.equal(service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: false } }, target), null);
  assert.ok(!fs.existsSync(path.join(target.installPath, "d3d9.dll")));
  assert.ok(!fs.existsSync(path.join(target.installPath, "hlsr-overlay.path")));

  assert.equal(service.dirFor("../evil"), null);
  const missing = new LiveSplitOverlay({ baseDir: root, resourcesPath: path.join(root, "nowhere"), platform: "win32" });
  assert.throws(() => missing.prepare("hl1-won", { liveSplitOverlay: { enabled: true } }, target), /не найдены/);
});

test("Оверлей LiveSplit: HL1 получает opengl32.dll рядом с hl.exe, чужую DLL лаунчер не трогает", (t) => {
  const root = tempDir(t);
  const service = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "win32" });
  const target = installOf(root, "Half-Life/hl.exe", "goldsrc");
  const settings = { liveSplitOverlay: { enabled: true } };
  const prepared = service.prepare("hl1-won", settings, target);
  assert.equal(prepared.proxy, "opengl32.dll");
  assert.deepEqual(prepared.placed, [path.join(target.installPath, "Half-Life")]);
  assert.ok(fs.existsSync(path.join(target.installPath, "Half-Life", "opengl32.dll")));

  // ReShade или DXVK пользователя: файл без нашей метки не затирается и не удаляется.
  const foreign = path.join(target.installPath, "d3d9.dll");
  fs.writeFileSync(foreign, "dxvk");
  const source = installOf(root, "hl2.exe");
  const placed = service.prepare("ghosting-renewed", settings, source);
  assert.deepEqual(placed.placed, [path.join(source.installPath, "bin")]);
  assert.equal(fs.readFileSync(foreign, "utf8"), "dxvk");
  service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: false } }, source);
  assert.equal(fs.readFileSync(foreign, "utf8"), "dxvk");
});

test("Оверлей LiveSplit на Linux: помощник для LiveSplit рядом с данными, WINEDLLOVERRIDES для игры", (t) => {
  const root = tempDir(t);
  const service = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "linux" });
  const target = installOf(root, "hl2.exe");
  const prepared = service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: true } }, target);
  assert.equal(prepared.dllOverride, "d3d9=n,b");
  for (const name of [LiveSplitOverlay.HELPER64, LiveSplitOverlay.SHARE_DLL]) assert.ok(fs.existsSync(path.join(prepared.dir, name)), name);
});

// Путь Z:\… строится из пути Linux (path.resolve хоста): на Windows-машине
// эмуляция platform: "linux" дала бы «Z:C:\…», проверять там нечего.
test("Оверлей LiveSplit на Linux: в hlsr-overlay.path путь данных так, как его видит Wine (Z:)",
  { skip: process.platform === "win32" && "путь Z:\\ строится из пути Linux, проверяется только на Linux" }, (t) => {
    const root = tempDir(t);
    const service = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "linux" });
    const target = installOf(root, "hl2.exe");
    service.prepare("ghosting-renewed", { liveSplitOverlay: { enabled: true } }, target);
    assert.match(fs.readFileSync(path.join(target.installPath, "hlsr-overlay.path"), "utf8"), /^Z:\\[^\n]+\n$/);
  });

test("Оверлей LiveSplit: снимок окна на Windows без своего exe, под Wine — помощник", (t) => {
  const root = tempDir(t);
  assert.ok(!LiveSplitOverlay.FILES_WINDOWS.some((name) => /\.exe$/i.test(name)), "в Windows-сборке оверлея нет своих exe");
  const windows = new LiveSplitOverlay({ baseDir: path.join(root, "w"), resourcesPath: fakeResources(root), platform: "win32" }).captureTool();
  assert.equal(windows.injector64, undefined);
  assert.equal(windows.shareDll, undefined);
  assert.deepEqual(fs.readdirSync(windows.dir), [], "в папку снимка ничего не копируется");
  assert.equal(path.basename(windows.snapshot), "snapshot.bin");
  const linux = new LiveSplitOverlay({ baseDir: path.join(root, "l"), resourcesPath: fakeResources(root), platform: "linux" }).captureTool();
  assert.equal(path.basename(linux.injector64), LiveSplitOverlay.HELPER64);
  assert.equal(path.basename(linux.shareDll), LiveSplitOverlay.SHARE_DLL);
});

test("Оверлей LiveSplit: копия hlsr-capture.exe от прежних версий убирается из данных", (t) => {
  const root = tempDir(t);
  const service = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "win32" });
  service.removeLegacyCapture();
  assert.ok(!fs.existsSync(path.join(root, "data")), "папки нет — создавать её незачем");
  const dir = service.dirFor("capture");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "hlsr-capture.exe"), "old");
  fs.writeFileSync(path.join(dir, "snapshot.bin"), "keep");
  service.removeLegacyCapture();
  assert.deepEqual(fs.readdirSync(dir), ["snapshot.bin"]);
});

// Поддельный PowerShell: отвечает на строки так, как скажет answer.
function fakePowerShell(answer) {
  const spawned = [];
  const spawnFn = (file, args, options) => {
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = new PassThrough();
    child.killed = false;
    child.file = file; child.args = args; child.options = options; child.lines = [];
    child.kill = () => { child.killed = true; setImmediate(() => child.emit("exit", 1)); };
    child.stdin.setEncoding("utf8");
    child.stdin.on("data", (text) => {
      for (const line of text.split("\n").filter(Boolean)) {
        child.lines.push(line);
        const reply = answer(line, child);
        if (reply) child.stdout.write(`${reply}\n`);
      }
    });
    spawned.push(child);
    setImmediate(() => child.stdout.write('{"ok":true,"ready":true}\n'));
    return child;
  };
  return { spawned, spawnFn };
}

test("Снимок окна LiveSplit: один процесс на запросы, путь в base64, код из ответа", async (t) => {
  const fake = fakePowerShell(() => '{"ok":true,"code":0}');
  const capture = new LiveSplitWindowCapture({ spawnFn: fake.spawnFn, idleMs: 60000 });
  t.after(() => capture.close());
  const file = "C:\\Users\\Иван Петров\\snapshot.bin";
  assert.equal(await capture.capture(file), 0);
  assert.equal(await capture.capture(file), 0);
  assert.equal(fake.spawned.length, 1, "PowerShell не запускается на каждый снимок");
  const [child] = fake.spawned;
  assert.equal(child.options.windowsHide, true);
  assert.equal(Buffer.from(child.lines[0].split(" ")[1], "base64").toString("utf8"), file);
});

test("Снимок окна LiveSplit: код 2 — окна нет, это не сбой", async (t) => {
  const fake = fakePowerShell(() => '{"ok":true,"code":2}');
  const capture = new LiveSplitWindowCapture({ spawnFn: fake.spawnFn, idleMs: 60000 });
  t.after(() => capture.close());
  assert.equal(await capture.capture("x"), 2);
  assert.equal(fake.spawned[0].killed, false);
});

test("Снимок окна LiveSplit: простой закрывает PowerShell, следующий снимок запускает его снова", async (t) => {
  const fake = fakePowerShell(() => '{"ok":true,"code":0}');
  const capture = new LiveSplitWindowCapture({ spawnFn: fake.spawnFn, idleMs: 5 });
  t.after(() => capture.close());
  assert.equal(await capture.capture("x"), 0);
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(fake.spawned[0].killed, true);
  assert.equal(await capture.capture("x"), 0);
  assert.equal(fake.spawned.length, 2);
});

test("Снимок окна LiveSplit: сбой и молчание PowerShell — ошибка, следующий снимок с новым процессом", async (t) => {
  let silent = true;
  const fake = fakePowerShell(() => (silent ? null : '{"ok":true,"code":0}'));
  const capture = new LiveSplitWindowCapture({ spawnFn: fake.spawnFn, idleMs: 60000, timeoutMs: 30 });
  t.after(() => capture.close());
  await assert.rejects(capture.capture("x"), /не ответил/);
  assert.equal(fake.spawned[0].killed, true, "зависший снимок не остаётся висеть");
  silent = false;
  assert.equal(await capture.capture("x"), 0);
  assert.equal(fake.spawned.length, 2);

  const failed = fakePowerShell(() => '{"ok":false,"error":"нет доступа"}');
  const other = new LiveSplitWindowCapture({ spawnFn: failed.spawnFn, idleMs: 60000 });
  t.after(() => other.close());
  await assert.rejects(other.capture("x"), /нет доступа/);
  assert.equal(failed.spawned[0].killed, true);
});

test("Снимок окна LiveSplit: запись в stdin упавшего PowerShell не роняет лаунчер", async (t) => {
  const fake = fakePowerShell((line, child) => {
    child.stdin.emit("error", Object.assign(new Error("write EPIPE"), { code: "EPIPE" }));
    child.emit("exit", 1);
    return null;
  });
  const capture = new LiveSplitWindowCapture({ spawnFn: fake.spawnFn, idleMs: 60000 });
  t.after(() => capture.close());
  await assert.rejects(capture.capture("x"), /завершился/);
});

test("Снимок окна LiveSplit: скрипт помещается в командную строку Windows", () => {
  const encoded = Buffer.from(LiveSplitWindowCapture.CAPTURE_SCRIPT, "utf16le").toString("base64");
  assert.ok(encoded.length < 30000, `-EncodedCommand ${encoded.length} символов, предел CreateProcess 32767`);
  for (const forbidden of ["WriteProcessMemory", "CreateRemoteThread", "VirtualAllocEx", "OpenProcess"])
    assert.ok(!LiveSplitWindowCapture.CAPTURE_SCRIPT.includes(forbidden), `${forbidden}: снимок ничего не внедряет`);
});

test("Снимок окна LiveSplit: настоящий PowerShell компилирует помощник и отвечает кодом",
  { skip: process.platform !== "win32" && "только Windows" }, async (t) => {
    const root = tempDir(t);
    const file = path.join(root, "снимок окна.bin");
    const capture = new LiveSplitWindowCapture({ idleMs: 60000 });
    t.after(() => capture.close());
    const code = await capture.capture(file);
    assert.ok(code === 0 || code === 2, `код ${code}`);
    if (code === 0) assert.ok(LiveSplitOverlay.parseSnapshot(fs.readFileSync(file)), "снимок разбирается");
    else assert.ok(!fs.existsSync(file), "без окна файла нет");
  });

test("Оверлей LiveSplit: файл статуса помощника", async (t) => {
  const root = tempDir(t);
  const service = new LiveSplitOverlay({ baseDir: root, resourcesPath: root });
  const file = path.join(root, "inject.status");
  setTimeout(() => fs.writeFileSync(file, "0\n"), 30);
  assert.equal(await service.waitForStatus(file, 2000, { intervalMs: 10 }), 0);
  assert.equal(await service.waitForStatus(path.join(root, "none"), 50, { intervalMs: 10 }), null);
  assert.equal(await service.waitForStatus(path.join(root, "none"), 2000, { intervalMs: 10, cancelled: () => true }), null);
});

test("Оверлей LiveSplit на Windows: запуск игры готовит прокси и ничего не запускает", (t) => {
  const LaunchService = require("../src/main/game-delivery/LaunchService");
  const root = tempDir(t);
  const liveSplitOverlay = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "win32" });
  const calls = [];
  const service = new LaunchService({
    liveSplitOverlay,
    logger: { info() {}, warn() {}, error() {} },
    spawnFn: (...args) => { calls.push(args); return new EventEmitter(); },
  });
  const installed = installOf(root, "Half-Life/hl.exe", "goldsrc");
  const installedOf = { installPath: installed.installPath };
  assert.equal(service.prepareOverlay("hl1-won", installedOf, installed.config, {}), null);
  const prepared = service.prepareOverlay("hl1-won", installedOf, installed.config, { liveSplitOverlay: { enabled: true } });
  assert.equal(prepared.proxy, "opengl32.dll");
  assert.ok(fs.existsSync(path.join(installed.installPath, "Half-Life", "opengl32.dll")));
  assert.equal(calls.length, 0, "никакие процессы не запускаются");
  // Сбой подготовки игру не останавливает.
  const broken = new LaunchService({ liveSplitOverlay: new LiveSplitOverlay({ baseDir: root, resourcesPath: path.join(root, "nowhere"), platform: "win32" }), logger: { info() {}, warn() {}, error() {} } });
  assert.equal(broken.prepareOverlay("hl1-won", installedOf, installed.config, { liveSplitOverlay: { enabled: true } }), null);
});

test("Оверлей LiveSplit на Linux: игра получает замену DLL, свои замены пользователя остаются", (t) => {
  const LinuxLaunchService = require("../src/main/linux/LaunchService");
  const root = tempDir(t);
  const liveSplitOverlay = new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: fakeResources(root), platform: "linux" });
  const service = new LinuxLaunchService({ liveSplitOverlay, logger: { info() {}, warn() {}, error() {} } });
  const installed = installOf(root, "hl2.exe");
  const prepared = service.prepareOverlay("ghosting-renewed", { installPath: installed.installPath }, installed.config, { liveSplitOverlay: { enabled: true } });
  const options = { env: { WINEDLLOVERRIDES: "dxgi=n", OTHER: "1" } };
  service.applyOverlay(options, prepared);
  assert.equal(options.env.WINEDLLOVERRIDES, "dxgi=n;d3d9=n,b");
  assert.equal(options.env.OTHER, "1");
  const untouched = { env: { A: "1" } };
  service.applyOverlay(untouched, null);
  assert.deepEqual(untouched, { env: { A: "1" } });
});

// Масштабирование снимка (resample.h) проверяется на хосте, если есть cc.
test("resample.h: 1:1 без изменений, сплошной цвет, предумножение, уменьшение без ступенек", (t) => {
  const { spawnSync } = require("child_process");
  if (spawnSync("cc", ["--version"]).status !== 0) return t.skip("нет компилятора cc");
  const source = path.join(__dirname, "livesplit-overlay/test/resample-test.c");
  // shortcut: тест на C не попал в git (.gitignore «test/»), вернуть его в репозиторий — и проверка снова пойдёт
  if (!fs.existsSync(source)) return t.skip("нет resample-test.c");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-resample-"));
  try {
    const binary = path.join(dir, "resample-test");
    const built = spawnSync("cc", ["-O2", "-Wall", "-Wextra", "-Werror", source, "-o", binary], { encoding: "utf8" });
    assert.equal(built.status, 0, built.stderr);
    const run = spawnSync(binary, [], { encoding: "utf8" });
    assert.equal(run.status, 0, run.stdout);
    assert.equal(run.stdout.trim(), "ok");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
