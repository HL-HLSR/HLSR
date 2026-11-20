"use strict";

// Vulkan (DXVK) для Ghosting: настройка, доступность по backend\dxvk_d3d9.dll,
// окружение запуска, прокси без оверлея, проверка файлов и рендер демок на DX9.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const launchArgs = require("../src/main/game-delivery/launch-args");
const manifests = require("../src/main/game-delivery/manifests");
const vulkanBackend = require("../src/main/game-delivery/vulkan-backend");
const mutablePaths = require("../src/main/game-delivery/mutable-paths");
const LaunchService = require("../src/main/game-delivery/LaunchService");
const LiveSplitOverlay = require("../src/main/game-delivery/livesplit-overlay");
const { childEnv } = require("../src/main/demo-tool/render-host");

const GHOSTING = manifests.launchConfigOf("ghosting-renewed");

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-vulkan-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// Папка Ghosting с bin\ и, по желанию, DXVK из сборки 1.0.13.
function ghostingInstall(root, withBackend) {
  const installPath = path.join(root, "game");
  fs.mkdirSync(path.join(installPath, "bin"), { recursive: true });
  fs.mkdirSync(path.join(installPath, "ghostingmod"), { recursive: true });
  fs.writeFileSync(path.join(installPath, "hl2.exe"), "test executable; never spawned");
  fs.writeFileSync(path.join(installPath, "ghostingmod", "gameinfo.txt"), '"GameInfo"\n{\nSteamAppId 243730\n}\n');
  fs.writeFileSync(path.join(installPath, "steam_appid.txt"), "243730\n\0");
  if (withBackend) {
    fs.mkdirSync(path.join(installPath, "backend"));
    fs.writeFileSync(path.join(installPath, "backend", "dxvk_d3d9.dll"), "dxvk");
  }
  return installPath;
}

function overlayService(root) {
  const dir = path.join(root, "resources", LiveSplitOverlay.RESOURCE_DIRECTORY);
  fs.mkdirSync(dir, { recursive: true });
  for (const name of LiveSplitOverlay.FILES_WINDOWS) fs.writeFileSync(path.join(dir, name), `${name} ${LiveSplitOverlay.PROXY_MARKER}`);
  return new LiveSplitOverlay({ baseDir: path.join(root, "data"), resourcesPath: path.join(root, "resources"), platform: "win32" });
}

// Настоящий launch() до spawn: возвращает окружение, с которым стартовала бы игра.
async function launchEnv(installPath, settings, options = {}) {
  const spawned = [];
  const warnings = [];
  const service = new LaunchService({
    platform: "win32",
    env: Object.assign({ PATH: "x", DXVK_HUD: "fps", hlsr_d3d9_backend: "vulkan" }, options.env),
    delivery: {
      processService: { isRunning: () => false, markStarting() {}, cancelStarting() {} },
      installedOf: () => ({ installPath }),
      isBusy: () => false,
    },
    launchRegistry: { "ghosting-renewed": GHOSTING },
    settingsStore: { get: () => launchArgs.sanitizeSettings(settings).settings },
    liveSplitOverlay: options.liveSplitOverlay || null,
    spawnFn: (file, argv, spawnOptions) => {
      spawned.push({ file, argv, env: spawnOptions.env });
      throw new Error("STOP_BEFORE_GAME");
    },
    steamLocator: { describeSteamOnly: () => ({ ok: true, steam: { pid: 1 } }) },
    logger: { info() {}, error() {}, warn(message) { warnings.push(message); } },
  });
  await assert.rejects(service.launch("ghosting-renewed"), /STOP_BEFORE_GAME/);
  assert.equal(spawned.length, 1);
  return { env: spawned[0].env, argv: spawned[0].argv, warnings };
}

test("Vulkan: настройка по умолчанию выключена, включается только true", () => {
  assert.equal(launchArgs.defaultSettings().vulkan, false);
  assert.equal(launchArgs.sanitizeSettings({ vulkan: true }).settings.vulkan, true);
  for (const value of ["true", 1, null, undefined]) assert.equal(launchArgs.sanitizeSettings({ vulkan: value }).settings.vulkan, false);
  assert.deepEqual(launchArgs.buildUserArguments({ vulkan: true }, []), [], "Vulkan — не аргумент запуска");
});

test("Vulkan: переключатель только у Ghosting на Windows и доступен по backend\\dxvk_d3d9.dll", (t) => {
  const root = tempDir(t);
  const without = ghostingInstall(path.join(root, "a"), false);
  const withBackend = ghostingInstall(path.join(root, "b"), true);
  assert.equal(vulkanBackend.availability(without, GHOSTING, fs, "win32"), false);
  assert.equal(vulkanBackend.availability(withBackend, GHOSTING, fs, "win32"), true);
  assert.equal(vulkanBackend.availability(null, GHOSTING, fs, "win32"), false, "игра не установлена");
  assert.equal(vulkanBackend.availability(withBackend, GHOSTING, fs, "linux"), null, "под Proton d3d9 и так через DXVK");
  for (const gameId of ["hl2-4260", "hl2-2707", "hl2-5288", "hl1-won"])
    assert.equal(vulkanBackend.availability(withBackend, manifests.launchConfigOf(gameId), fs, "win32"), null, gameId);
});

test("Vulkan: запуск с DXVK — окружение как у Play Vulkan.cmd, прокси без оверлея, аргументы прежние", async (t) => {
  const root = tempDir(t);
  const installPath = ghostingInstall(root, true);
  const liveSplitOverlay = overlayService(root);
  const plain = await launchEnv(installPath, {}, { liveSplitOverlay });
  const { env, argv } = await launchEnv(installPath, { vulkan: true }, { liveSplitOverlay });

  assert.equal(env.HLSR_D3D9_BACKEND, "vulkan");
  assert.ok(!("hlsr_d3d9_backend" in env), "переменная пользователя другим регистром не остаётся");
  assert.equal(env.DXVK_LOG_PATH, path.join(installPath, "renderer-logs"));
  assert.equal(env.DXVK_SHADER_CACHE_PATH, path.join(installPath, "renderer-cache"));
  assert.equal(env.DXVK_CONFIG_FILE, path.join(installPath, "dxvk.conf"));
  assert.equal(env.DXVK_LOG_LEVEL, "info");
  assert.ok(!("DXVK_HUD" in env), "свой DXVK_HUD пользователя стирается, как в Play Vulkan.cmd");
  assert.ok(fs.statSync(path.join(installPath, "renderer-logs")).isDirectory());
  assert.ok(fs.statSync(path.join(installPath, "renderer-cache")).isDirectory());
  assert.deepEqual(argv, plain.argv, "флаги Play Vulkan.cmd (-insecure сверх обычных, -novid, -windowed, -condebug) не добавляются");
  assert.ok(!argv.includes("-windowed") && !argv.includes("-condebug") && !argv.includes("-novid"));

  // Оверлей выключен, а прокси стоит в обеих папках, и overlay.ini его выключает.
  for (const folder of [installPath, path.join(installPath, "bin")]) {
    assert.ok(liveSplitOverlay.isOwnProxy(path.join(folder, "d3d9.dll")), folder);
    assert.ok(fs.existsSync(path.join(folder, LiveSplitOverlay.SIDECAR)), folder);
  }
  const ini = fs.readFileSync(path.join(liveSplitOverlay.dirFor("ghosting-renewed"), "overlay.ini"), "utf8");
  assert.match(ini, /^enabled=0\r\n/);

  // С включённым оверлеем — тот же прокси, оверлей в overlay.ini включён.
  await launchEnv(installPath, { vulkan: true, liveSplitOverlay: { enabled: true } }, { liveSplitOverlay });
  assert.match(fs.readFileSync(path.join(liveSplitOverlay.dirFor("ghosting-renewed"), "overlay.ini"), "utf8"), /^enabled=1\r\n/);
});

test("Vulkan выключен или DXVK нет: DirectX 9 как раньше, прокси без оверлея убирается", async (t) => {
  const root = tempDir(t);
  const liveSplitOverlay = overlayService(root);
  const installPath = ghostingInstall(root, true);
  await launchEnv(installPath, { vulkan: true }, { liveSplitOverlay });
  assert.ok(fs.existsSync(path.join(installPath, "d3d9.dll")));

  const off = await launchEnv(installPath, { vulkan: false }, { liveSplitOverlay });
  assert.ok(!Object.keys(off.env).some((key) => key.toUpperCase() === "HLSR_D3D9_BACKEND"), "без переменной прокси берёт системную d3d9.dll");
  assert.equal(off.env.DXVK_HUD, "fps", "чужие DXVK_* без Vulkan не трогаются");
  assert.ok(!("DXVK_LOG_PATH" in off.env));
  assert.ok(!fs.existsSync(path.join(installPath, "d3d9.dll")), "оверлей выключен — прокси убран, как раньше");
  assert.ok(!fs.existsSync(path.join(installPath, "bin", "d3d9.dll")));

  const old = ghostingInstall(path.join(root, "old"), false);
  const missing = await launchEnv(old, { vulkan: true }, { liveSplitOverlay });
  assert.ok(!Object.keys(missing.env).some((key) => key.toUpperCase() === "HLSR_D3D9_BACKEND"));
  assert.ok(missing.warnings.some((line) => /backend\/dxvk_d3d9\.dll: запуск на DirectX 9/.test(line)), missing.warnings.join("\n"));
  assert.ok(!fs.existsSync(path.join(old, "d3d9.dll")));
  assert.ok(!fs.existsSync(path.join(old, "renderer-logs")));
});

test("Vulkan: чужую d3d9.dll (DXVK или ReShade пользователя) лаунчер не трогает", async (t) => {
  const root = tempDir(t);
  const liveSplitOverlay = overlayService(root);
  const installPath = ghostingInstall(root, true);
  fs.writeFileSync(path.join(installPath, "bin", "d3d9.dll"), "reshade");
  await launchEnv(installPath, { vulkan: true }, { liveSplitOverlay });
  assert.equal(fs.readFileSync(path.join(installPath, "bin", "d3d9.dll"), "utf8"), "reshade");
  assert.ok(liveSplitOverlay.isOwnProxy(path.join(installPath, "d3d9.dll")));
  await launchEnv(installPath, {}, { liveSplitOverlay });
  assert.equal(fs.readFileSync(path.join(installPath, "bin", "d3d9.dll"), "utf8"), "reshade");
});

test("Vulkan: журналы и кэш DXVK не ломают проверку файлов и не идут в .hlsrpack", () => {
  for (const file of ["renderer-logs/hl2_d3d9.log", "renderer-logs/hl2_device_fault.bin", "renderer-cache/0123456789abcdef.dxvk.bin", "renderer-cache/0123456789abcdef.dxvk.lut"])
    assert.ok(mutablePaths.matchesMutable(file), file);
  const engineCaches = mutablePaths.createMatcher(mutablePaths.ENGINE_CACHES);
  assert.ok(engineCaches("renderer-cache/x.dxvk.bin"));
  // Файлы DXVK из сборки проверяются как обычные файлы игры.
  for (const file of ["backend/dxvk_d3d9.dll", "dxvk.conf", "licenses/DXVK.txt"]) assert.ok(!mutablePaths.matchesMutable(file), file);
});

test("Рендер демок всегда на DirectX 9: HLSR_D3D9_BACKEND не доходит до игры", () => {
  const env = childEnv({ PATH: "x", HLSR_D3D9_BACKEND: "vulkan", hlsr_d3d9_backend: "vulkan" });
  assert.deepEqual(Object.keys(env), ["PATH"]);
});
