#!/usr/bin/env node
"use strict";

const path = require("path");
const { spawnSync } = require("child_process");

const REPO_ROOT = path.resolve(__dirname, "..");

const QA_APP_ID = "com.hlsrteam.hlsr.qa";
const QA_NAME = "hlsr-qa";
const QA_PRODUCT = "HLSR-QA";

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--version") args.version = argv[++i];
    else if (arg === "--out-root") args.outRoot = argv[++i];
    else if (arg === "--skip-bundle") args.skipBundle = true;
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function run(command, args, env) {
  process.stdout.write(`\n> ${command} ${args.join(" ")}\n`);

  const quoted = args.map((arg) => (/[\s${}]/.test(arg) ? `"${arg}"` : arg));

  const result = spawnSync(command, quoted, {
    cwd: REPO_ROOT,
    stdio: "inherit",
    shell: process.platform === "win32",
    env: Object.assign({}, process.env, env || {}),
  });

  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} завершился с кодом ${result.status}`);
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || !args.version) {
    process.stdout.write(
      "node scripts/qa-launcher-build.js --version <x.y.z> [--skip-bundle]\n"
    );
    return args.help ? 0 : 2;
  }

  if (!/^\d+\.\d+\.\d+$/.test(args.version))
    throw new Error(`Версия не SemVer: ${args.version}`);

  const outDir = path.join(args.outRoot || "build-qa", args.version);

  if (!args.skipBundle)
    run("node", [".electron-vue/build.js"], {
      NODE_ENV: "production",
      HLSR_BUILD_CHANNEL: "qa",
      HLSR_TARGET_PLATFORM: "win32",
    });

  // Установщик берёт ключи реестра, каталог (C:\HLSR-QA\Launcher) и ярлык
  // из appId и имени продукта сборки — QA-сборка не встаёт поверх HLSR.
  run(
    "node",
    [
      "scripts/build-windows-setup.js",
      `-c.extraMetadata.version=${args.version}`,
      `-c.extraMetadata.name=${QA_NAME}`,
      `-c.appId=${QA_APP_ID}`,
      `-c.productName=${QA_PRODUCT}`,
      `-c.directories.output=${outDir}`,
    ],
    { NODE_ENV: "production", HLSR_BUILD_CHANNEL: "qa" }
  );

  process.stdout.write(
    `\nQA-сборка ${args.version} готова: ${path.join(REPO_ROOT, outDir)}\n` +
      "Боевой build/ не изменялся, package.json не менялся.\n"
  );

  return 0;
}

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`QA-сборка не выполнена: ${error.message}\n`);
    process.exitCode = 2;
  }
}

module.exports = { QA_APP_ID, QA_NAME, QA_PRODUCT };
