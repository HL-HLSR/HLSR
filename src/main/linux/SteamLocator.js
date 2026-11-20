"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const vdf = require("../game-delivery/vdf");
const { DeliveryError } = require("../game-delivery/errors");

const SOURCE_SDK_APP_ID = 243730;

class SteamLocator {
  constructor(options = {}) {
    this.fs = options.fs || fs;
    this.home = options.home || os.homedir();
    this.logger = options.logger || { info() {}, warn() {}, error() {} };
  }

  roots() {
    return [
      path.join(this.home, ".local/share/Steam"),
      path.join(this.home, ".steam/root"),
      path.join(this.home, ".steam/steam"),
      path.join(this.home, ".var/app/com.valvesoftware.Steam/data/Steam"),
    ];
  }

  locateSteam() {
    for (const candidate of this.roots()) {
      try {
        const root = this.fs.realpathSync(candidate);
        if (!this.fs.statSync(path.join(root, "steamapps")).isDirectory()) continue;
        const exe = ["/usr/bin/steam", path.join(root, "steam.sh")].find((file) => this.fs.existsSync(file));
        const flatpak = candidate.includes(".var/app/com.valvesoftware.Steam");
        return {
          installPath: root,
          steamExe: exe || (flatpak ? "/usr/bin/flatpak" : "steam"),
          steamArgs: exe || !flatpak ? [] : ["run", "com.valvesoftware.Steam"],
        };
      } catch (error) { /* next installation */ }
    }
    throw new DeliveryError("STEAM_NOT_INSTALLED", "Steam не найден в домашнем каталоге пользователя");
  }

  assertSteamRunning() {
    let entries = [];
    try { entries = this.fs.readdirSync("/proc"); } catch (error) { /* unavailable */ }
    for (const name of entries) {
      if (!/^\d+$/.test(name)) continue;
      try {
        const dir = `/proc/${name}`;
        if (this.fs.statSync(dir).uid !== process.getuid()) continue;
        const comm = this.fs.readFileSync(path.join(dir, "comm"), "utf8").trim();
        if (comm !== "steam" && comm !== "steamwebhelper") continue;
        return { pid: Number(name), source: "proc" };
      } catch (error) { /* process exited */ }
    }
    throw new DeliveryError("STEAM_NOT_RUNNING", "Steam не запущен");
  }

  steamLibraries(steam) {
    const out = [steam.installPath];
    const file = path.join(steam.installPath, "steamapps", "libraryfolders.vdf");
    let tree;
    try { tree = vdf.parse(this.fs.readFileSync(file, "utf8")); } catch (error) { return out; }
    const folders = vdf.getInsensitive(tree, "libraryfolders");
    if (!folders || typeof folders !== "object") return out;
    for (const key of Object.keys(folders)) {
      if (!/^\d+$/.test(key)) continue;
      const entry = folders[key];
      const value = typeof entry === "string" ? entry : vdf.getInsensitive(entry, "path");
      if (typeof value === "string" && path.isAbsolute(value) && !out.includes(value)) out.push(value);
    }
    return out;
  }

  findApp(steam, appId) {
    for (const library of this.steamLibraries(steam)) {
      const manifestPath = path.join(library, "steamapps", `appmanifest_${appId}.acf`);
      let tree;
      try { tree = vdf.parse(this.fs.readFileSync(manifestPath, "utf8")); } catch (error) { continue; }
      const state = vdf.getInsensitive(tree, "AppState");
      if (!state || Number(vdf.getInsensitive(state, "appid")) !== Number(appId)) continue;
      const installdir = vdf.getInsensitive(state, "installdir");
      if (typeof installdir !== "string" || /[\\/]|\.\./.test(installdir)) continue;
      const appPath = path.join(library, "steamapps", "common", installdir);
      if (!this.fs.existsSync(appPath)) continue;
      return { appId: Number(appId), library, manifestPath, installdir, appPath };
    }
    throw new DeliveryError("SOURCE_SDK_NOT_INSTALLED", `Приложение Steam ${appId} не установлено`);
  }

  preflight(appId = SOURCE_SDK_APP_ID) {
    const steam = this.locateSteam();
    const running = this.assertSteamRunning(steam);
    const app = this.findApp(steam, appId);
    return { steam: { ...steam, pid: running.pid, source: running.source }, app };
  }

  describeSteamOnly() {
    try {
      const steam = this.locateSteam();
      const running = this.assertSteamRunning(steam);
      return { ok: true, code: null, steam: { ...steam, pid: running.pid, source: running.source } };
    } catch (error) {
      return { ok: false, code: error.code || "STEAM_NOT_RUNNING", message: error.message };
    }
  }

  describeAvailability(appId) {
    try { return { ok: true, code: null, ...this.preflight(appId) }; }
    catch (error) { return { ok: false, code: error.code || "LAUNCH_FAILED", message: error.message }; }
  }
}

module.exports = SteamLocator;
module.exports.SOURCE_SDK_APP_ID = SOURCE_SDK_APP_ID;
