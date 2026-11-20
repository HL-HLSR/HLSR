#!/usr/bin/env node
"use strict";

// Приёмка (подпроект 6, задача 8; GPUI U-d): готовые артефакты выпуска проходят проверки
// замороженной обновлялки каждой базы — версии, которая стоит у пользователей, — как есть.
// Выпуск, не прошедший приёмку, не публикуется.
//
//   node tools/compat-047/accept-release.js --dist <каталог> [--platform win32|linux|both]
//        [--base 0.4.8,0.4.9] [--version X.Y.Z] [--test-keys <json|pem>] [--hlsr-release <hlsr-release.exe>]
//        [--native-windows] [--dev-hooks] [--expect-qa] [--allow-unversioned] [--smoke] [--keep]
//
//  --dist            target/dist (внутри windows/ и linux/) или сам каталог выпуска одной платформы:
//                    HLSR-Setup-<v>.exe (HLSR-Setup-<v>-linux-x86_64.AppImage), latest*.yml,
//                    launcher-channel.json; payload.json и архив рядом с AppImage — по желанию.
//  --base            версии у пользователей через запятую (по умолчанию 0.4.8,0.4.9). Каждая
//                    проверяется своим набором копий (frozen.json: 0.4.7 и 0.4.8 — набор 0.4.7,
//                    0.4.9 — набор 0.4.9), канал обязан быть новее каждой.
//  --test-keys       подмена trust root: JSON {"publicKeys": [pem, ...]} или файл PEM. Без неё канал
//                    проверяется вшитыми ключами лаунчера базы (боевой выпуск). Только тестовые
//                    артефакты, подписанные тестовым ключом.
//  --native-windows  путь Б без Electron: замороженные install.js и process-wait.js запускают
//                    установщик. Только Windows; без --dev-hooks — только на одноразовом раннере CI
//                    (GITHUB_ACTIONS=true), с --dev-hooks — на сборке dev-hooks с перенаправлениями
//                    (реестр — файл JSON, ярлыки и данные во временном каталоге).
//  --smoke           Linux: запустить <версия>/hlsr --no-sandbox под scripts/slint/headless.sh.
//
// Коды выхода: 0 — всё прошло (пропуски перечислены с причинами), 1 — есть неудачи, 2 — неверные
// аргументы. Linux-часть с настоящим рантаймом AppImage идёт только на Linux; на другой ОС её шаги
// получают skip с причиной, а не молчат.

const fs = require("fs");
const path = require("path");
const { Report } = require("./lib/report");
const { acceptWindows } = require("./lib/accept-windows");
const { acceptLinux } = require("./lib/accept-linux");
const { nativeWindows } = require("./lib/native-windows");
const { setForBase } = require("./freeze");

function parseArgs(argv) {
  const args = { platform: "both", bases: ["0.4.8", "0.4.9"] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`У ${arg} нет значения`);
      return argv[++i];
    };
    if (arg === "--dist") args.dist = value();
    else if (arg === "--platform") args.platform = value();
    else if (arg === "--base") args.bases = value().split(",").map((item) => item.trim()).filter(Boolean);
    else if (arg === "--version") args.version = value();
    else if (arg === "--test-keys") args.testKeys = value();
    else if (arg === "--hlsr-release") args.hlsrRelease = value();
    else if (arg === "--native-windows") args.nativeWindows = true;
    else if (arg === "--dev-hooks") args.devHooks = true;
    else if (arg === "--expect-qa") args.expectQa = true;
    else if (arg === "--allow-unversioned") args.allowUnversioned = true;
    else if (arg === "--smoke") args.smoke = true;
    else if (arg === "--keep") args.keep = true;
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  if (!["win32", "linux", "both"].includes(args.platform)) throw new Error(`--platform: win32, linux или both, получено ${args.platform}`);
  if (!args.bases.length) throw new Error("--base: нужна хотя бы одна версия");
  args.bases.forEach(setForBase); // неизвестная база — ошибка аргументов
  return args;
}

/** Тестовые ключи доверия: JSON {publicKeys:[pem]} или один PEM. */
function readTestKeys(file) {
  const text = fs.readFileSync(file, "utf8");
  if (/^\s*-----BEGIN /.test(text)) return [text];
  const data = JSON.parse(text);
  const keys = Array.isArray(data) ? data : data.publicKeys;
  if (!Array.isArray(keys) || !keys.length || keys.some((key) => typeof key !== "string" || !key.includes("BEGIN PUBLIC KEY")))
    throw new Error(`${file}: ожидался {"publicKeys": [<PEM>, ...]}`);
  return keys;
}

/** Каталог выпуска платформы: сам --dist, если в нём канал, иначе <dist>/<windows|linux>. */
function releaseDir(dist, name) {
  if (fs.existsSync(path.join(dist, "launcher-channel.json"))) return dist;
  return path.join(dist, name);
}

async function main(argv, write) {
  const args = parseArgs(argv);
  if (args.help || !args.dist) {
    (write || ((line) => process.stdout.write(`${line}\n`)))(
      "node tools/compat-047/accept-release.js --dist <каталог> [--platform win32|linux|both] [--base 0.4.8,0.4.9] [--version X.Y.Z] " +
        "[--test-keys <json|pem>] [--hlsr-release <bin>] [--native-windows] [--dev-hooks] [--expect-qa] " +
        "[--allow-unversioned] [--smoke] [--keep]"
    );
    return args.help ? 0 : 2;
  }

  const dist = path.resolve(args.dist);
  const report = new Report(write);
  const common = {
    version: args.version || null,
    publicKeys: args.testKeys ? readTestKeys(args.testKeys) : null,
    expectQa: !!args.expectQa,
    allowUnversioned: !!args.allowUnversioned,
    hlsrRelease: args.hlsrRelease ? path.resolve(args.hlsrRelease) : null,
    devHooks: !!args.devHooks,
    smoke: !!args.smoke,
    keep: !!args.keep,
  };
  if (common.publicKeys) report.write(`ВНИМАНИЕ: --test-keys подменяет trust root лаунчера ${args.bases.join(", ")}. Допустимо ТОЛЬКО для тестовых артефактов.`);

  for (const base of args.bases) {
    const own = { ...common, base, set: setForBase(base) };
    // Сбой одной базы (например, набор копий не загрузился) — неудача в отчёте, остальные базы идут дальше.
    try {
      if (args.platform !== "linux") {
        const ctx = { ...own, dir: releaseDir(dist, "windows") };
        const result = await acceptWindows(ctx, report);
        if (args.nativeWindows) {
          if (result) await nativeWindows(ctx, report, result);
          else report.fail("путь Б на месте", "артефакты Windows не прочитаны");
        }
      }
      if (args.platform !== "win32") await acceptLinux({ ...own, dir: releaseDir(dist, "linux") }, report);
    } catch (error) {
      report.fail(`база ${base} (набор ${own.set})`, error.message);
    }
  }

  report.summary();
  return report.failed.length ? 1 : 0;
}

module.exports = { main, parseArgs, readTestKeys, releaseDir };

if (require.main === module) {
  main(process.argv.slice(2)).then(
    (code) => { process.exitCode = code; },
    (error) => { process.stderr.write(`приёмка не выполнена: ${error.message}\n`); process.exitCode = 2; }
  );
}
