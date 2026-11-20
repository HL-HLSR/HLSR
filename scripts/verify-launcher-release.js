#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const launcherChannel = require("../src/main/launcher-update/launcher-channel");
const common = require("./lib/release-common");
const endpoints = require("../src/shared/delivery-endpoints");

const RELEASES_REPO = "HL-HLSR/HLSR-Releases";

const ALLOWED_URL_HOSTS = ["github.com", "objects.githubusercontent.com", "hlsr.pro"];

const LEAK_PATTERNS = [
  [/[A-Za-z]:\\/, "локальный Windows-путь"],
  [/file:\/\//i, "file:// URL"],
  [/127\.0\.0\.1|localhost/i, "loopback URL"],
  [/\.ngrok\.|\.trycloudflare\.|\.loca\.lt/i, "временный туннельный URL"],
  [/(token|secret|password|api[_-]?key)=/i, "секрет в query-параметре"],
  [/-----BEGIN [A-Z ]*PRIVATE KEY-----/, "приватный ключ"],
  [/ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}/, "GitHub-токен"],
];

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--channel") args.channel = argv[++i];
    else if (arg === "--installer") args.installer = argv[++i];
    else if (arg === "--blockmap") args.blockmap = argv[++i];
    else if (arg === "--latest-yml") args.latestYml = argv[++i];
    else if (arg === "--expected-version") args.expectedVersion = argv[++i];
    else if (arg === "--public-key") args.publicKey = argv[++i];
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function verifyLauncherRelease(options) {
  const problems = [];
  const push = (message) => problems.push(message);

  const expectedVersion = options.expectedVersion;
  if (!common.semverParse(expectedVersion)) {
    push(`--expected-version не SemVer: ${expectedVersion}`);
    return problems;
  }

  for (const [label, file] of [
    ["channel", options.channel],
    ["installer", options.installer],
    ["blockmap", options.blockmap],
    ["latest-yml", options.latestYml],
  ]) {
    if (!file) push(`не задан --${label}`);
    else if (!fs.existsSync(file)) push(`${label}: файл не найден: ${file}`);
  }
  if (problems.length) return problems;

  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(options.channel, "utf8"));
  } catch (error) {
    push(`launcher-channel.json не парсится: ${error.message}`);
    return problems;
  }

  const trustedPems = options.publicKeyPem
    ? [options.publicKeyPem]
    : launcherChannel.LAUNCHER_PUBLIC_KEYS;

  const shape = launcherChannel.validateShape(manifest, "win32");
  for (const problem of shape) push(`schema: ${problem}`);

  const verdict = launcherChannel.check(manifest, { publicKeys: trustedPems, platform: "win32" });
  if (!verdict.ok)
    for (const problem of verdict.problems)
      if (shape.indexOf(problem) === -1) push(`check: ${problem}`);

  const trustedIds = trustedPems.map((pem) => common.keyIdOf(pem));
  const keyId = manifest.signature ? manifest.signature.keyId : null;
  if (!keyId || trustedIds.indexOf(keyId) === -1)
    push(`keyId подписи (${keyId || "нет"}) не входит в trust root (${trustedIds.join(", ")})`);

  if (problems.length) return problems;

  if (manifest.version !== expectedVersion)
    push(`канал описывает версию ${manifest.version}, ожидалась ${expectedVersion}`);

  if (common.semverCompare(manifest.minimumSupportedVersion, manifest.version) > 0)
    push(
      `minimumSupportedVersion (${manifest.minimumSupportedVersion}) новее самой версии ` +
        `${manifest.version} — обновиться не сможет никто`
    );

  if (typeof manifest.updateRequired !== "boolean")
    push("updateRequired должен быть boolean");

  const expectedName = `HLSR-Setup-${expectedVersion}.exe`;
  const actualName = path.basename(options.installer);

  if (manifest.installer.filename !== expectedName)
    push(`канал описывает ${manifest.installer.filename}, ожидался ${expectedName}`);
  if (actualName !== expectedName)
    push(`файл установщика называется ${actualName}, ожидался ${expectedName}`);

  const installerSize = fs.statSync(options.installer).size;
  if (installerSize !== manifest.installer.size)
    push(`размер установщика ${installerSize} != ${manifest.installer.size} из канала`);

  const sha256 = common.sha256File(options.installer);
  if (sha256 !== manifest.installer.sha256)
    push("SHA-256 установщика не совпадает с каналом — канал описывает ДРУГИЕ байты");

  const sha512 = common.sha512File(options.installer);
  if (sha512 !== manifest.installer.sha512)
    push("SHA-512 установщика не совпадает с каналом");

  const latestYmlSha = common.sha256File(options.latestYml);
  if (manifest.latestYmlSha256 && latestYmlSha !== manifest.latestYmlSha256)
    push("SHA-256 latest.yml не совпадает с каналом — yml от другой сборки");
  if (!manifest.latestYmlSha256)
    push("канал не содержит latestYmlSha256 — подмена latest.yml не будет обнаружена");

  const latestYml = fs.readFileSync(options.latestYml, "utf8");
  if (latestYml.indexOf(expectedName) === -1)
    push(`latest.yml не упоминает ${expectedName}`);
  if (!new RegExp(`^version:\\s*${expectedVersion.replace(/\./g, "\\.")}\\s*$`, "m").test(latestYml))
    push(`latest.yml описывает другую версию, ожидалась ${expectedVersion}`);
  if (latestYml.indexOf(sha512) === -1)
    push("SHA-512 установщика отсутствует в latest.yml");

  const blockmapSize = fs.statSync(options.blockmap).size;
  if (blockmapSize <= 0) push("blockmap пустой");
  if (path.basename(options.blockmap) !== `${expectedName}.blockmap`)
    push(`blockmap называется ${path.basename(options.blockmap)}, ожидался ${expectedName}.blockmap`);

  const expected = endpoints.launcherFor("win32").installerUrls(expectedVersion, expectedName);
  const expectedPrimaryUrl = expected[0];
  const expectedGithubUrl = expected[1];

  const urls = manifest.installer.urls || [];
  if (urls.indexOf(expectedGithubUrl) === -1)
    push(`в installer.urls нет точного GitHub-URL релиза: ${expectedGithubUrl}`);
  if (urls.indexOf(expectedPrimaryUrl) === -1)
    push(`в installer.urls нет точного URL основного источника: ${expectedPrimaryUrl}`);
  if (urls[0] !== expectedPrimaryUrl)
    push(
      `первым в installer.urls должен стоять основной источник ${expectedPrimaryUrl}, ` +
        `а стоит ${urls[0] || "ничего"}`
    );

  for (const url of urls) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (error) {
      push(`некорректный URL: ${url}`);
      continue;
    }
    if (parsed.protocol !== "https:") push(`URL не HTTPS: ${url}`);
    if (ALLOWED_URL_HOSTS.indexOf(parsed.hostname) === -1)
      push(`URL вне allowlist-хостов: ${url}`);
    if (!parsed.pathname.endsWith(`/${expectedName}`))
      push(`URL указывает не на ${expectedName}: ${url}`);
  }

  const rawChannel = fs.readFileSync(options.channel, "utf8");
  for (const [re, what] of LEAK_PATTERNS)
    if (re.test(rawChannel)) push(`в launcher-channel.json найден ${what}`);

  return problems;
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help) {
    process.stdout.write(
      "node scripts/verify-launcher-release.js --channel <json> --installer <exe> " +
        "--blockmap <blockmap> --latest-yml <yml> --expected-version <x.y.z> " +
        "[--public-key <pem> — только для тестов]\n"
    );
    return 0;
  }

  const publicKeyPem = args.publicKey ? fs.readFileSync(args.publicKey, "utf8") : null;
  if (publicKeyPem)
    process.stdout.write(
      "ВНИМАНИЕ: --public-key подменяет trust root. Это допустимо ТОЛЬКО в тестах.\n"
    );

  const problems = verifyLauncherRelease({
    channel: args.channel,
    installer: args.installer,
    blockmap: args.blockmap,
    latestYml: args.latestYml,
    expectedVersion: args.expectedVersion,
    publicKeyPem,
  });

  if (problems.length) {
    process.stderr.write("Проверка релиза лаунчера НЕ пройдена:\n");
    for (const problem of problems) process.stderr.write(`  ✗ ${problem}\n`);
    return 1;
  }

  process.stdout.write(
    `Релиз лаунчера ${args.expectedVersion} корректен: подпись, hash'и, latest.yml, ` +
      "blockmap и URL описывают ровно эти байты.\n"
  );
  return 0;
}

module.exports = { verifyLauncherRelease, parseArgs, LEAK_PATTERNS, ALLOWED_URL_HOSTS };

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`verify релиза не выполнен: ${error.message}\n`);
    process.exitCode = 2;
  }
}
