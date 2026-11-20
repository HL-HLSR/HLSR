"use strict";

const path = require("path");

const SteamLocator = require("./SteamLocator");

const STEAM_ID64_BASE = "76561197960265728";
const STEAM_ID64_LOW_DIGITS = 10;
const STEAM_ID64_LOW_LIMIT = Math.pow(10, STEAM_ID64_LOW_DIGITS);
const STEAM_ID64_HIGH = Number(STEAM_ID64_BASE.slice(0, -STEAM_ID64_LOW_DIGITS));
const STEAM_ID64_LOW = Number(STEAM_ID64_BASE.slice(-STEAM_ID64_LOW_DIGITS));

const MAX_ACCOUNT_ID = 0xffffffff;

if (!Number.isSafeInteger(STEAM_ID64_LOW + MAX_ACCOUNT_ID))
  throw new RangeError(
    `база SteamID64 ${STEAM_ID64_BASE} разложена так, что младшие ${STEAM_ID64_LOW_DIGITS} разрядов выходят за точную арифметику Number`
  );

const MAX_AVATAR_BYTES = 512 * 1024;

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function steamId64Of(accountId) {
  let low = STEAM_ID64_LOW + accountId;
  let high = STEAM_ID64_HIGH;

  if (low >= STEAM_ID64_LOW_LIMIT) {
    high += Math.floor(low / STEAM_ID64_LOW_LIMIT);
    low = low % STEAM_ID64_LOW_LIMIT;
  }

  return String(high) + String(low).padStart(STEAM_ID64_LOW_DIGITS, "0");
}

function steamId2Of(accountId) {
  return `STEAM_0:${accountId % 2}:${Math.floor(accountId / 2)}`;
}

class SteamAccount {
  constructor(options) {
    const opts = options || {};

    this.locator = opts.locator || null;
    this.fs = opts.fs || require("fs");
    this.registry = opts.registry || { read: SteamLocator.readRegistryValue };
    this.vdf = opts.vdf || require("./vdf");
    this.logger = opts.logger || { info() {}, warn() {} };

    this.lastKnown = null;

    this.connection = "checking";
  }

  describe() {
    const empty = {
      signedIn: false,
      steamId64: null,
      steamId2: null,
      personaName: null,
      avatarDataUri: null,
      connection: this.connection,
    };

    if (process.platform !== "win32") return empty;

    let steam = null;

    try {
      steam = this.locator ? this.locator.locateSteam() : null;
    } catch (error) {
      return empty;
    }

    const accountId = this.activeAccountId();
    if (!accountId) return this.remember(empty);

    const steamId64 = steamId64Of(accountId);

    const result = {
      signedIn: true,
      steamId64,
      steamId2: steamId2Of(accountId),
      personaName: this.personaNameOf(steam.installPath, steamId64),
      avatarDataUri: this.avatarOf(steam.installPath, steamId64),
      connection: empty.connection,
    };

    if (!result.avatarDataUri && this.lastKnown && this.lastKnown.steamId64 === steamId64)
      result.avatarDataUri = this.lastKnown.avatarDataUri;

    return this.remember(result);
  }

  remember(value) {
    if (value && value.signedIn) this.lastKnown = value;
    return value;
  }

  refreshConnection() {
    this.connection = this.measureConnection();
    return this.connection;
  }

  measureConnection() {
    if (process.platform !== "win32") return "unknown";
    if (!this.locator || typeof this.locator.describeSteamOnly !== "function") return "unknown";

    let state = null;

    try {
      state = this.locator.describeSteamOnly();
    } catch (error) {
      return "unknown";
    }

    if (!state) return "unknown";
    if (state.ok) return "connected";
    if (state.code === "STEAM_ELEVATED") return "elevated";

    return "offline";
  }

  activeAccountId() {
    let raw = null;

    try {
      raw = this.registry.read(SteamLocator.ACTIVE_PROCESS_KEY, "ActiveUser");
    } catch (error) {
      return null;
    }

    const accountId = Number(raw);
    if (!Number.isInteger(accountId) || accountId <= 0) return null;

    return accountId;
  }

  personaNameOf(installPath, steamId64) {
    const file = path.join(installPath, "config", "loginusers.vdf");

    let text;

    try {
      text = this.fs.readFileSync(file, "utf8");
    } catch (error) {
      return null;
    }

    let tree;

    try {
      tree = this.vdf.parse(text);
    } catch (error) {
      this.logger.warn(`[steam] loginusers.vdf не разобран: ${error.message}`);
      return null;
    }

    const users = this.vdf.getInsensitive(tree, "users");
    if (!users || typeof users !== "object") return null;

    const record = this.vdf.getInsensitive(users, steamId64);
    if (!record || typeof record !== "object") return null;

    const name = this.vdf.getInsensitive(record, "PersonaName");
    return typeof name === "string" && name.trim() ? name.trim() : null;
  }

  avatarOf(installPath, steamId64) {
    const file = path.join(installPath, "config", "avatarcache", `${steamId64}.png`);

    let stat = null;

    try {
      stat = this.fs.statSync(file);
    } catch (error) {
      return null;
    }

    if (!stat || !stat.isFile() || stat.size <= 0 || stat.size > MAX_AVATAR_BYTES) return null;

    let data;

    try {
      data = this.fs.readFileSync(file);
    } catch (error) {
      return null;
    }

    if (!Buffer.isBuffer(data) || data.length < PNG_SIGNATURE.length) return null;
    if (!data.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) return null;

    return `data:image/png;base64,${data.toString("base64")}`;
  }
}

module.exports = SteamAccount;
module.exports.steamId64Of = steamId64Of;
module.exports.steamId2Of = steamId2Of;
module.exports.MAX_AVATAR_BYTES = MAX_AVATAR_BYTES;
