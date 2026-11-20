#!/usr/bin/env node
"use strict";

// Аналог scripts/qa-launcher-build.js 0.4.7 (подпроект 6, задача 8, решение 18) для лаунчера на Rust.
// QA-сборка встаёт рядом с боевой и не трогает её: свой appId (com.hlsrteam.hlsr.qa), имя HLSR-QA,
// свой GUID реестра и каталог C:\HLSR-QA\Launcher — всё это `hlsr-release setup --qa`.
//
//   node tools/release/node/qa-launcher-build.js [--version x.y.z] [--out-root build-qa]
//        [--exe <HLSR.exe>] [--skip-bundle]
//
// Без --skip-bundle собирается `hlsr.exe` с признаком dev-hooks (HLSR_BUILD_CHANNEL=qa: локальная
// лента HLSR_UPDATE_FEED и ключ HLSR_UPDATE_CHANNEL_KEY работают только в такой сборке). Версия
// QA-сборки — версия самого exe (ProductVersion из Cargo.toml): другая не пройдёт проверку
// апдейтера 0.4.7 после установки. Результат — <out-root>/<версия>/HLSR-Setup-<версия>.exe,
// latest.yml, win-unpacked. Ничего не публикуется.

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");
const common = require("./lib/release-common");

const REPO_ROOT = common.REPO_ROOT;

const QA_APP_ID = "com.hlsrteam.hlsr.qa";
const QA_NAME = "hlsr-qa";
const QA_PRODUCT = "HLSR-QA";

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--version") args.version = argv[++i];
    else if (arg === "--out-root") args.outRoot = argv[++i];
    else if (arg === "--exe") args.exe = argv[++i];
    else if (arg === "--skip-bundle") args.skipBundle = true;
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function run(command, args, env) {
  process.stdout.write(`\n> ${command} ${args.join(" ")}\n`);

  const result = spawnSync(command, args, {
    cwd: REPO_ROOT,
    stdio: "inherit",
    env: Object.assign({}, process.env, env || {}),
  });

  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} завершился с кодом ${result.status}`);
}

// Упаковщик: готовый бинарник (HLSR_RELEASE_BIN) или `cargo run -p hlsr-release`.
function releaseTool() {
  if (process.env.HLSR_RELEASE_BIN) return [process.env.HLSR_RELEASE_BIN, []];
  return ["cargo", ["run", "--quiet", "--release", "-p", "hlsr-release", "--"]];
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help) {
    process.stdout.write(
      "node tools/release/node/qa-launcher-build.js [--version <x.y.z>] [--out-root build-qa] [--exe <HLSR.exe>] [--skip-bundle]\n"
    );
    return 0;
  }

  const version = common.requireReleaseVersion(args.version || common.workspaceVersion());
  if (version !== common.workspaceVersion())
    throw new Error(
      `Версия QA-сборки ${version} не совпадает с версией проекта ${common.workspaceVersion()}: ` +
        "ProductVersion exe берётся из Cargo.toml, и апдейтер 0.4.7 отвергнет установку"
    );

  const outDir = path.resolve(REPO_ROOT, args.outRoot || "build-qa", version);
  let exe = args.exe ? path.resolve(args.exe) : null;

  if (!args.skipBundle) {
    // Только Windows: build.rs GPUI собирает шейдеры fxc.exe из Windows SDK, кросс-сборка (cargo-xwin)
    // их не соберёт. На другой ОС — готовый exe через --skip-bundle --exe.
    if (process.platform !== "win32")
      throw new Error("QA-сборка Windows собирается только на Windows (или --skip-bundle --exe <hlsr.exe>)");
    run("cargo", ["build", "-p", "hlsr-app", "--release", "--features", "dev-hooks"], { HLSR_BUILD_CHANNEL: "qa" });
    exe = exe || path.join(REPO_ROOT, "target", "release", "hlsr.exe");
  }
  if (!exe || !fs.existsSync(exe)) throw new Error("Нет hlsr.exe: соберите без --skip-bundle или укажите --exe");

  const [tool, prefix] = releaseTool();
  const resources = path.join(outDir, "resources-src");
  fs.rmSync(resources, { recursive: true, force: true });
  run(tool, [...prefix, "stage", "--platform", "windows", "--repo", REPO_ROOT, "--out", resources]);
  // Установщик берёт ключи реестра, каталог (C:\HLSR-QA\Launcher) и ярлык из appId и имени
  // продукта сборки — QA-сборка не встаёт поверх HLSR.
  run(tool, [...prefix, "setup", "--qa", "--exe", exe, "--resources", resources, "--version", version, "--out", outDir]);

  process.stdout.write(
    `\nQA-сборка ${version} (${QA_APP_ID}, ${QA_PRODUCT}) готова: ${outDir}\n` +
      "Боевой target/dist не изменялся, Cargo.toml не менялся.\n"
  );

  return 0;
}

module.exports = { QA_APP_ID, QA_NAME, QA_PRODUCT, parseArgs };

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`QA-сборка не выполнена: ${error.message}\n`);
    process.exitCode = 2;
  }
}
