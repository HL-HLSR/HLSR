#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const EXPECTED_BY_PLATFORM = {
  win32: {
    provider: "github",
    owner: "HL-HLSR",
    repo: "HLSR-Releases",
    releaseType: "release",
  },
  linux: {
    provider: "generic",
    url: require("../src/shared/delivery-endpoints").launcherFor("linux").feedUrls[0],
    updaterCacheDirName: "hlsr-linux-updater",
  },
};

const EXPECTED = EXPECTED_BY_PLATFORM.win32;

// Платформа берётся из --platform или из имени каталога сборки
// (win-unpacked / linux-unpacked), иначе — ОС, на которой идёт проверка.
function platformOf(args) {
  if (args.platform) return args.platform === "linux" ? "linux" : "win32";
  if (args.unpacked && /linux-unpacked[\\/]?$/.test(args.unpacked)) return "linux";
  if (args.unpacked && /win-unpacked[\\/]?$/.test(args.unpacked)) return "win32";
  return process.platform === "linux" ? "linux" : "win32";
}

const SOURCE_REPO = "HLSR";

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--unpacked") args.unpacked = argv[++i];
    else if (arg === "--platform") args.platform = argv[++i];
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function parseFlatYaml(text) {
  const out = {};

  for (const line of String(text).split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;

    const index = trimmed.indexOf(":");
    if (index === -1) continue;

    const key = trimmed.slice(0, index).trim();
    let value = trimmed.slice(index + 1).trim();
    if (/^["'].*["']$/.test(value)) value = value.slice(1, -1);
    out[key] = value;
  }

  return out;
}

function check(text, platform) {
  const data = parseFlatYaml(text);
  const problems = [];
  const expectedFields = EXPECTED_BY_PLATFORM[platform === "linux" ? "linux" : "win32"];

  for (const [key, expected] of Object.entries(expectedFields))
    if (data[key] !== expected)
      problems.push(`${key}: ожидалось "${expected}", получено "${data[key]}"`);

  if (data.repo === SOURCE_REPO)
    problems.push("feed указывает на ИСХОДНЫЙ репозиторий: обновления сломаются после private");

  if (!data.updaterCacheDirName) problems.push("не задан updaterCacheDirName");

  return { data, problems };
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || !args.unpacked) {
    process.stdout.write(
      "node scripts/verify-app-update-yml.js --unpacked <каталог win-unpacked|linux-unpacked> [--platform win32|linux]\n"
    );
    return args.help ? 0 : 2;
  }

  const file = path.join(args.unpacked, "resources", "app-update.yml");
  if (!fs.existsSync(file)) throw new Error(`Файл не найден: ${file}`);

  const { data, problems } = check(fs.readFileSync(file, "utf8"), platformOf(args));

  process.stdout.write(`Файл: ${file}\n`);
  for (const [key, value] of Object.entries(data)) process.stdout.write(`  ${key}: ${value}\n`);

  if (problems.length) {
    process.stderr.write("\nПроблемы feed обновления:\n");
    for (const problem of problems) process.stderr.write(`  - ${problem}\n`);
    return 1;
  }

  process.stdout.write("\napp-update.yml корректен: бинарный канал, не исходный репозиторий\n");
  return 0;
}

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`Проверка не выполнена: ${error.message}\n`);
    process.exitCode = 2;
  }
}

module.exports = { EXPECTED, EXPECTED_BY_PLATFORM, parseFlatYaml, check };
