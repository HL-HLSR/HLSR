#!/usr/bin/env node
"use strict";

// Копия scripts/publish-launcher-linux.js 0.4.7 (подпроект 6, задача 8, решение Р8): модули канала и
// адресов - замороженные копии tools/compat-047 (lib/frozen.js), проверка и подготовка - соседние
// копии в этом каталоге; каталог по умолчанию - target/dist/linux. ПУБЛИКУЕТ В GITHUB (gh) с
// --publish: запускает только владелец, по своему "да" (ПП7). Без --publish только проверяет.

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const { verify } = require("./verify-launcher-release-linux");
const { CHANNEL_FILE, LATEST_YML } = require("./launcher-release-prepare-linux");
const { endpoints, launcherChannel: channel } = require("./lib/frozen");
const common = require("./lib/release-common");

const repo = endpoints.LAUNCHER_RELEASES_REPO;
const STABLE = "linux-stable";
function gh(args) { return execFileSync("gh", args.concat(["--repo", repo]), { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }); }

// Как у Windows: в релизе только установщик, latest-linux.yml и подписанный
// launcher-channel.json. Версия уходит в linux-v<версия>, linux-stable
// всегда содержит актуальный канал.
async function main() {
  const dir = path.resolve(process.argv[2] || path.join(common.REPO_ROOT, "target", "dist", "linux"));
  const manifest = await verify(dir);
  const tag = endpoints.launcherFor("linux").releaseTag(manifest.version);
  const installer = manifest.installer.filename;
  const assets = [installer, LATEST_YML, CHANNEL_FILE];
  if (!process.argv.includes("--publish")) {
    console.log(`Проверено. Публикация с --publish создаст ${repo}/${tag} и обновит ${STABLE}; Windows latest не изменится.`);
    return;
  }
  // Only a confirmed 404 permits creation; auth/network errors must not masquerade as absence.
  function release(name) {
    try { return JSON.parse(gh(["release", "view", name, "--json", "isDraft,assets"])); }
    catch (error) {
      if (/release not found|HTTP 404/i.test(String(error.stderr))) return null;
      throw error;
    }
  }
  const existing = release(tag);
  if (!existing) {
    gh(["release", "create", tag, "--draft", "--latest=false", "--title", `HLSR Linux ${manifest.version}`, "--notes", `Linux x64 AppImage ${manifest.version}`]);
    gh(["release", "upload", tag, ...assets.map((name) => path.join(dir, name))]);
  } else if (existing.isDraft) {
    // Resume a partially uploaded draft without overwriting any existing bytes.
    const missing = assets.filter((name) => !existing.assets.some((asset) => asset.name === name));
    if (missing.length) gh(["release", "upload", tag, ...missing.map((name) => path.join(dir, name))]);
  }
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-release-verify-"));
  try {
    gh(["release", "download", tag, "--dir", scratch]);
    await verify(scratch);
    for (const name of assets) {
      if (common.sha256File(path.join(dir, name)) !== common.sha256File(path.join(scratch, name)))
        throw new Error(`Загруженный ${name} отличается от локального`);
    }
    gh(["release", "edit", tag, "--draft=false", "--latest=false"]);
    const stable = release(STABLE);
    if (stable && stable.assets.some((asset) => asset.name === CHANNEL_FILE)) {
      const previousChannel = path.join(scratch, "previous-channel");
      fs.mkdirSync(previousChannel);
      gh(["release", "download", STABLE, "--pattern", CHANNEL_FILE, "--dir", previousChannel]);
      const previous = JSON.parse(fs.readFileSync(path.join(previousChannel, CHANNEL_FILE), "utf8"));
      const verdict = channel.check(previous, { platform: "linux" });
      if (!verdict.ok) throw new Error("Текущий Linux stable не прошёл проверку подписи");
      if (channel.compareSemver(previous.version, manifest.version) > 0)
        throw new Error("Нельзя заменить Linux stable более старой версией");
    }
    if (!stable) gh(["release", "create", STABLE, "--draft", "--latest=false", "--title", "HLSR Linux stable", "--notes", "Linux update channel; Windows is in win-stable."]);
    // Upload the installer first. Never overwrite an existing version with different bytes.
    const old = stable && stable.assets.find((asset) => asset.name === installer);
    if (old) {
      const previous = path.join(scratch, "stable");
      fs.mkdirSync(previous);
      gh(["release", "download", STABLE, "--pattern", installer, "--dir", previous]);
      if (common.sha256File(path.join(previous, installer)) !== manifest.installer.sha256)
        throw new Error(`${STABLE} содержит другой AppImage той же версии`);
    } else gh(["release", "upload", STABLE, path.join(dir, installer)]);
    gh(["release", "upload", STABLE, path.join(dir, LATEST_YML), "--clobber"]);
    gh(["release", "upload", STABLE, path.join(dir, CHANNEL_FILE), "--clobber"]);
    const stableDir = path.join(scratch, "verified-stable");
    fs.mkdirSync(stableDir);
    for (const name of assets) gh(["release", "download", STABLE, "--pattern", name, "--dir", stableDir]);
    await verify(stableDir);
    gh(["release", "edit", STABLE, "--draft=false", "--latest=false"]);
    console.log(`Опубликован ${tag}; Linux stable проверен. Windows latest не изменён.`);
  } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
}
main().catch((error) => { console.error(error.message); process.exitCode = 1; });
