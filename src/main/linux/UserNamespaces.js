"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFile } = require("child_process");

function execute(file, args, options = {}) {
  return new Promise((resolve, reject) => execFile(file, args, {
    encoding: "utf8", maxBuffer: 8192, timeout: 120000, ...options,
  }, (error, stdout, stderr) => {
    if (error) { error.message = `${error.message}\n${stderr || ""}`.trim(); reject(error); }
    else resolve(stdout);
  }));
}

function umuRoot(env) {
  const data = env.container === "flatpak" ? env.HOST_XDG_DATA_HOME : env.XDG_DATA_HOME;
  return path.join(env.UMU_FOLDERS_PATH || data || path.join(os.homedir(), ".local", "share"), "umu");
}

function appArmorRestricted() {
  try { return fs.readFileSync("/proc/sys/kernel/apparmor_restrict_unprivileged_userns", "utf8").trim() === "1"; }
  catch (error) { return false; }
}

class UserNamespaces {
  constructor(options = {}) {
    this.resourcesPath = options.resourcesPath;
    this.logger = options.logger || { info() {} };
    this.pending = null;
    this.isRestricted = options.isRestricted || appArmorRestricted;
    this.execute = options.execute || execute;
  }

  async ensure(env) {
    if (this.pending) return this.pending;
    this.pending = this.check(env).finally(() => { this.pending = null; });
    return this.pending;
  }

  async check(env) {
    if (!this.isRestricted()) return;
    const root = umuRoot(env);
    let bwrap = null;
    try {
      for (const name of fs.readdirSync(root).filter((entry) => /^steamrt\d+$/.test(entry))) {
        const candidate = path.join(root, name, "pressure-vessel", "libexec", "steam-runtime-tools-0", "srt-bwrap");
        if (fs.existsSync(candidate)) { bwrap = candidate; break; }
      }
    } catch (error) { /* umu has not downloaded its runtime yet */ }
    if (!bwrap) return;
    const args = ["--ro-bind", "/", "/", "--unshare-user", "--", "/bin/true"];
    try { await this.execute(bwrap, args, { env, timeout: 10000 }); return; }
    catch (error) { /* AppArmor may need a per-umu grant */ }
    const helper = path.join(this.resourcesPath, "linux", "install-userns-profile.sh");
    this.logger.info("[runtime] AppArmor блокирует umu; ОС запросит разрешение администратора для профиля HLSR");
    try {
      await this.execute("/usr/bin/pkexec", ["/bin/bash", helper, String(process.getuid()), root], { env });
      await this.execute(bwrap, args, { env, timeout: 10000 });
    } catch (error) {
      const failure = new Error(`AppArmor блокирует Steam Runtime. Для подготовки профиля HLSR требуется подтверждение администратора через PolicyKit (pkexec). ${error.message}`);
      failure.code = "LINUX_USERNS_BLOCKED";
      throw failure;
    }
  }
}

module.exports = UserNamespaces;
module.exports.umuRoot = umuRoot;
