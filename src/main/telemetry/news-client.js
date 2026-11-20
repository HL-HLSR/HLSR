"use strict";

const config = require("./config");
const transport = require("./transport");
const contract = require("../../shared/news-contract");
const nickStyles = require("../../shared/nick-styles");
const avatarFrames = require("../../shared/avatar-frames");
const { parseCrop } = require("../../shared/news-image-layout");

// Лента «Главной»: посты из админки сайта, мировые рекорды и свежие
// подтверждённые забеги speedrun.com.
// Запрашивается по требованию (открыли вкладку, нажали «Обновить»), а не
// по таймеру: новости меняются редко, сервер и так кеширует ответ.
const CACHE_MS = 2 * 60 * 1000;
const MAX_ITEMS = 40;
// Рекорды всех таблиц speedrun.com: по одному на сочетание сабкатегорий.
const MAX_RECORDS = 400;
// Страница забега — /<игра>/runs/<id>. Старый сервер присылал /runs/<id>
// без игры: такой страницы нет, и браузер открывал её только после запроса
// к API speedrun.com. Её адрес собирается из gameKey (аббревиатура игры на
// speedrun.com) — сразу, без запросов.
const RUN_URL = /^https:\/\/www\.speedrun\.com\/(?:([A-Za-z0-9_-]{1,64})\/)?runs?\/([A-Za-z0-9]{8})$/;

function text(value, max) {
  return typeof value === "string" && value.trim() !== "" && Array.from(value).length <= max ? value : null;
}

function isoDate(value) {
  return typeof value === "string" && !Number.isNaN(Date.parse(value)) ? new Date(value).toISOString() : null;
}

// Аватар Steam автора — только с CDN Steam, тот же список, что у сервера
// (SteamDirectory.php): лента не должна грузить картинки с чужих хостов.
const AVATAR_HOSTS = ["avatars.steamstatic.com", "avatars.akamai.steamstatic.com", "steamcdn-a.akamaihd.net"];

function avatarUrl(value) {
  if (typeof value !== "string" || !value || value.length > 256) return "";
  try {
    const url = new URL(value);
    return url.protocol === "https:" && AVATAR_HOSTS.indexOf(url.hostname) !== -1 && !url.username && !url.password ? url.href : "";
  } catch (error) { return ""; }
}

// Профиль HLSR автора поста: по нему лента рисует аватар и оформленный ник
// и открывает профиль. Нет профиля или он не прошёл проверку — пост
// остаётся с именем автора, как раньше.
function parseAuthorProfile(raw) {
  if (!raw || typeof raw !== "object" || typeof raw.steamId64 !== "string" || !/^7656119[0-9]{10}$/.test(raw.steamId64)) return null;
  return { steamId64: raw.steamId64, ...parseProfileCard(raw) };
}

// Карточка без SteamID (окно «Спонсорство», /v2): ник, аватар и оформление.
// Проверки те же, что у профиля автора; пустой ник (скрытый) допустим.
// subscriber — подписка активна сейчас (значок у ника); сервер кладёт ключ
// только тогда, и только true его включает.
function parseProfileCard(raw) {
  if (!raw || typeof raw !== "object") return null;
  return { personaName: text(raw.personaName, 64) || "", avatarUrl: avatarUrl(raw.avatarUrl), nick: nickStyles.normalize(raw.nick), avatarFrame: avatarFrames.normalize(raw.avatarFrame),
    subscriber: raw.subscriber === true };
}

function parseImage(raw, apiBase) {
  if (!raw || typeof raw !== "object" || typeof raw.url !== "string" || raw.url.length > 512
    || !Number.isInteger(raw.width) || !Number.isInteger(raw.height) || raw.width < 1 || raw.height < 1
    || raw.width > 8000 || raw.height > 8000 || raw.width * raw.height > 32000000) return null;
  try {
    const base = new URL(apiBase), url = new URL(raw.url, base);
    const local = base.protocol === "http:" && base.hostname === "127.0.0.1";
    if (url.origin !== base.origin || (!local && url.protocol !== "https:") || url.username || url.password || url.search || url.hash
      || !/^\/v1\/public\/news\/image\/[a-f0-9]{64}\.(png|jpg|webp)$/.test(url.pathname)) return null;
    const image = { url: url.href, width: raw.width, height: raw.height };
    const crop = parseCrop(raw.crop);
    if (crop) image.crop = crop;
    return image;
  } catch (error) { return null; }
}

function parseItem(raw, apiBase = config.API_BASE) {
  if (!raw || typeof raw !== "object") return null;
  const date = isoDate(raw.date);
  if (!date) return null;

  if (raw.kind === "post") {
    const id = typeof raw.id === "string" && /^[1-9][0-9]{0,18}$/.test(raw.id) ? raw.id : null;
    const title = text(raw.title, 160);
    const body = text(raw.body, 8000);
    const author = raw.author === undefined || raw.author === "" ? "" : text(raw.author, 80);
    if (author === null || (raw.images !== undefined && !Array.isArray(raw.images))
      || (raw.reactions !== undefined && (!raw.reactions || typeof raw.reactions !== "object" || Array.isArray(raw.reactions)))) return null;
    const images = (raw.images || []).slice(0, 6).map((img) => parseImage(img, apiBase)).filter(Boolean);
    const authorProfile = parseAuthorProfile(raw.authorProfile);
    return id && title && body ? { kind: "post", id, title, body, author, authorProfile, images, reactions: contract.counts(raw.reactions), date } : null;
  }

  // run — свежий подтверждённый забег (не рекорд), в той же форме, что рекорд.
  if (raw.kind === "record" || raw.kind === "run") {
    // profiles — SteamID64 профиля HLSR бегуна (speedrun.com привязан) или null, в порядке runners;
    // отбор имён тот же, поэтому пары собираются до него.
    const rawRunners = Array.isArray(raw.runners) ? raw.runners : [];
    const rawProfiles = Array.isArray(raw.profiles) && raw.profiles.length === rawRunners.length ? raw.profiles : null;
    const pairs = rawRunners.map((name, i) => [name, rawProfiles ? rawProfiles[i] : null]).filter(([name]) => text(name, 64)).slice(0, 8);
    const runners = pairs.map(([name]) => name);
    const profiles = pairs.map(([, id]) => (typeof id === "string" && /^\d{17}$/.test(id) ? id : null));
    const seconds = typeof raw.seconds === "number" && raw.seconds > 0 && raw.seconds <= 604800 ? raw.seconds : null;
    const game = text(raw.game, 96);
    const link = typeof raw.url === "string" ? RUN_URL.exec(raw.url) : null;
    if (!runners.length || seconds === null || !game || !link) return null;
    const gameKey = typeof raw.gameKey === "string" && /^[a-z0-9]{1,16}$/.test(raw.gameKey) ? raw.gameKey : "";
    const url = link[1] || !gameKey ? raw.url : `https://www.speedrun.com/${gameKey}/runs/${link[2]}`;
    return {
      kind: raw.kind,
      id: link[2],
      game,
      gameKey,
      category: text(raw.category, 96) || "",
      subcategory: text(raw.subcategory, 160) || "",
      runners,
      seconds,
      date,
      url,
      // Место в таблице (1–3) у свежего забега; у рекорда оно и так первое.
      place: raw.kind === "run" && Number.isInteger(raw.place) && raw.place >= 1 && raw.place <= 3 ? raw.place : null,
      profiles: profiles.some(Boolean) ? profiles : null,
    };
  }

  return null;
}

class NewsClient {
  constructor(options) {
    const opts = options || {};
    this.endpoint = opts.endpoint || `${config.API_BASE}/v1/public/news`;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.request = opts.request || transport.request;
    this.now = opts.now || (() => Date.now());
    this.cache = null;
    this.cachedAt = 0;
    this.pending = null;
    this.reactionUpdates = new Map();
  }

  updateReactions(id, raw) {
    const reactions = contract.counts(raw);
    this.reactionUpdates.set(id, { at: this.now(), reactions });
    if (this.cache) this.cache = this.cache.map((item) => item.kind === "post" && item.id === id ? Object.assign({}, item, { reactions }) : item);
  }

  // { status: "ok", items } или { status: "error", items } — при ошибке
  // остаётся последняя удачная лента, если она была.
  async get(options) {
    const force = options && options.force === true;
    if (!force && this.cache && this.now() - this.cachedAt < CACHE_MS) return { status: "ok", items: this.cache };
    if (!this.pending) this.pending = this.fetch().finally(() => { this.pending = null; });
    return this.pending;
  }

  async fetch() {
    try {
      const response = await this.request(this.endpoint, {
        method: "GET",
        headers: {},
        timeoutMs: config.ONLINE.timeoutMs,
        // Посты длиннее счётчиков онлайна: до 40 записей по 8000 символов.
        maxResponseBytes: 2 * 1024 * 1024,
        launcherVersion: this.launcherVersion,
      });
      if (response.status < 200 || response.status >= 300) throw new Error(`HTTP_${response.status}`);
      const data = JSON.parse(response.body);
      if (!data || !Array.isArray(data.items)) throw new Error("BAD_PAYLOAD");
      // Полный список рекордов сервер отдаёт отдельно (records); в items
      // их доля для старых лаунчеров.
      const records = Array.isArray(data.records) ? data.records.slice(0, MAX_RECORDS) : null;
      const raw = records ? data.items.slice(0, MAX_ITEMS).filter((item) => !item || item.kind !== "record").concat(records) : data.items.slice(0, MAX_ITEMS);
      const seen = new Set();
      this.cache = raw.map((item) => parseItem(item, this.endpoint)).filter((item) => {
        if (!item) return false;
        const key = item.kind + ":" + item.id;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      }).map((item) => {
        const recent = this.reactionUpdates.get(item.id);
        if (item.kind === "post" && recent && this.now() - recent.at < CACHE_MS) return Object.assign({}, item, { reactions: recent.reactions });
        return item;
      });
      for (const [id, update] of this.reactionUpdates) if (this.now() - update.at >= CACHE_MS) this.reactionUpdates.delete(id);
      this.cachedAt = this.now();
      return { status: "ok", items: this.cache };
    } catch (error) {
      return { status: "error", items: this.cache || [] };
    }
  }
}

module.exports = NewsClient;
module.exports.parseItem = parseItem;
module.exports.parseAuthorProfile = parseAuthorProfile;
module.exports.parseProfileCard = parseProfileCard;
