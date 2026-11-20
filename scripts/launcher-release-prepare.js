#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const signing = require("../src/main/game-delivery/manifest-signing");
const launcherChannel = require("../src/main/launcher-update/launcher-channel");
const common = require("./lib/release-common");
const endpoints = require("../src/shared/delivery-endpoints");
const launcherChangelog = require("./lib/launcher-changelog");
const launcherChunks = require("./lib/launcher-chunks");

const REPO_ROOT = common.REPO_ROOT;
const BUILD_DIR = path.join(REPO_ROOT, "build");

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
        "публичная половина которого уже вшита в src/main/launcher-update/launcher-channel.js"
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
      "HLSR_MANIFEST_PRIVATE_KEY_PATH=<ключ лаунчера> node scripts/launcher-release-prepare.js\n" +
        "  [--version x.y.z] [--minimum-supported x.y.z] [--update-required] [--no-chunks] [--out <каталог>]\n"
    );
    return 0;
  }

  const packageJson = common.readJson(path.join(REPO_ROOT, "package.json"));
  const version = args.version || packageJson.version;

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
  const blockmapPath = `${installerPath}.blockmap`;
  const latestYmlPath = path.join(buildDir, "latest.yml");

  for (const file of [installerPath, blockmapPath, latestYmlPath])
    if (!fs.existsSync(file))
      throw new Error(`Не найден артефакт сборки: ${file}. Сначала npm run build:win`);

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

  // Обновление чанками из той же сборки, что упакована в установщик.
  const delta = args.noChunks ? null : launcherChunks.prepare({
    version, platform: "win32", tree: path.join(buildDir, "win-unpacked"), outDir,
  });
  if (delta) manifest.delta = delta.delta;

  const shapeProblems = launcherChannel.validateShape(manifest, "win32");
  if (shapeProblems.length) {
    process.stderr.write("Манифест лаунчера не прошёл проверку:\n");
    for (const problem of shapeProblems) process.stderr.write(`  - ${problem}\n`);
    return 1;
  }

  const key = common.loadPrivateKey();
  const signed = signing.attach(manifest, key.pem);

  const crypto = require("crypto");
  const publicPem = crypto
    .createPublicKey(key.pem)
    .export({ type: "spki", format: "pem" })
    .toString();
  signed.signature.keyId = common.keyIdOf(publicPem);

  const verdict = launcherChannel.check(signed, { publicKeys: [publicPem], platform: "win32" });
  if (!verdict.ok)
    throw new Error(`Самопроверка не прошла: ${verdict.problems.join("; ")}`);

  if (!args.qaProfile) {
    const trustProblems = productionTrustProblems(signed);
    if (trustProblems.length) {
      process.stderr.write("PRODUCTION-канал не может быть подписан этим ключом:\n");
      for (const problem of trustProblems) process.stderr.write(`  - ${problem}\n`);
      process.stderr.write(
        "Если это осознанная ротация ключа — сначала добавьте новый публичный ключ в\n" +
          "src/main/launcher-update/launcher-channel.js, выпустите лаунчер с ним и лишь\n" +
          "затем подписывайте канал новым ключом.\n"
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
      ...(fs.existsSync(blockmapPath)
        ? [
            {
              kind: "launcher-blockmap",
              localPath: blockmapPath,
              remotePath: `${profile.remoteDir}/${installerName}.blockmap`,
              size: fs.statSync(blockmapPath).size,
              sha256: common.sha256File(blockmapPath),
              immutable: true,
              phase: "artifact",
            },
          ]
        : []),
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
      `[--publish --confirm ${profile.confirmToken}]\n` +
      "GitHub Release (владелец, вручную): загрузить установщик, blockmap, latest.yml " +
      "и launcher-channel.json в HL-HLSR/HLSR-Releases.\n"
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
