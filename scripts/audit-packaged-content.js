#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const REPO_ROOT = path.resolve(__dirname, "..");

const COMMON_REQUIRED = [
  "resources/app.asar",
  "resources/licenses/HLSR-LICENSE.txt",
  "resources/licenses/LICENSE_en.txt",
  "resources/licenses/LICENSE_ru.txt",
  "resources/licenses/THIRD-PARTY-NOTICES.txt",
  "resources/licenses/SOURCE-SDK-LICENSE.txt",
  "resources/licenses/thirdpartylegalnotices.txt",
  // Оверлей LiveSplit в окне игры: прокси нужны обеим системам, помощники
  // снимка — только Linux (LINUX_ONLY_OVERLAY).
  "resources/livesplit-overlay/proxy-d3d9.dll",
  "resources/livesplit-overlay/proxy-opengl32.dll",
  // Автосплиттер HL1 (BunnySplit), лаунчер кладёт его в LiveSplit сам.
  "resources/livesplit-components/LiveSplit.BunnySplit.dll",
  "resources/livesplit-components/BunnySplit-LICENSE.txt",
];

const WINDOWS_REQUIRED = [
  // Свой 7-Zip (src/main/seven-zip.js), а не 21.07 из 7zip-bin.
  "resources/7zip/7za.exe",
  "resources/7zip/License.txt",
  // Помощник рендера демо (src/main/demo-tool/render-host.js): скрытый стол и объект задания.
  "resources/demo-render/render-host.ps1",
];

// Только Wine: hlsr-overlay64.exe внедряет hlsr-share64.dll в LiveSplit. В
// Windows-сборке они не используются, а внедряющий exe без подписи
// Defender уже принимал за троян (Trojan:Win32/Yomal!rfn у прежнего
// hlsr-overlay.exe), поэтому в Windows-пакет они не попадают.
const LINUX_ONLY_OVERLAY = [
  "resources/livesplit-overlay/hlsr-overlay64.exe",
  "resources/livesplit-overlay/hlsr-share64.dll",
];

// Антивирусы блокировали его в папке распаковки, и установка останавливалась.
// Окно LiveSplit теперь снимает сам лаунчер (livesplit-window-capture.js).
const REMOVED_FROM_WINDOWS = [
  "resources/livesplit-overlay/hlsr-capture.exe",
];

// Аудит проверяет сборку той ОС, на которой запущен; --platform win32|linux
// задаёт её явно (например, для Windows-сборки, собранной на Linux).
const TARGETS = {
  win32: {
    dir: "win-unpacked",
    buildCommand: "npm run build:win",
    required: COMMON_REQUIRED.concat(WINDOWS_REQUIRED, ["HLSR.exe"]),
    forbidden: LINUX_ONLY_OVERLAY.concat(REMOVED_FROM_WINDOWS, ["resources/app.asar.unpacked/node_modules/7zip-bin/win/x64/7za.exe"]),
    executables: [],
  },
  linux: {
    dir: "linux-unpacked",
    buildCommand: "npm run build:linux",
    forbidden: WINDOWS_REQUIRED,
    required: COMMON_REQUIRED.concat(LINUX_ONLY_OVERLAY, [
      "resources/livesplit-fonts/Carlito-Regular.ttf",
      "resources/livesplit-fonts/Carlito-Bold.ttf",
      "resources/livesplit-fonts/OFL.txt",
      "resources/livesplit-fixes/LiveSplit.SourceSplit.dll",
      "resources/livesplit-fixes/SourceSplit-LICENSE.txt",
      "resources/presence-relay/hlsr-presence-relay.exe",
      "resources/umu/umu-run",
      "resources/linux/install-userns-profile.sh",
      "resources/app.asar.unpacked/node_modules/7zip-bin/linux/x64/7za",
      "hlsr",
      "start",
    ]),
    executables: ["hlsr", "start", "resources/umu/umu-run", "resources/app.asar.unpacked/node_modules/7zip-bin/linux/x64/7za"],
  },
};

const REQUIRED = TARGETS.win32.required;

function targetPlatform(argv) {
  const index = argv.indexOf("--platform");
  const value = index === -1 ? process.platform : argv[index + 1];
  return value === "linux" ? "linux" : "win32";
}

const DENY_RULES = [
  { label: "source map", re: /\.map$/i },
  { label: "тесты", re: /(^|[\\/])(test|tests|__tests__|spec)[\\/]/i },
  { label: "внутренние доки", re: /(^|[\\/])docs[\\/]/i },
  { label: ".env", re: /(^|[\\/])\.env($|\.)/i },
  { label: "приватный ключ", re: /\.(pem|key|p8|pkcs8)$/i },
  { label: "known_hosts/deploy", re: /(known_hosts|timeweb\.json|htaccess)/i },
  { label: "QA-скриншот", re: /(^|[\\/])(screenshots?|qa-)[^\\/]*\.(png|jpe?g)$/i },
  { label: "архив игры", re: /ghosting[^\\/]*\.(7z|zip)$/i },
  { label: "sample/fixture build metadata", re: /hlsr-build[^\\/]*\.json$/i },
  { label: "исходники Vue/SCSS", re: /\.(vue|scss|sass)$/i },
  { label: "steam_appid.txt", re: /steam_appid\.txt$/i },
  { label: "git-служебное", re: /(^|[\\/])\.git([\\/]|$|ignore|attributes)/i },
  { label: "npm-lock/CI", re: /(package-lock\.json|\.github[\\/])/i },
  {
    label: "hlsr-console (отвязан в 0.3.15)",
    re: /(^|[\\/])node_modules[\\/]hlsr-console[\\/]/i,
  },
  { label: "личный путь разработчика", re: devUserPath("[\\\\/]+") },
];

// Домашний каталог того, кто собирает (раньше здесь стояли имена разработчиков).
function devUserPath(sep) {
  const name = require("os").userInfo().username.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?:users|home)${sep}${name}(?:${sep}|$)`, "i");
}

const CONTENT_DENY = [
  { label: "локальный путь разработчика", re: devUserPath("(?:\\\\+|/)") },
  {
    label: "приватный ключ PEM",
    re: /BEGIN [A-Z ]*PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=]{40,}/,
  },
];

function listAsar(asarPath) {
  const fd = fs.openSync(asarPath, "r");

  try {
    const header = Buffer.alloc(16);
    fs.readSync(fd, header, 0, 16, 0);
    const jsonSize = header.readUInt32LE(12);

    const jsonBuffer = Buffer.alloc(jsonSize);
    fs.readSync(fd, jsonBuffer, 0, jsonSize, 16);

    const text = jsonBuffer.toString("utf8");
    const start = text.indexOf("{");
    const directory = JSON.parse(text.slice(start));

    const files = [];
    const walk = (node, prefix) => {
      if (!node.files) return;
      for (const [name, child] of Object.entries(node.files)) {
        const full = prefix ? `${prefix}/${name}` : name;
        if (child.files) walk(child, full);
        else files.push(full);
      }
    };

    walk(directory, "");
    return files;
  } finally {
    fs.closeSync(fd);
  }
}

function walkDir(root) {
  const files = [];
  const stack = [root];

  while (stack.length) {
    const dir = stack.pop();
    for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, item.name);
      if (item.isDirectory()) stack.push(full);
      else files.push(path.relative(root, full).replace(/\\/g, "/"));
    }
  }

  return files;
}

function main() {
  const target = TARGETS[targetPlatform(process.argv.slice(2))];
  const UNPACKED = path.join(REPO_ROOT, "build", target.dir);

  if (!fs.existsSync(UNPACKED)) {
    process.stderr.write(`Сборка не найдена: ${UNPACKED}. Сначала ${target.buildCommand}\n`);
    return 2;
  }

  const problems = [];
  const diskFiles = walkDir(UNPACKED);
  if (target === TARGETS.linux && diskFiles.some((file) => file.startsWith("resources/installer/")))
    problems.push("Инсталлер не должен входить в архив установленного лаунчера");

  for (const required of target.required)
    if (!diskFiles.includes(required))
      problems.push(`ОТСУТСТВУЕТ обязательный файл: ${required}`);

  for (const forbidden of target.forbidden || [])
    if (diskFiles.includes(forbidden))
      problems.push(`ЛИШНИЙ файл: ${forbidden}`);

  const asarPath = path.join(UNPACKED, "resources", "app.asar");
  const asarFiles = fs.existsSync(asarPath) ? listAsar(asarPath) : [];

  const allNames = [
    ...diskFiles.map((f) => ({ where: target.dir, name: f })),
    ...asarFiles.map((f) => ({ where: "app.asar", name: f })),
  ];

  // В app.asar — только собранный код, зависимости и значки. Правила
  // electron-builder вида **/node_modules/**/LICENSE* совпадают и с копиями
  // проекта в qa-out/, так в пакет однажды ушли 5000 файлов QA-прогонов.
  const ASAR_ROOTS = ["dist/", "node_modules/", "icons/"];
  const stray = asarFiles.filter((name) => name !== "package.json" && !ASAR_ROOTS.some((root) => name.startsWith(root)));
  if (stray.length)
    problems.push(`ЛИШНЕЕ в app.asar (${stray.length}): ${stray.slice(0, 5).join(", ")}`);

  for (const { where, name } of allNames)
    for (const rule of DENY_RULES)
      if (rule.re.test(name)) problems.push(`ЗАПРЕЩЕНО [${rule.label}] ${where}: ${name}`);

  if (fs.existsSync(asarPath)) {
    const raw = fs.readFileSync(asarPath);
    for (const rule of CONTENT_DENY) {
      if (rule.re.test(raw.toString("latin1")))
        problems.push(`ЗАПРЕЩЕНО [${rule.label}] найдено в содержимом app.asar`);
    }
  }

  for (const file of diskFiles) {
    const size = fs.statSync(path.join(UNPACKED, file)).size;
    if (size > 400 * 1024 * 1024)
      problems.push(`ПОДОЗРИТЕЛЬНО БОЛЬШОЙ файл (${size} байт): ${file}`);
  }

  for (const relative of target.executables) {
    const file = path.join(UNPACKED, relative);
    if (fs.existsSync(file) && !(fs.statSync(file).mode & 0o111))
      problems.push(`НЕТ ПРАВА ИСПОЛНЕНИЯ: ${relative}`);
  }

  process.stdout.write(
    `Файлов на диске: ${diskFiles.length}, в app.asar: ${asarFiles.length}\n`
  );

  if (problems.length) {
    process.stdout.write("\nПроблемы:\n");
    for (const problem of problems) process.stdout.write(`  - ${problem}\n`);
    return 1;
  }

  process.stdout.write("Аудит содержимого пакета: OK\n");
  return 0;
}

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`Аудит не выполнен: ${error.message}\n`);
    process.exitCode = 2;
  }
}

module.exports = { listAsar, DENY_RULES, CONTENT_DENY, REQUIRED, TARGETS };
