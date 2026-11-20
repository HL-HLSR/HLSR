#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const common = require("./lib/release-common");
const signing = require("../src/main/game-delivery/manifest-signing");
const channel = require("../src/main/launcher-update/launcher-channel");
const endpoints = require("../src/shared/delivery-endpoints");
const launcherChunks = require("./lib/launcher-chunks");

const CHANNEL_FILE = "launcher-channel.json";
const LATEST_YML = "latest-linux.yml";

// Тот же формат, что у latest.yml Windows от electron-builder.
function latestLinuxYml({ version, filename, sha512, size, releaseDate }) {
  return `version: ${version}\nfiles:\n  - url: ${filename}\n    sha512: ${sha512}\n    size: ${size}\n` +
    `path: ${filename}\nsha512: ${sha512}\nreleaseDate: '${releaseDate}'\n`;
}

// Релиз Linux устроен как релиз Windows: установщик, latest-linux.yml и
// подписанный launcher-channel.json. Архив лаунчера отдельно не публикуется —
// он вшит в AppImage, и обновлятор достаёт его оттуда.
function prepare(options = {}) {
  const version = options.version || require("../package.json").version;
  const buildDir = path.resolve(options.buildDir || "build");
  const outDir = path.resolve(options.outDir || `release/launcher-linux/${version}`);
  const filename = `HLSR-Setup-${version}-linux-x86_64.AppImage`;
  const image = path.join(buildDir, filename);
  const size = fs.statSync(image).size;
  const sha256 = common.sha256File(image), sha512 = common.sha512File(image);
  const publishedAt = new Date().toISOString();

  const output = path.join(outDir, CHANNEL_FILE);
  if (fs.existsSync(output) && JSON.parse(fs.readFileSync(output)).installer.sha256 !== sha256)
    throw new Error("Эта версия уже подготовлена с другими байтами; увеличьте версию");
  fs.mkdirSync(outDir, { recursive: true });

  const ymlPath = path.join(outDir, LATEST_YML);
  fs.writeFileSync(ymlPath, latestLinuxYml({ version, filename, sha512, size, releaseDate: publishedAt }));

  const manifest = {
    schemaVersion: channel.SUPPORTED_SCHEMA_VERSION, version, publishedAt,
    minimumSupportedVersion: options.minimumSupported || "0.3.16",
    updateRequired: !!options.updateRequired,
    installer: { filename, size, sha256, sha512, urls: endpoints.launcherFor("linux").installerUrls(version, filename) },
    latestYmlSha256: common.sha256File(ymlPath),
  };
  // Обновление чанками из той же сборки, что вшита в AppImage.
  const delta = options.chunks === false ? null : launcherChunks.prepare({
    version, platform: "linux", tree: options.tree || path.join(buildDir, "linux-unpacked"), outDir, store: options.chunkStore,
  });
  if (delta) manifest.delta = delta.delta;
  const key = options.privateKey || common.loadPrivateKey().pem;
  const signed = signing.attach(manifest, key);
  signed.signature.keyId = common.keyIdOf(crypto.createPublicKey(key).export({ type: "spki", format: "pem" }).toString());
  const verdict = channel.check(signed, { publicKeys: options.publicKeys || channel.LAUNCHER_PUBLIC_KEYS, platform: "linux" });
  if (!verdict.ok) throw new Error(`Production-подпись не прошла проверку: ${verdict.problems.join("; ")}`);
  fs.writeFileSync(output, JSON.stringify(signed, null, 2) + "\n");

  const remote = "launcher/linux/stable";
  const artifacts = [
    { kind: "launcher-installer", localPath: image, remotePath: `${remote}/${filename}`, immutable: true, phase: "artifact" },
    ...(delta ? [{ kind: "launcher-chunk-manifest", localPath: delta.manifestPath, remotePath: `${remote}/${path.basename(delta.manifestPath)}`, immutable: true, phase: "artifact" }] : []),
    { kind: "launcher-latest-yml", localPath: ymlPath, remotePath: `${remote}/${LATEST_YML}`, immutable: false, phase: "manifest" },
    { kind: "channel-manifest", localPath: output, remotePath: `${remote}/${CHANNEL_FILE}`, immutable: false, phase: "manifest" },
  ].map((item) => Object.assign(item, { size: fs.statSync(item.localPath).size, sha256: common.sha256File(item.localPath) }));
  const plan = {
    generatedAt: new Date().toISOString(), gameId: "hlsr-launcher-linux", version,
    channel: "stable", profile: "production", pointerRemotePath: `${remote}/${CHANNEL_FILE}`,
    confirmToken: `hlsr-launcher-linux@${version}:stable`, downloadBaseUrl: `${endpoints.PRIMARY_BASE}/`, artifacts,
    ...(delta ? { chunkStore: { localDir: delta.store, remoteDir: launcherChunks.REMOTE_STORE } } : {}),
  };
  fs.writeFileSync(path.join(outDir, "publish-plan.json"), JSON.stringify(plan, null, 2) + "\n");
  return { manifest: signed, outDir, plan };
}

if (require.main === module) {
  try {
    const options = {};
    const args = process.argv.slice(2);
    for (let i = 0; i < args.length; i++) {
      const key = { "--version": "version", "--build-dir": "buildDir", "--out": "outDir", "--minimum-supported": "minimumSupported" }[args[i]];
      if (key) options[key] = args[++i];
      else if (args[i] === "--update-required") options.updateRequired = true;
      else if (args[i] === "--no-chunks") options.chunks = false;
      else throw new Error(`Неизвестный аргумент: ${args[i]}`);
    }
    console.log(`Подготовлен Linux-релиз: ${prepare(options).outDir}`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = { prepare, latestLinuxYml, CHANNEL_FILE, LATEST_YML };
