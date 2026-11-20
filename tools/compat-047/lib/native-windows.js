"use strict";

// Приёмка 0.4.7, Windows, путь Б без Electron: замороженные install.js и process-wait.js запускают
// HLSR-Setup-<v>.exe так же, как внешний апдейтер 0.4.7 (`/S --updated /D=<каталог>`, ожидание
// кода 0 до 10 мин, проверка версии), затем `--updated /S --force-run` без `/D=` (путь А: каталог
// из реестра) и `HLSR.exe --hlsr-uninstall /S`.
//
// Два режима, и граница между ними жёсткая:
//  * devHooks — установщик — hlsr.exe, собранный с dev-hooks; реестр — файл JSON
//    (HLSR_FAKE_REGISTRY), ярлыки, данные и журналы во временном каталоге (HLSR_KNOWN_FOLDERS,
//    HLSR_TEMP_DIR, HLSR_DATA_DIR, HLSR_LOGS_DIR), запуск процессов только записывается
//    (HLSR_LAUNCH_DRY_RUN). Настоящие реестр, меню «Пуск», %APPDATA%\hlsr, %LOCALAPPDATA%\hlsr и
//    C:\HLSR не трогаются — так проверяется на машине, где стоит настоящая 0.4.7. Без полного набора
//    перенаправлений hlsr.exe с dev-hooks режимы установщика не запускает.
//  * настоящий — боевая сборка без перенаправлений, настоящий HKCU. Разрешён только на
//    одноразовом раннере CI (GITHUB_ACTIONS=true): раннер после задачи выбрасывается.

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { loadAs } = require("./load-as");
const { sha256Hex } = require("./accept-windows");

const W = "win32";
const POLLUTION = {
  SLINT_SCALE_FACTOR: "1.25",
  WINIT_X11_SCALE_FACTOR: "1.25",
  PORTABLE_EXECUTABLE_DIR: "X:\\unpacked",
};

function findFiles(root, suffix, out = []) {
  if (!fs.existsSync(root)) return out;
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) findFiles(full, suffix, out);
    else if (entry.name.toLowerCase().endsWith(suffix)) out.push(full);
  }
  return out;
}

function shortcutTarget(link) {
  const run = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command",
    `(New-Object -ComObject WScript.Shell).CreateShortcut('${link.replace(/'/g, "''")}').TargetPath`], { encoding: "utf8", timeout: 30000 });
  return (run.stdout || "").trim();
}

/** Значение реестра: модель JSON (devHooks) или reg.exe (раннер CI). */
function registryReader({ devHooks, registryFile }) {
  if (devHooks) {
    return (key, name) => {
      if (!fs.existsSync(registryFile)) return null;
      const model = JSON.parse(fs.readFileSync(registryFile, "utf8"));
      const section = Object.keys(model).find((candidate) => candidate.toLowerCase() === key.toLowerCase());
      if (!section) return null;
      const value = Object.keys(model[section]).find((candidate) => candidate.toLowerCase() === name.toLowerCase());
      return value ? model[section][value].data : null;
    };
  }
  return (key, name) => {
    const run = spawnSync("reg", ["query", key.replace(/^HKEY_CURRENT_USER/, "HKCU"), "/v", name], { encoding: "utf8" });
    if (run.status !== 0) return null;
    const match = new RegExp(`^\\s*${name}\\s+REG_\\w+\\s+(.*)$`, "im").exec(run.stdout);
    return match ? match[1].trim() : null;
  };
}

async function nativeWindows(ctx, report, { installer, version, setup, hlsrExeSize }) {
  report.begin(`Windows, база ${ctx.base}, путь Б на месте (${ctx.devHooks ? "dev-hooks, перенаправления" : "настоящий реестр раннера"})`);
  if (process.platform !== "win32") return report.skip("путь Б на месте", "нужна Windows (Wine — на Linux-машине, в linux-deferred.md)");
  if (!ctx.devHooks && process.env.GITHUB_ACTIONS !== "true") {
    return report.fail("путь Б на месте", "без --dev-hooks установщик пишет в настоящий реестр и меню «Пуск»: допустимо только на одноразовом раннере CI (GITHUB_ACTIONS=true)");
  }

  const install = loadAs("updater/core/install.js", W, ctx.set);
  const processWait = loadAs("updater/core/process-wait.js", W, ctx.set);
  const layout = loadAs("main/windows-install/layout.js", W, ctx.set);
  const identity = layout.identity(setup.list.app);
  const keys = {
    install: `HKEY_CURRENT_USER\\Software\\${identity.guid}`,
    uninstall: `HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${identity.guid}`,
  };

  const work = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-accept-win-"));
  const home = path.join(work, "home");
  const temp = path.join(work, "temp");
  const data = path.join(work, "data");
  const logs = path.join(work, "logs");
  const library = path.join(work, "Library");
  const registryFile = path.join(work, "registry.json");
  const dryRun = path.join(work, "dry-run.jsonl");
  const installDir = path.join(work, "HLSR", "Launcher");
  const exe = path.join(installDir, "HLSR.exe");
  for (const dir of [home, temp, data]) fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(data, "library.json"), JSON.stringify({ libraryPath: library }));

  const env = { ...process.env, ...POLLUTION };
  delete env.PORTABLE_EXECUTABLE_FILE;
  if (ctx.devHooks) {
    Object.assign(env, {
      HLSR_FAKE_REGISTRY: registryFile, HLSR_KNOWN_FOLDERS: home, HLSR_TEMP_DIR: temp,
      HLSR_LAUNCH_DRY_RUN: dryRun, HLSR_DATA_DIR: data, HLSR_LOGS_DIR: logs,
    });
  }
  const registry = registryReader({ devHooks: ctx.devHooks, registryFile });
  const dry = () => (fs.existsSync(dryRun)
    ? fs.readFileSync(dryRun, "utf8").split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line)) : []);
  const homeLinks = () => (ctx.devHooks ? findFiles(home, ".lnk") : []);

  try {
    // Без /D= установщик взял бы C:\HLSR\Launcher: первый запуск обязан его получить.
    if (!install.installerArgsFor(installDir).some((arg) => arg.startsWith("/D=")))
      throw new Error(`путь ${installDir} с пробелом или кавычкой: апдейтер 0.4.7 не передаст /D=; задайте TEMP без пробелов`);

    await report.step("установщик /S --updated /D=<каталог>: код 0 (install.runInstaller)", async () => {
      const result = await install.runInstaller(installer, { installDir, env, timeoutMs: 10 * 60 * 1000, windowsHide: true });
      if (!result.ok || result.exitCode !== 0) throw new Error(`код ${result.exitCode}`);
    });

    await report.step("дерево установки: файлы списка, install-files.json, маркер владельца", () => {
      for (const item of setup.list.files) {
        const file = path.join(installDir, ...item.path.split("/"));
        if (!fs.existsSync(file)) throw new Error(`нет ${item.path}`);
        if (fs.statSync(file).size !== item.size || sha256Hex(fs.readFileSync(file)) !== item.sha256) throw new Error(`${item.path}: размер или sha256`);
      }
      const list = fs.readFileSync(path.join(installDir, "resources", "install-files.json"));
      if (Buffer.compare(list, setup.listBytes) !== 0) throw new Error("resources/install-files.json отличается от списка в установщике");
      const id = registry(keys.install, "HLSRInstallId");
      if (!/^\{[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}\}$/.test(id || "")) throw new Error(`HLSRInstallId: ${id}`);
      const owner = fs.readFileSync(path.join(installDir, ".hlsr-install-owner.json"), "utf8");
      if (owner !== `{"schemaVersion":1,"appId":"${identity.guid}","installId":"${id}"}`) throw new Error(`маркер владельца: ${owner}`);
      return `${setup.list.files.length} файлов`;
    });

    await report.step("реестр (карта §5.2): InstallLocation, DisplayVersion, строки удаления", () => {
      const expected = [
        [keys.install, "InstallLocation", installDir],
        [keys.install, "KeepShortcuts", "true"],
        [keys.install, "ShortcutName", identity.shortcutName],
        [keys.uninstall, "DisplayVersion", version],
        [keys.uninstall, "DisplayName", `${identity.productName} ${version}`],
        [keys.uninstall, "Publisher", "HLSR Team"],
        [keys.uninstall, "InstallLocation", installDir],
        [keys.uninstall, "UninstallString", `"${exe}" --hlsr-uninstall`],
        [keys.uninstall, "QuietUninstallString", `"${exe}" --hlsr-uninstall /S`],
      ];
      for (const [key, name, want] of expected) {
        const got = registry(key, name);
        if (String(got).toLowerCase() !== want.toLowerCase()) throw new Error(`${name}: ${got} (ожидалось ${want})`);
      }
      if (registry(keys.install, "HLSRSeededInstallLocation") != null) throw new Error("метка незаконченной установки HLSRSeededInstallLocation осталась");
    });

    await report.step("ярлыки .lnk: меню «Пуск» и рабочий стол", () => {
      if (!ctx.devHooks) return { skip: "настоящие папки раннера проверяет CI-сценарий вручную (AUMID, панель задач)" };
      const links = homeLinks();
      if (links.length !== 2) throw new Error(`ярлыков ${links.length}, ожидалось 2`);
      for (const link of links) {
        if (path.basename(link) !== `${identity.shortcutName}.lnk`) throw new Error(`имя ${path.basename(link)}`);
        const target = shortcutTarget(link);
        if (target.toLowerCase() !== exe.toLowerCase()) throw new Error(`${path.basename(link)} → ${target}`);
      }
    });

    await report.step("process-wait.js: перечень процессов (tasklist) и мгновенный возврат без процесса", async () => {
      const imageName = "HLSR-ACCEPT-NO-SUCH-PROCESS.exe";
      if (processWait.listPids(imageName).length) throw new Error("tasklist нашёл несуществующий образ");
      const waited = await processWait.waitForExit({ imageName, timeoutMs: 2000 });
      if (!waited.ok) throw new Error("waitForExit не вернулся");
    });

    await report.step("ProductVersion установленного HLSR.exe (verifyInstalled)", () => {
      const verdict = install.verifyInstalled({ launcherExe: exe, targetVersion: version });
      if (!verdict.ok) throw new Error(`${verdict.code}: ${verdict.detail}`);
      if (verdict.unverified) {
        if (ctx.allowUnversioned) return { skip: "у пробного exe нет VERSIONINFO" };
        throw new Error("версию HLSR.exe прочитать нельзя (нет VERSIONINFO)");
      }
      return `ProductVersion ${verdict.version}`;
    });

    await report.step("--updated /S --force-run без /D=: каталог из реестра, чистое окружение", async () => {
      if (ctx.devHooks) fs.rmSync(dryRun, { force: true });
      const idBefore = registry(keys.install, "HLSRInstallId");
      const result = await install.runInstaller(installer, { args: ["--updated", "/S", "--force-run"], env, timeoutMs: 10 * 60 * 1000, windowsHide: true });
      if (!result.ok || result.exitCode !== 0) throw new Error(`код ${result.exitCode}`);
      if (registry(keys.install, "HLSRInstallId") !== idBefore) throw new Error("HLSRInstallId изменился при обновлении");
      if (!ctx.devHooks) {
        // Раннер: лаунчер стартовал по-настоящему; он должен появиться и сниматься с процессов.
        let pids = [];
        for (let i = 0; i < 30 && !pids.length; i++) {
          pids = processWait.listPids("HLSR.exe");
          if (!pids.length) await new Promise((resolve) => setTimeout(resolve, 500));
        }
        for (const pid of pids) spawnSync("taskkill", ["/F", "/PID", String(pid)]);
        if (!pids.length) throw new Error("--force-run не запустил HLSR.exe");
        return `запущен PID ${pids.join(", ")} (снят)`;
      }
      const spawned = dry().filter((entry) => entry.kind === "spawn");
      if (spawned.length !== 1) throw new Error(`запусков ${spawned.length}, ожидался 1`);
      const [launch] = spawned;
      if (launch.file.toLowerCase() !== exe.toLowerCase() || launch.cwd.toLowerCase() !== installDir.toLowerCase())
        throw new Error(`запуск ${launch.file} в ${launch.cwd}`);
      if (launch.args.length) throw new Error(`аргументы запуска: ${launch.args.join(" ")}`);
      const dirty = Object.keys(launch.env || {}).filter((name) => /^(PORTABLE_EXECUTABLE_|SLINT_|WINIT_|APPIMAGE|APPDIR|ARGV0|OWD)/i.test(name));
      if (dirty.length) throw new Error(`в окружении лаунчера остались ${dirty.join(", ")}`);
      return "HLSR.exe из InstallLocation, окружение без PORTABLE_EXECUTABLE_*, SLINT_*, WINIT_*";
    });

    await report.step("HLSR.exe --hlsr-uninstall /S: код 0, ключи и ярлыки убраны", async () => {
      if (ctx.devHooks) fs.rmSync(dryRun, { force: true });
      const run = spawnSync(exe, ["--hlsr-uninstall", "/S"], { env, encoding: "utf8", timeout: 5 * 60 * 1000, windowsHide: true });
      if (run.status !== 0) throw new Error(`код ${run.status}: ${(run.stderr || "").slice(-300)}`);
      if (registry(keys.install, "InstallLocation") != null || registry(keys.uninstall, "DisplayVersion") != null)
        throw new Error("ключи установки и удаления не убраны");
      if (ctx.devHooks) {
        if (homeLinks().length) throw new Error("ярлыки не убраны");
        const removal = dry().filter((entry) => entry.kind === "spawn" && /powershell/i.test(entry.file));
        if (removal.length !== 1 || removal[0].env.HLSR_REMOVE_DIR !== installDir || String(removal[0].cwd).toLowerCase() !== temp.toLowerCase())
          throw new Error(`команда удаления каталога: ${JSON.stringify(removal).slice(0, 300)}`);
        return "каталог удалит отложенная команда (dry-run: HLSR_REMOVE_DIR, cwd = TEMP)";
      }
      for (let i = 0; i < 60 && fs.existsSync(installDir); i++) await new Promise((resolve) => setTimeout(resolve, 1000));
      if (fs.existsSync(installDir)) throw new Error("каталог установки не удалён за 60 с");
    });
  } finally {
    if (!ctx.keep) fs.rmSync(work, { recursive: true, force: true });
    else report.write(`рабочий каталог оставлен: ${work}`);
  }
}

module.exports = { nativeWindows, registryReader, findFiles };
