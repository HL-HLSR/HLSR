#!/usr/bin/env node
"use strict";

// Копия scripts/launcher-release-prepare.js 0.4.7 (подпроект 6, задача 8, решение Р8) с правками:
//  * модули подписи, канала, адресов и чанков — замороженные копии tools/compat-047 (lib/frozen.js);
//  * версия — из Cargo.toml (package.json в 0.5 нет), только X.Y.Z (Р9);
//  * установщик, latest.yml и win-unpacked лежат там, куда их кладёт `hlsr-release setup`
//    (по умолчанию target/dist/windows); blockmap не публикуется (Р4) — его нет ни в проверке
//    артефактов, ни в плане публикации;
//  * в канал добавляется блок build (Р3): исполняемый файл с кусками и ключ аттестации;
//  * --test-public-key подменяет trust root для проверки подписи тестовым ключом (только тесты и
//    приёмка: боевой выпуск проверяется вшитыми ключами лаунчера).
// Ключ канала — только HLSR_MANIFEST_PRIVATE_KEY_PATH; ключ аттестации — HLSR_ATTEST_KEYS_DIR.

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { signing, launcherChannel, endpoints } = require("./lib/frozen");
const common = require("./lib/release-common");
const launcherChangelog = require("./lib/launcher-changelog");
const launcherChunks = require("./lib/launcher-chunks");

const REPO_ROOT = common.REPO_ROOT;
const BUILD_DIR = path.join(REPO_ROOT, "target", "dist", "windows");

function launcherChannelRemotePath() {
  const canonical = endpoints.launcherFor("win32").channelUrls[0];
  const base = new URL(endpoints.PRIMARY_BASE.replace(/\/?$/, "/"));
  const remote = new URL(canonical);

  if (remote.origin !== base.origin || !remote.pathname.startsWith(base.pathname))
    throw new Error(
      `Канонический URL канала лаунчера ${canonical} не лежит под ${base.href}. ` +
        "Схема публикации изменилась — обновите launcher-release-prepare."
    );

  return remote.pathname.slice(base.pathname.length);
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--version") args.version = argv[++i];
    else if (arg === "--build-dir") args.buildDir = argv[++i];
    else if (arg === "--qa-profile") args.qaProfile = true;
    else if (arg === "--minimum-supported") args.minimumSupported = argv[++i];
    else if (arg === "--update-required") args.updateRequired = true;
    else if (arg === "--no-chunks") args.noChunks = true;
    else if (arg === "--out") args.out = argv[++i];
    else if (arg === "--chunk-store") args.chunkStore = argv[++i];
    else if (arg === "--test-public-key") args.testPublicKey = argv[++i];
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`Неизвестный аргумент: ${arg}`);
  }
  return args;
}

function productionTrustProblems(signedManifest, trustedPems) {
  const problems = [];
  const pems = trustedPems || launcherChannel.LAUNCHER_PUBLIC_KEYS;

  const trustedIds = pems.map((pem) => common.keyIdOf(pem));
  const keyId = signedManifest.signature ? signedManifest.signature.keyId : null;

  if (!keyId || trustedIds.indexOf(keyId) === -1)
    problems.push(
      `keyId подписи (${keyId || "нет"}) не входит в trust root лаунчера ` +
        `(${trustedIds.join(", ")}). Production подписывается ТОЛЬКО ключом, ` +
        "публичная половина которого уже вшита в launcher-channel.js лаунчера 0.4.7 " +
        "(tools/compat-047/src/main/launcher-update/launcher-channel.js)"
    );

  const verdict = launcherChannel.check(signedManifest, { publicKeys: pems, platform: "win32" });
  if (!verdict.ok)
    problems.push(`подпись не проходит по вшитым ключам лаунчера: ${verdict.problems.join("; ")}`);

  return problems;
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help) {
    process.stdout.write(
      "HLSR_MANIFEST_PRIVATE_KEY_PATH=<ключ лаунчера> HLSR_ATTEST_KEYS_DIR=<ключи аттестации> \\\n" +
        "  node tools/release/node/launcher-release-prepare.js\n" +
        "  [--version x.y.z] [--build-dir <каталог hlsr-release setup>] [--minimum-supported x.y.z]\n" +
        "  [--update-required] [--no-chunks] [--qa-profile] [--out <каталог>]\n" +
        "  [--test-public-key <pem> — только для тестов]\n"
    );
    return 0;
  }

  const version = common.requireReleaseVersion(args.version || common.workspaceVersion());

  const profile = args.qaProfile
    ? {
        name: "qa",
        channel: "qa",
        remoteDir: `launcher/qa/${version}`,
        pointer: "launcher/channel/qa.json",
        confirmToken: `hlsr-launcher@${version}:qa`,
        outName: `qa-${version}`,
      }
    : {
        name: "production",
        channel: "stable",
        remoteDir: "launcher/stable",
        pointer: launcherChannelRemotePath(),
        confirmToken: `hlsr-launcher@${version}:stable`,
        outName: version,
      };

  const buildDir = args.buildDir ? path.resolve(args.buildDir) : BUILD_DIR;

  const installerName = `HLSR-Setup-${version}.exe`;
  const installerPath = path.join(buildDir, installerName);
  const latestYmlPath = path.join(buildDir, "latest.yml");

  for (const file of [installerPath, latestYmlPath])
    if (!fs.existsSync(file))
      throw new Error(`Не найден артефакт сборки: ${file}. Сначала hlsr-release setup`);

  const outDir = args.out
    ? path.resolve(args.out)
    : path.join(REPO_ROOT, "release", "launcher", profile.outName);
  fs.mkdirSync(outDir, { recursive: true });

  process.stdout.write(`Установщик: ${installerPath}\nСчитаю хеши...\n`);

  const installerSize = fs.statSync(installerPath).size;
  const installerSha256 = common.sha256File(installerPath);
  const installerSha512 = common.sha512File(installerPath);
  const latestYmlSha256 = common.sha256File(latestYmlPath);

  if (installerSize > launcherChannel.MAX_INSTALLER_BYTES)
    throw new Error(
      `Установщик ${installerSize} байт больше предела ${launcherChannel.MAX_INSTALLER_BYTES}`
    );

  const latestYml = fs.readFileSync(latestYmlPath, "utf8");
  if (latestYml.indexOf(installerName) === -1)
    throw new Error(`latest.yml не упоминает ${installerName} — это yml другой сборки`);
  if (!new RegExp(`^version:\\s*${version.replace(/\./g, "\\.")}\\s*$`, "m").test(latestYml))
    throw new Error(`latest.yml описывает другую версию, ожидалась ${version}`);
  if (latestYml.indexOf(installerSha512) === -1)
    throw new Error("sha512 установщика не совпадает с latest.yml — артефакты рассинхронизированы");

  const manifest = {
    schemaVersion: 1,
    version,
    publishedAt: new Date().toISOString(),
    minimumSupportedVersion: args.minimumSupported || "0.2.7",
    updateRequired: !!args.updateRequired,
    installer: {
      filename: installerName,
      size: installerSize,
      sha256: installerSha256,
      sha512: installerSha512,
      urls: endpoints.launcherFor("win32").installerUrls(version, installerName),
    },
    latestYmlSha256,
  };

  const tree = path.join(buildDir, "win-unpacked");

  // Обновление чанками из той же сборки, что упакована в установщик.
  const delta = args.noChunks ? null : launcherChunks.prepare({
    version, platform: "win32", tree, outDir, store: args.chunkStore ? path.resolve(args.chunkStore) : undefined,
  });
  if (delta) manifest.delta = delta.delta;

  // Регистрация сборки на сервере (Р3): исполняемый файл с кусками и ключ аттестации.
  manifest.build = launcherChunks.buildBlock({ version, platform: "win32", tree });

  const shapeProblems = launcherChannel.validateShape(manifest, "win32");
  if (shapeProblems.length) {
    process.stderr.write("Манифест лаунчера не прошёл проверку:\n");
    for (const problem of shapeProblems) process.stderr.write(`  - ${problem}\n`);
    return 1;
  }

  const key = common.loadPrivateKey();
  const signed = signing.attach(manifest, key.pem);

  const publicPem = crypto
    .createPublicKey(key.pem)
    .export({ type: "spki", format: "pem" })
    .toString();
  signed.signature.keyId = common.keyIdOf(publicPem);

  const verdict = launcherChannel.check(signed, { publicKeys: [publicPem], platform: "win32" });
  if (!verdict.ok)
    throw new Error(`Самопроверка не прошла: ${verdict.problems.join("; ")}`);

  const testPem = args.testPublicKey ? fs.readFileSync(args.testPublicKey, "utf8") : null;
  if (testPem)
    process.stdout.write("ВНИМАНИЕ: --test-public-key подменяет trust root. Это допустимо ТОЛЬКО в тестах.\n");

  if (!args.qaProfile) {
    const trustProblems = productionTrustProblems(signed, testPem ? [testPem] : undefined);
    if (trustProblems.length) {
      process.stderr.write("PRODUCTION-канал не может быть подписан этим ключом:\n");
      for (const problem of trustProblems) process.stderr.write(`  - ${problem}\n`);
      process.stderr.write(
        "Если это осознанная ротация ключа — сначала добавьте новый публичный ключ в\n" +
          "launcher-channel.js, выпустите лаунчер с ним и лишь затем подписывайте канал новым ключом.\n"
      );
      return 1;
    }
  } else {
    const builtin = launcherChannel.check(signed, { platform: "win32" });
    if (!builtin.ok)
      process.stdout.write(
        "QA-профиль: подпись не проходит по боевым ключам лаунчера (ожидаемо для QA-ключа).\n"
      );
  }

  const manifestOut = path.join(outDir, "launcher-channel.json");

  if (fs.existsSync(manifestOut)) {
    const previous = common.readJson(manifestOut);
    const prevSha = previous && previous.installer ? previous.installer.sha256 : null;
    if (prevSha && prevSha !== installerSha256)
      throw new Error(
        `${manifestOut} уже существует и описывает ДРУГОЙ установщик ` +
          `(${prevSha.slice(0, 16)}… != ${installerSha256.slice(0, 16)}…). ` +
          "Удалите каталог релиза и запустите prepare заново для текущей сборки"
      );
  }

  common.writeJson(manifestOut, signed);

  const changelogOut = path.join(outDir, "changelog.json");
  let changelogArtifact = null;

  try {
    common.writeJson(changelogOut, launcherChangelog.buildChangelogJson(version));
    changelogArtifact = {
      kind: "launcher-changelog",
      localPath: changelogOut,
      remotePath: `${profile.remoteDir}/changelog.json`,
      size: fs.statSync(changelogOut).size,
      sha256: common.sha256File(changelogOut),
      immutable: false,
      phase: "manifest",
    };
  } catch (error) {
    if (!args.qaProfile) throw error;
    process.stdout.write(`QA-профиль: ченджлог не собран (${error.message})\n`);
  }

  const plan = {
    generatedAt: new Date().toISOString(),
    gameId: "hlsr-launcher",
    version,
    channel: profile.channel,
    profile: profile.name,
    qaPrefix: "launcher/qa/",
    qaPointer: "launcher/channel/qa.json",
    pointerRemotePath: profile.pointer,
    confirmToken: profile.confirmToken,
    downloadBaseUrl: "https://hlsr.pro/downloads/",
    ...(delta ? { chunkStore: { localDir: delta.store, remoteDir: launcherChunks.REMOTE_STORE } } : {}),
    artifacts: [
      {
        kind: "launcher-installer",
        localPath: installerPath,
        remotePath: `${profile.remoteDir}/${installerName}`,
        size: installerSize,
        sha256: installerSha256,
        immutable: true,
        phase: "artifact",
      },
      ...(delta
        ? [
            {
              kind: "launcher-chunk-manifest",
              localPath: delta.manifestPath,
              remotePath: `${profile.remoteDir}/${path.basename(delta.manifestPath)}`,
              size: fs.statSync(delta.manifestPath).size,
              sha256: common.sha256File(delta.manifestPath),
              immutable: true,
              phase: "artifact",
            },
          ]
        : []),
      {
        kind: "launcher-latest-yml",
        localPath: latestYmlPath,
        remotePath: `${profile.remoteDir}/latest.yml`,
        size: fs.statSync(latestYmlPath).size,
        sha256: latestYmlSha256,
        immutable: false,
        phase: "manifest",
      },
      ...(changelogArtifact ? [changelogArtifact] : []),
      {
        kind: "channel-manifest",
        localPath: manifestOut,
        remotePath: profile.pointer,
        size: fs.statSync(manifestOut).size,
        sha256: common.sha256File(manifestOut),
        immutable: false,
        phase: "manifest",
      },
    ],
  };

  common.writeJson(path.join(outDir, "publish-plan.json"), plan);

  process.stdout.write(
    `\nКаталог релиза : ${outDir}\n` +
      `Манифест       : ${manifestOut} (keyId ${signed.signature.keyId})\n` +
      `sha256         : ${installerSha256}\n` +
      `sha512         : ${installerSha512.slice(0, 24)}...\n` +
      `latest.yml sha : ${latestYmlSha256}\n\n` +
      "Зеркало hlsr.pro (выкладка вне репозитория): --plan " +
      `${path.relative(REPO_ROOT, path.join(outDir, "publish-plan.json"))} ` +
      `[--publish --confirm ${profile.confirmToken}]  (только владелец)\n` +
      "GitHub Release (владелец, вручную): загрузить установщик, latest.yml и launcher-channel.json " +
      "в HL-HLSR/HLSR-Releases (blockmap не публикуется, Р4).\n"
  );

  return 0;
}

module.exports = { productionTrustProblems, parseArgs };

if (require.main === module) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`prepare лаунчера не выполнен: ${error.message}\n`);
    process.exitCode = 2;
  }
}
