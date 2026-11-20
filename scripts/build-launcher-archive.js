"use strict";
const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

function buildArchive(options = {}) {
  const version = options.version || require("../package.json").version;
  const source = path.resolve(options.source || "build/linux-unpacked");
  const output = path.resolve(options.output || `build/HLSR-${version}-linux-x86_64.tar.gz`);
  const embedded = JSON.parse(require("@electron/asar").extractFile(path.join(source, "resources/app.asar"), "package.json"));
  if (embedded.version !== version) throw new Error("Версия упакованного приложения не совпала");
  fs.writeFileSync(path.join(source, ".hlsr-release.json"), JSON.stringify({ version, platform: "linux", arch: "x64", executable: "hlsr" }) + "\n");
  let unpackedSize = 0;
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(file);
      else if (entry.isFile()) unpackedSize += fs.statSync(file).size;
      else throw new Error(`Архив не должен содержать ссылки: ${file}`);
    }
  }
  walk(source);
  execFileSync("tar", ["--format=ustar", "--sort=name", "--owner=0", "--group=0", "--numeric-owner", "-czf", `${output}.partial`, "-C", source, "."]);
  fs.renameSync(`${output}.partial`, output);
  fs.writeFileSync(`${output}.json`, JSON.stringify({ version, unpackedSize }) + "\n");
  return { output, unpackedSize };
}
if (require.main === module) {
  try { console.log(buildArchive()); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
module.exports = { buildArchive };
