"use strict";

// Приёмка, Windows: проверки замороженной обновлялки базы ctx.base (версия у пользователя, канал
// обязан быть новее) набором ctx.set (frozen.json: 0.4.8 → 0.4.7) над артефактами выпуска.
// Чистые проверки идут на любой ОС (пути Windows — модули в копии для win32). Путь Б «на месте»
// (замороженные install.js и process-wait.js запускают установщик) — native-windows.js.
//
// Каталог выпуска: HLSR-Setup-<v>.exe, latest.yml, launcher-channel.json; рядом по желанию
// HLSR-<v>-win32.chunks.json (если в канале есть delta).

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");
const { loadAs } = require("./load-as");
const { readSetup } = require("./setup-payload");

const W = "win32";

const sha256Hex = (data) => crypto.createHash("sha256").update(data).digest("hex");
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const yamlQuote = (value) => `'${String(value).replace(/'/g, "''")}'`;

/** Текст latest.yml по build-windows-setup.js:92-113 / launcher-release-prepare-linux.js:17-20. */
function expectedLatestYml({ version, filename, sha512, size, releaseDate }) {
  return `version: ${version}\nfiles:\n  - url: ${filename}\n    sha512: ${sha512}\n    size: ${size}\n` +
    `path: ${filename}\nsha512: ${sha512}\nreleaseDate: ${yamlQuote(releaseDate)}\n`;
}

function modules(set) {
  return {
    channel: loadAs("main/launcher-update/launcher-channel.js", W, set),
    chunks: loadAs("main/launcher-update/chunks.js", W, set),
    endpoints: loadAs("shared/delivery-endpoints.js", W, set),
    plan: loadAs("updater/core/plan.js", W, set),
    install: loadAs("updater/core/install.js", W, set),
    fileList: loadAs("main/windows-install/file-list.js", W, set),
    layout: loadAs("main/windows-install/layout.js", W, set),
    signing: loadAs("main/game-delivery/manifest-signing.js", W, set),
  };
}

/** ProductVersion установщика: hlsr-release (разбор PE, любая ОС) и PowerShell 0.4.7 (Windows). */
function productVersions(installer, ctx, m) {
  const found = {};
  if (ctx.hlsrRelease) {
    const run = spawnSync(ctx.hlsrRelease, ["exe-version", installer], { encoding: "utf8" });
    if (run.status === 0) found.pe = run.stdout.trim();
    else if (!/нет ProductVersion/.test(run.stderr || "")) throw new Error(`hlsr-release exe-version: ${(run.stderr || run.error || "").toString().trim()}`);
  }
  if (process.platform === "win32") {
    const powershell = m.install.readExeProductVersion(installer);
    if (powershell) found.powershell = powershell;
  }
  return found;
}

async function acceptWindows(ctx, report) {
  const m = modules(ctx.set);
  const CURRENT = ctx.base;
  const dir = ctx.dir;
  report.begin(`Windows, база ${CURRENT} (набор ${ctx.set}): ${dir}`);

  const channelFile = path.join(dir, "launcher-channel.json");
  let manifest = null;
  await report.step("канал: launcher-channel.json читается", () => {
    manifest = readJson(channelFile);
    return `версия ${manifest.version}`;
  });
  if (!manifest) return;
  const version = ctx.version || manifest.version;
  const installerName = `HLSR-Setup-${version}.exe`;
  const installer = path.join(dir, installerName);
  const publicKeys = ctx.publicKeys || undefined;
  const checkOptions = { currentVersion: CURRENT, publicKeys, platform: W };

  await report.step(`канал: форма и подпись (launcher-channel.js ${ctx.set})`, () => {
    const shape = m.channel.validateShape(manifest, W);
    if (shape.length) throw new Error(`форма: ${shape.join("; ")}`);
    const verdict = m.channel.check(manifest, checkOptions);
    if (!verdict.ok) throw new Error(verdict.problems.join("; "));
    if (!verdict.updateAvailable) throw new Error(`канал ${manifest.version} не новее ${CURRENT}: ${CURRENT} не предложит обновление`);
    const keyId = manifest.signature && manifest.signature.keyId;
    return `подписано ключом ${keyId}`;
  });

  await report.step("версия выпуска — только X.Y.Z (Р9)", () => {
    if (!/^\d+\.\d+\.\d+$/.test(manifest.version)) throw new Error(`версия ${manifest.version}`);
    if (manifest.version !== version) throw new Error(`канал ${manifest.version}, ожидалась ${version}`);
    return manifest.version;
  });

  await report.step("установщик: имя, размер, ровно два адреса", () => {
    const entry = manifest.installer;
    if (entry.filename !== installerName) throw new Error(`канал описывает ${entry.filename}, ожидался ${installerName}`);
    if (!fs.existsSync(installer)) throw new Error(`нет файла ${installer}`);
    const size = fs.statSync(installer).size;
    if (size !== entry.size) throw new Error(`размер ${size} != ${entry.size} из канала`);
    if (size > m.channel.MAX_INSTALLER_BYTES) throw new Error(`${size} байт больше 512 МиБ`);
    const urls = m.endpoints.launcherFor(W).installerUrls(version, installerName);
    if (JSON.stringify(entry.urls) !== JSON.stringify(urls)) throw new Error(`urls ${JSON.stringify(entry.urls)} != ${JSON.stringify(urls)}`);
    if (urls.length !== 2) throw new Error("адресов не два");
    return `${(size / 1048576).toFixed(1)} MiB`;
  });

  await report.step("установщик: размер, sha256 и sha512 (verifyInstallerStream)", async () => {
    const result = await m.channel.verifyInstallerStream(installer, manifest.installer);
    if (!result.ok) throw new Error(result.problems.join("; "));
  });

  await report.step("latest.yml: байт в байт (§2.6) и latestYmlSha256", () => {
    const file = path.join(dir, "latest.yml");
    const bytes = fs.readFileSync(file);
    const text = bytes.toString("utf8");
    const date = /releaseDate: '([^']*)'\n$/.exec(text);
    if (!date) throw new Error("нет releaseDate в конце файла");
    const expected = expectedLatestYml({
      version, filename: installerName, sha512: manifest.installer.sha512, size: manifest.installer.size, releaseDate: date[1],
    });
    if (text !== expected) throw new Error(`текст отличается от шаблона ${ctx.set}:\n${text}`);
    if (!Number.isFinite(Date.parse(date[1])) || new Date(date[1]).toISOString() !== date[1]) throw new Error(`releaseDate ${date[1]} — не toISOString()`);
    if (sha256Hex(bytes) !== manifest.latestYmlSha256) throw new Error("sha256 latest.yml не совпал с latestYmlSha256 канала");
    return `sha256 ${manifest.latestYmlSha256.slice(0, 12)}…`;
  });

  await report.step("путь Б: plan.validate принимает выпуск (updater/core/plan.js)", () => {
    const raw = (installDir) => ({
      manifest, currentVersion: CURRENT, targetVersion: version, installDir,
      launcherExe: `${installDir}\\HLSR.exe`, receiptFile: "C:\\Users\\user\\AppData\\Roaming\\hlsr\\version-state.json", language: "ru",
    });
    const plan = m.plan.validate(raw("C:\\HLSR\\Launcher"), { publicKeys });
    if (plan.installer.filename !== installerName) throw new Error(`план: ${plan.installer.filename}`);
    const args = m.install.installerArgsFor("C:\\HLSR\\Launcher");
    if (args.join(" ") !== "/S --updated /D=C:\\HLSR\\Launcher") throw new Error(`аргументы установщика: ${args.join(" ")}`);
    // Путь с пробелом: /D= не передаётся (0.4.7), каталог установщик берёт из реестра.
    if (m.install.installerArgsFor("C:\\HLSR Games\\Launcher").join(" ") !== "/S --updated") throw new Error("путь с пробелом получил /D=");
  });

  await report.step("путь Б: D:\\Games\\Launcher без сегмента HLSR — UPD_BAD_PLAN (карта §0.5)", () => {
    try {
      m.plan.validate({
        manifest, currentVersion: CURRENT, targetVersion: version, installDir: "D:\\Games\\Launcher",
        launcherExe: "D:\\Games\\Launcher\\HLSR.exe", receiptFile: "C:\\Users\\user\\AppData\\Roaming\\hlsr\\version-state.json",
      }, { publicKeys });
    } catch (error) {
      if (error.code === "UPD_BAD_PLAN") return `отказ, как и должен быть у ${CURRENT} (исправить нельзя)`;
      throw error;
    }
    throw new Error(`${CURRENT} приняла каталог без сегмента HLSR`);
  });

  await report.step("delta: блок и манифест кусков (deltaProblems / problemsOf)", () => {
    if (!manifest.delta) return { skip: "в канале нет delta (--no-chunks)" };
    const problems = m.chunks.deltaProblems(manifest.delta, m.channel.isAllowedUrl);
    if (problems.length) throw new Error(problems.join("; "));
    const file = path.join(dir, manifest.delta.manifest.filename);
    if (!fs.existsSync(file)) return `блок годен; манифест ${manifest.delta.manifest.filename} рядом не лежит`;
    const bytes = fs.readFileSync(file);
    if (bytes.length !== manifest.delta.manifest.size || sha256Hex(bytes) !== manifest.delta.manifest.sha256)
      throw new Error("размер или sha256 манифеста кусков не совпал с delta");
    const chunkManifest = JSON.parse(bytes.toString("utf8"));
    const bad = m.chunks.problemsOf(chunkManifest, { version, platform: W });
    if (bad.length) throw new Error(bad.join("; "));
    const exe = chunkManifest.files.find((item) => item.path === "HLSR.exe");
    if (!exe) throw new Error("в манифесте кусков нет HLSR.exe");
    if (!chunkManifest.files.some((item) => item.path === "resources/install-files.json")) throw new Error("в манифесте кусков нет resources/install-files.json: путь В не соберёт установщик");
    if (manifest.build && manifest.build.executable.sha256 !== exe.sha256) throw new Error("build.executable.sha256 != HLSR.exe в манифесте кусков");
    return `${chunkManifest.files.length} файлов`;
  });

  await report.step("блок build: присутствует и покрыт подписью (Р3)", () => {
    const build = manifest.build;
    if (!build || typeof build !== "object") throw new Error("в канале нет блока build");
    const exe = build.executable || {};
    if (exe.path !== "HLSR.exe" || !Number.isSafeInteger(exe.size) || exe.size <= 0 || !/^[a-f0-9]{64}$/.test(exe.sha256 || "") || !Array.isArray(exe.chunks) || !exe.chunks.length)
      throw new Error(`build.executable: ${JSON.stringify(exe).slice(0, 200)}`);
    const total = exe.chunks.reduce((sum, chunk) => sum + chunk[1], 0);
    if (total !== exe.size) throw new Error(`куски build.executable дают ${total} байт, размер ${exe.size}`);
    if (build.attest !== undefined && (build.attest.v !== 1 || !/^[a-f0-9]{64}$/.test(build.attest.publicKey || "")))
      throw new Error(`build.attest: ${JSON.stringify(build.attest)}`);
    // Покрытие подписью: подмена любого байта блока ломает подпись, а канал при этом остаётся годным по форме.
    const forged = JSON.parse(JSON.stringify(manifest));
    forged.build.executable.sha256 = (exe.sha256[0] === "0" ? "1" : "0") + exe.sha256.slice(1);
    const verdict = m.channel.check(forged, checkOptions);
    if (verdict.ok) throw new Error("подмена build.executable.sha256 не ломает подпись");
    return build.attest ? "executable + attest" : "executable (без аттестации)";
  });

  let setup = null;
  await report.step("нагрузка установщика: хвост, sha256, install-files.json, файлы", () => {
    setup = readSetup(installer, { problemsOf: m.fileList.problemsOf, set: ctx.set });
    if (setup.list.version !== version) throw new Error(`install-files.json описывает ${setup.list.version}`);
    const expected = ctx.expectQa ? "com.hlsrteam.hlsr.qa" : "com.hlsrteam.hlsr";
    if (setup.list.app.appId !== expected) throw new Error(`appId ${setup.list.app.appId}, ожидался ${expected}`);
    const identity = m.layout.identity(setup.list.app);
    if (!ctx.expectQa && identity.guid !== "3f824379-6b41-5725-b424-59cb87f2acae") throw new Error(`GUID ${identity.guid}`);
    return `${setup.files.length} файлов, GUID ${identity.guid}`;
  });

  await report.step("build.executable совпал с HLSR.exe из установщика", () => {
    if (!setup) throw new Error("нагрузка не прочитана");
    if (!manifest.build) throw new Error("нет блока build");
    if (manifest.build.executable.sha256 !== setup.exeSha256 || manifest.build.executable.size !== setup.exeLength)
      throw new Error("sha256 или размер HLSR.exe в build не совпали с первыми N байтами установщика");
  });

  await report.step(`ProductVersion HLSR.exe — версия выпуска (verifyInstalled ${ctx.set})`, () => {
    const found = productVersions(installer, ctx, m);
    const values = Object.entries(found);
    if (!values.length) {
      if (ctx.allowUnversioned) return { skip: "у пробного exe нет VERSIONINFO (--allow-unversioned): проверит сборка hlsr-app" };
      if (process.platform !== "win32" && !ctx.hlsrRelease) return { skip: "нужен hlsr-release (--hlsr-release) или Windows с PowerShell" };
      throw new Error("у HLSR.exe нет ProductVersion: verifyInstalled ${ctx.set} не сможет подтвердить версию");
    }
    for (const [source, value] of values)
      if (m.install.versionTriple(value) !== version) throw new Error(`${source}: ProductVersion ${value}, версия выпуска ${version}`);
    const verdict = m.install.verifyInstalled({ launcherExe: installer, targetVersion: version, readVersion: () => values[0][1] });
    if (!verdict.ok) throw new Error(`${verdict.code}: ${verdict.detail}`);
    return values.map(([source, value]) => `${source} ${value}`).join(", ");
  });

  return { manifest, version, installer, setup };
}

module.exports = { acceptWindows, expectedLatestYml, modules, sha256Hex, readJson };
