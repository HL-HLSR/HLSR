"use strict";

const fs = require("fs");
const path = require("path");
const vdf = require("../game-delivery/vdf");

const BASE = BigInt("76561197960265728");
const MAX_AVATAR_BYTES = 512 * 1024;
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function steamId64Of(accountId) {
  return String(BASE + BigInt(accountId));
}

function steamId2Of(accountId) {
  const account = BigInt(accountId);
  return `STEAM_0:${account % BigInt(2)}:${account / BigInt(2)}`;
}

class SteamAccount {
  constructor(options = {}) {
    this.locator = options.locator;
    this.fs = options.fs || fs;
    this.logger = options.logger || { info() {}, warn() {} };
    this.connection = "checking";
  }

  readUsers(root) {
    const file = path.join(root, "config", "loginusers.vdf");
    try {
      const tree = vdf.parse(this.fs.readFileSync(file, "utf8"));
      return vdf.getInsensitive(tree, "users") || {};
    } catch (error) { return {}; }
  }

  activeAccountId() {
    const id = this.describe().steamId64;
    return id ? Number(BigInt(id) - BASE) : null;
  }

  avatarOf(root, steamId64) {
    const file = path.join(root, "config", "avatarcache", `${steamId64}.png`);
    try {
      const stat = this.fs.statSync(file);
      if (!stat.isFile() || stat.size > MAX_AVATAR_BYTES) return null;
      const content = this.fs.readFileSync(file);
      if (!content.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) return null;
      return `data:image/png;base64,${content.toString("base64")}`;
    } catch (error) { return null; }
  }

  describe() {
    const empty = {
      signedIn: false, steamId64: null, steamId2: null,
      personaName: null, avatarDataUri: null, connection: this.connection,
    };
    let steam;
    try { steam = this.locator.locateSteam(); } catch (error) { return empty; }
    const users = this.readUsers(steam.installPath);
    const ids = Object.keys(users).filter((id) => /^7656119\d{10}$/.test(id));
    const id = ids.find((candidate) => String(vdf.getInsensitive(users[candidate], "MostRecent")) === "1") || ids[0];
    if (!id) return empty;
    const record = users[id];
    return {
      signedIn: this.connection === "connected",
      steamId64: id,
      steamId2: steamId2Of(BigInt(id) - BASE),
      personaName: vdf.getInsensitive(record, "PersonaName") || null,
      avatarDataUri: this.avatarOf(steam.installPath, id),
      connection: this.connection,
    };
  }

  refreshConnection() {
    const result = this.locator.describeSteamOnly();
    this.connection = result.ok ? "connected" : "offline";
    return this.connection;
  }
}

module.exports = SteamAccount;
module.exports.steamId2Of = steamId2Of;
module.exports.steamId64Of = steamId64Of;
module.exports.MAX_AVATAR_BYTES = MAX_AVATAR_BYTES;
