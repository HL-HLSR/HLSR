"use strict";

const fs = require("fs");
const path = require("path");
const https = require("https");
const crypto = require("crypto");
const { execFile } = require("child_process");
const { pipeline } = require("stream/promises");
const { validateArchiveListing } = require("./ProtonManager");

const DISTRIBUTION = Object.freeze({
  name: "cpython-3.12.15+20261001-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz",
  size: 34289539,
  sha256: "7bb1659e3235077b7f63d5b6eb6ce653c6fcd6c5041e9d5f73b42ce10421464d",
  url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261001/cpython-3.12.15%2B20261001-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz",
});
const HOSTS = new Set(["github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"]);
const PROBE = "import sys,ssl,lzma,ctypes; assert sys.version_info >= (3,10)";

function command(file, args, env, maxBuffer = 16 * 1024 * 1024) {
  return new Promise((resolve, reject) => execFile(file, args, {
    env, encoding: "utf8", timeout: 60000, maxBuffer,
  }, (error, stdout) => error ? reject(error) : resolve(stdout)));
}

async function compatiblePython(file, env) {
  try {
    await command(file, ["-I", "-c", PROBE], env, 4096);
    return true;
  } catch (error) { return false; }
}

function request(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" || !HOSTS.has(parsed.hostname) || redirects > 5)
      return reject(new Error("Недоверенный адрес загрузки Python"));
    const req = https.get(parsed, { headers: { "User-Agent": "HLSR-Linux" }, timeout: 30000 }, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode) && response.headers.location) {
        response.resume();
        request(new URL(response.headers.location, parsed).href, redirects + 1).then(resolve, reject);
      } else if (response.statusCode === 200) resolve(response);
      else { response.resume(); reject(new Error(`Загрузка Python: HTTP ${response.statusCode}`)); }
    });
    req.on("timeout", () => req.destroy(new Error("Тайм-аут загрузки Python")));
    req.on("error", reject);
  });
}

class PythonRuntime {
  constructor(options = {}) {
    this.root = path.join(options.dataDir, "runtimes", "python-3.12.15-20261001");
    this.logger = options.logger || { info() {}, warn() {} };
    this.probe = options.probe || compatiblePython;
    this.pending = null;
    this.selected = null;
  }

  async ensure(env) {
    if (this.selected) return this.selected;
    if (this.pending) return this.pending;
    this.pending = this.findOrInstall(env).finally(() => { this.pending = null; });
    return this.pending;
  }

  async findOrInstall(env) {
    // Probe the exact PATH used by umu's /usr/bin/env python3 shebang.
    for (const directory of String(env.PATH || "").split(path.delimiter).filter(Boolean)) {
      const candidate = path.join(directory, "python3");
      if (await this.probe(candidate, env)) return (this.selected = candidate);
    }
    const managed = path.join(this.root, "bin", "python3");
    if (await this.probe(managed, env)) return (this.selected = managed);
    this.logger.info("[runtime] Python 3.10+ отсутствует или неполон; скачиваю Python для umu");
    await this.install(env);
    if (!await this.probe(managed, env))
      throw new Error("Скачанный Python не запускается. Нужен Linux x86_64 с glibc.");
    return (this.selected = managed);
  }

  async install(env) {
    fs.mkdirSync(path.dirname(this.root), { recursive: true });
    const staging = fs.mkdtempSync(path.join(path.dirname(this.root), ".python-"));
    try {
      const archive = path.join(staging, "python.tar.gz");
      const response = await request(DISTRIBUTION.url);
      const hash = crypto.createHash("sha256");
      let size = 0;
      response.on("data", (chunk) => {
        size += chunk.length;
        hash.update(chunk);
        if (size > DISTRIBUTION.size) response.destroy(new Error("Архив Python слишком велик"));
      });
      await pipeline(response, fs.createWriteStream(archive, { mode: 0o600 }));
      if (size !== DISTRIBUTION.size || hash.digest("hex") !== DISTRIBUTION.sha256)
        throw new Error("Размер или SHA-256 архива Python не совпал");
      const [names, verbose] = await Promise.all([
        command("tar", ["-tzf", archive], env),
        command("tar", ["-tvzf", archive], env),
      ]);
      validateArchiveListing(names, verbose, "python", /^python$/);
      await command("tar", ["-xzf", archive, "-C", staging, "--no-same-owner", "--no-same-permissions"], env);
      const extracted = path.join(staging, "python");
      if (!await compatiblePython(path.join(extracted, "bin", "python3"), env))
        throw new Error("Проверка скачанного Python не пройдена");
      // Replace only HLSR's managed copy after successful verification.
      fs.rmSync(this.root, { recursive: true, force: true });
      fs.renameSync(extracted, this.root);
      this.logger.info("[runtime] Python 3.12.15 установлен для umu");
    } finally { fs.rmSync(staging, { recursive: true, force: true }); }
  }

  async prepareEnvironment(env) {
    const python = await this.ensure(env);
    return Object.assign({}, env, {
      PATH: [path.dirname(python), env.PATH || ""].join(path.delimiter),
      PYTHONNOUSERSITE: "1",
    });
  }
}

module.exports = PythonRuntime;
module.exports.DISTRIBUTION = DISTRIBUTION;
module.exports.compatiblePython = compatiblePython;
