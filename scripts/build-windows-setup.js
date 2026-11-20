"use strict";

// Windows-установщик HLSR: тот же установщик, что в Linux, без NSIS-мастера.
//
//   1. electron-builder --dir собирает <output>/win-unpacked (сам лаунчер;
//      апдейтер уже лежит в app.asar, его собирает .electron-vue/build.js);
//   2. в resources кладутся app-update.yml (для electron-updater; NSIS-цель
//      писала его сама) и install-files.json — список файлов с SHA-256, по
//      которому установщик сверяет копию, и appId сборки;
//   3. electron-builder --prepackaged упаковывает win-unpacked в portable-exe
//      HLSR-Setup-<версия>.exe: при запуске он распаковывает лаунчер во
//      временный каталог, а тот открывает окно установщика
//      (src/main/windows-install/setup.js);
//   4. рядом пишутся blockmap и latest.yml — их ждут electron-updater
//      прежних версий и релизный workflow.
//
// portable.unpackDirName: true в package.json — не опечатка. Строка или
// false (вопреки документации electron-builder 26) дают один каталог
// %TEMP%\<имя> на все запуски одного exe, и второй запуск стирает файлы
// первого. Только true оставляет распаковку в $PLUGINSDIR, свой на каждый
// запуск.
//
// Аргументы: --dir — только win-unpacked; -c.<ключ>=<значение> уходят в
// electron-builder (так собирается QA-сборка, scripts/qa-launcher-build.js).
// Webpack-сборка (node .electron-vue/build.js) должна быть выполнена заранее.
const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");
const fileList = require("../src/main/windows-install/file-list");

const root = path.resolve(__dirname, "..");
const pkg = require("../package.json");

function parseArgs(argv) {
  const overrides = argv.filter((arg) => arg.startsWith("-c."));
  const unknown = argv.filter((arg) => !arg.startsWith("-c.") && arg !== "--dir");
  if (unknown.length) throw new Error(`Неизвестные аргументы: ${unknown.join(" ")}`);
  const override = (key) => {
    const found = overrides.filter((arg) => arg.startsWith(`-c.${key}=`)).pop();
    return found ? found.slice(key.length + 4) : null;
  };
  const output = path.resolve(root, override("directories.output") || pkg.build.directories.output);
  const version = override("extraMetadata.version") || pkg.version;
  const name = override("extraMetadata.name") || pkg.name;
  const setupName = `HLSR-Setup-${version}.exe`;
  return {
    dirOnly: argv.includes("--dir"),
    overrides,
    version,
    name,
    app: {
      appId: override("appId") || pkg.build.appId,
      productName: override("productName") || pkg.build.productName,
    },
    output,
    unpacked: path.join(output, "win-unpacked"),
    setupName,
    setup: path.join(output, setupName),
  };
}

function builder(args) {
  const bin = path.join(root, "node_modules", ".bin", process.platform === "win32" ? "electron-builder.cmd" : "electron-builder");
  execFileSync(bin, args, { cwd: root, stdio: "inherit", shell: process.platform === "win32" });
}

// Кеш electron-updater в %LOCALAPPDATA%: свой у каждого имени приложения,
// чтобы QA-сборка (hlsr-qa) не делила его с боевым HLSR (hlsr-updater).
function appUpdateYml(name = pkg.name) {
  const publish = pkg.build.win.publish[0];
  return [
    `owner: ${publish.owner}`,
    `repo: ${publish.repo}`,
    `provider: ${publish.provider}`,
    `releaseType: ${publish.releaseType}`,
    `updaterCacheDirName: ${name}-updater`,
    "",
  ].join("\n");
}

function finalizeUnpacked(options) {
  const resources = path.join(options.unpacked, "resources");
  fs.writeFileSync(path.join(resources, "app-update.yml"), appUpdateYml(options.name));
  const list = fileList.build(options.unpacked, options.version, options.app);
  const problems = fileList.problemsOf(list);
  if (problems.length) throw new Error(`Список файлов сборки некорректен: ${problems.join("; ")}`);
  fs.writeFileSync(path.join(options.unpacked, fileList.RELATIVE), JSON.stringify(list) + "\n");
  const bytes = list.files.reduce((sum, item) => sum + item.size, 0);
  console.log(`install-files.json: ${list.files.length} файлов, ${(bytes / 1048576).toFixed(1)} MiB, ${list.app.appId}`);
}

function yamlQuote(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

async function writeUpdateInfo(options) {
  // app-builder-lib не прямая зависимость: npm кладёт его внутрь electron-builder,
  // поэтому ищем модуль от него, а не из корня node_modules.
  const blockmapModule = require.resolve("app-builder-lib/out/targets/blockmap/blockmap", {
    paths: [path.dirname(require.resolve("electron-builder/package.json"))]
  });
  const { buildBlockMap } = require(blockmapModule);
  const info = await buildBlockMap(options.setup, "gzip", `${options.setup}.blockmap`);
  const yml = [
    `version: ${options.version}`,
    "files:",
    `  - url: ${options.setupName}`,
    `    sha512: ${info.sha512}`,
    `    size: ${info.size}`,
    `path: ${options.setupName}`,
    `sha512: ${info.sha512}`,
    `releaseDate: ${yamlQuote(new Date().toISOString())}`,
    "",
  ].join("\n");
  fs.writeFileSync(path.join(options.output, "latest.yml"), yml);
  console.log(`latest.yml: ${options.setupName}, ${(info.size / 1048576).toFixed(1)} MiB`);
}

async function main(argv) {
  const options = parseArgs(argv);
  execFileSync(process.execPath, [path.join(root, "scripts", "prepare-installer-assets.js")], { stdio: "inherit" });
  fs.rmSync(options.unpacked, { recursive: true, force: true });
  builder(["--win", "--x64", "--dir", "--publish", "never", ...options.overrides]);
  finalizeUnpacked(options);
  if (options.dirOnly) return;
  fs.rmSync(options.setup, { force: true });
  builder(["--win", "portable", "--x64", "--prepackaged", options.unpacked, "--publish", "never", ...options.overrides]);
  if (!fs.existsSync(options.setup) || !fs.statSync(options.setup).size) throw new Error(`Нет установщика ${options.setup}`);
  await writeUpdateInfo(options);
}

if (require.main === module)
  main(process.argv.slice(2)).catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });

module.exports = { parseArgs, appUpdateYml, finalizeUnpacked };
