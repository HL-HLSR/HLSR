"use strict";

const fs = require("fs");
const path = require("path");
const os = require("os");
const https = require("https");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const VERSION = "1.4.4";
const RELEASE_URL = `https://github.com/Open-Wine-Components/umu-launcher/releases/download/${VERSION}/umu-launcher-${VERSION}-zipapp.tar`;
const SHA256 = "eb590691841f7fad3fc3ad8fd5db4ccb87849fe7948e62b28ece7a4ee48cc851";
const DEST = path.join(__dirname, "..", "build", "umu");

function get(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" || ![
      "github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com",
    ].includes(parsed.hostname) || redirects > 5) {
      reject(new Error("Недоверенный адрес umu-launcher"));
      return;
    }
    const req = https.get(parsed, { headers: { "User-Agent": "HLSR-Linux-Build" } }, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        get(response.headers.location, redirects + 1).then(resolve, reject);
      } else if (response.statusCode === 200) resolve(response);
      else { response.resume(); reject(new Error(`GitHub ответил ${response.statusCode}`)); }
    });
    req.on("error", reject);
  });
}

async function main() {
  const target = path.join(DEST, "umu-run");
  if (fs.existsSync(target)) return;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-umu-"));
  try {
    const archive = path.join(temp, "umu.tar");
    const response = await get(RELEASE_URL);
    const writer = fs.createWriteStream(archive);
    const hash = crypto.createHash("sha256");
    let size = 0;
    await new Promise((resolve, reject) => {
      response.on("data", (chunk) => {
        size += chunk.length;
        if (size > 2 * 1024 * 1024) response.destroy(new Error("Архив umu слишком велик"));
        hash.update(chunk);
      });
      response.on("error", reject);
      writer.on("error", reject);
      writer.on("finish", resolve);
      response.pipe(writer);
    });
    if (hash.digest("hex") !== SHA256) throw new Error("SHA-256 umu-launcher не совпал");
    const listing = spawnSync("tar", ["-tf", archive], { encoding: "utf8" });
    if (listing.status !== 0 || listing.stdout.trim().split("\n").some((name) =>
      !["umu/", "umu/umu-run", "umu/umu_run.py"].includes(name)))
      throw new Error("Неожиданное содержимое архива umu-launcher");
    fs.mkdirSync(path.dirname(DEST), { recursive: true });
    const extracted = spawnSync("tar", ["-xf", archive, "-C", path.dirname(DEST)], { encoding: "utf8" });
    if (extracted.status !== 0 || !fs.existsSync(target))
      throw new Error(`Не удалось распаковать umu-launcher: ${extracted.stderr}`);
    fs.chmodSync(target, 0o755);
    process.stdout.write(`Bundled umu-launcher ${VERSION}\n`);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

main().catch((error) => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
