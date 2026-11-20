"use strict";

const config = require("./config");
const transport = require("./transport");
const { parseAuthorProfile, parseProfileCard } = require("./news-client");
// Разделы и ключи ссылок — общие с окном (SponsorsModal.vue), один источник.
const { SECTIONS, SUPPORT_KEYS } = require("../../shared/sponsor-sections");

// Окно «Спонсорство». Новый сервер отдаёт /v2/public/sponsors: три раздела
// (активные подписчики, донаты, прошлые) и ссылки Tribute/бота. Сервер на PHP
// знает только /v1: на 404 клиент откатывается на него, и окно показывает
// прежний список. Другие ошибки /v2 (сеть, 5xx) на /v1 не откатываются: это
// сбой, а не старый сервер.
const CACHE_MS = 60 * 1000; // как max-age сервера для /v2
const LEGACY_CACHE_MS = 5 * 60 * 1000;
const MAX_ITEMS = 200;
const AMOUNT_MAX = 100000000;
// Telegram и Tribute: на эти хосты могут вести ссылки links сервера.
const SUPPORT_HOSTS = ["t.me", "tribute.tg", "web.tribute.tg"];
// Бот — только Telegram.
const BOT_HOST = "t.me";
const ENTRY_URL_MAX = 255;
const SUPPORT_URL_MAX = 512;

function text(value, max) {
  return typeof value === "string" && Array.from(value).length <= max ? value.trim() : null;
}

function parseSponsor(raw) {
  if (!raw || typeof raw !== "object") return null;
  const id = typeof raw.id === "string" && /^[1-9][0-9]{0,18}$/.test(raw.id) ? raw.id : null;
  const name = text(raw.name, 64);
  const note = raw.note === undefined ? "" : text(raw.note, 160);
  const amount = Number.isInteger(raw.amount) && raw.amount >= 0 && raw.amount <= AMOUNT_MAX ? raw.amount : null;
  if (!id || !name || note === null || amount === null) return null;
  return { id, name, note, amount, profile: parseAuthorProfile(raw.profile) };
}

// https-адрес без учётных данных и порта, без пробелов и управляющих знаков.
// Возвращает разобранный URL или null.
function httpsUrl(raw, max) {
  if (typeof raw !== "string" || !raw || raw.length > max) return null;
  if (Array.from(raw).some((c) => c.charCodeAt(0) <= 32 || c.charCodeAt(0) === 127)) return null;
  try {
    const url = new URL(raw);
    return url.protocol === "https:" && url.hostname && !url.username && !url.password && !url.port ? url : null;
  } catch (error) {
    return null;
  }
}

// Ссылка записи: { mode: "none" } | { mode: "profile", steamId64 } |
// { mode: "url", url }. Всё непроверенное превращается в none.
function safeLink(mode, value) {
  if (mode === "profile") return typeof value === "string" && /^7656119[0-9]{10}$/.test(value) ? { mode, steamId64: value } : { mode: "none" };
  if (mode === "url") {
    const url = httpsUrl(value, ENTRY_URL_MAX);
    return url ? { mode, url: url.href } : { mode: "none" };
  }
  return { mode: "none" };
}

function parseLink(raw) {
  if (!raw || typeof raw !== "object") return { mode: "none" };
  return safeLink(raw.mode, raw.mode === "profile" ? raw.steamId64 : raw.url);
}

// Адрес кнопки поддержки: https и один из хостов Telegram/Tribute. Проверка
// нужна и при разборе ответа, и перед открытием.
function supportUrl(raw) {
  const url = httpsUrl(raw, SUPPORT_URL_MAX);
  return url && SUPPORT_HOSTS.includes(url.hostname) ? url.href : null;
}

// Адрес бота для кнопок: https://t.me без учётных данных и порта; start —
// с какого сценария бот начинает (subscribe, donate) или null — бот как есть.
// Оплата идёт через бота: он сначала привязывает Telegram к Steam и только
// потом даёт кнопку Tribute, поэтому адреса Tribute лаунчер сам не открывает.
function botUrl(raw, start) {
  const url = httpsUrl(raw, SUPPORT_URL_MAX);
  if (!url || url.hostname !== BOT_HOST) return null;
  if (start) url.searchParams.set("start", start);
  return url.href;
}

// Бот принимается только с адресом, который откроет support-open (хост
// ровно t.me): иначе окно показало бы кнопки оплаты, которые ничего не
// открывают.
function parseLinks(raw) {
  const links = {};
  if (!raw || typeof raw !== "object") return links;
  for (const key of SUPPORT_KEYS) {
    if (!Object.hasOwn(raw, key)) continue;
    const url = key === "bot" ? botUrl(raw[key], null) : supportUrl(raw[key]);
    if (url) links[key] = url;
  }
  return links;
}

// Запись любого из трёх разделов. amount — только у донатов. Пустые name и
// personaName допустимы: так сервер передаёт скрытый ник.
function parseEntry(raw, section) {
  if (!raw || typeof raw !== "object") return null;
  const name = raw.name === undefined ? "" : text(raw.name, 64);
  const note = raw.note === undefined ? "" : text(raw.note, 160);
  let amount = 0;
  if (section === "donors") {
    amount = raw.amountRub;
    if (!Number.isInteger(amount) || amount < 0 || amount > AMOUNT_MAX) return null;
  }
  if (name === null || note === null) return null;
  const since = typeof raw.since === "string" && /^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(raw.since) ? raw.since : "";
  return { profile: parseProfileCard(raw.card), name, amount, note, since, link: parseLink(raw.link) };
}

// Тело /v2/public/sponsors -> { links, active, donors, past } или null, если
// это не тот ответ (нет какого-то из трёх разделов).
function parseList(raw) {
  if (!raw || typeof raw !== "object" || !SECTIONS.every((section) => Array.isArray(raw[section]))) return null;
  const list = { links: parseLinks(raw.links) };
  for (const section of SECTIONS) list[section] = raw[section].slice(0, MAX_ITEMS).map((entry) => parseEntry(entry, section)).filter(Boolean);
  return list;
}

function emptyList() {
  return { links: {}, active: [], donors: [], past: [] };
}

class SponsorsClient {
  constructor(options) {
    const opts = options || {};
    this.endpoint = opts.endpoint || `${config.API_BASE}/v2/public/sponsors`;
    this.legacyEndpoint = opts.legacyEndpoint || `${config.API_BASE}/v1/public/sponsors`;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.request = opts.request || transport.request;
    this.now = opts.now || (() => Date.now());
    // Последний удачный ответ: либо v2 (list), либо v1 (legacy), не оба.
    this.list = null;
    this.listAt = 0;
    this.legacy = null;
    this.legacyAt = 0;
    this.pending = null;
  }

  // v2: { status: "ok", version: 2, links, active, donors, past };
  // v1: { status: "ok", version: 1, sponsors }. При ошибке status "error" и
  // последний удачный ответ той же версии (или пустой), без исключений.
  async get(options) {
    const force = options && options.force === true;
    if (!force) {
      if (this.list && this.now() - this.listAt < CACHE_MS) return this.answer(2, "ok");
      if (this.legacy && this.now() - this.legacyAt < LEGACY_CACHE_MS) return this.answer(1, "ok");
    }
    if (!this.pending) this.pending = this.fetch().finally(() => { this.pending = null; });
    return this.pending;
  }

  // Последний список /v2 — единственный источник адресов для sponsor-open-link
  // и support-open: окно присылает лишь раздел и номер записи. null, если
  // /v2 не отвечал или сервер старый.
  lastList() {
    return this.list;
  }

  answer(version, status) {
    if (version === 1) return { status, version: 1, sponsors: this.legacy || [] };
    return { status, version: 2, ...(this.list || emptyList()) };
  }

  async getJson(endpoint) {
    const response = await this.request(endpoint, {
      method: "GET",
      headers: {},
      timeoutMs: config.ONLINE.timeoutMs,
      maxResponseBytes: 512 * 1024,
      launcherVersion: this.launcherVersion,
    });
    if (response.status < 200 || response.status >= 300) throw new Error(`HTTP_${response.status}`);
    return JSON.parse(response.body);
  }

  async fetch() {
    try {
      const list = parseList(await this.getJson(this.endpoint));
      if (!list) throw new Error("BAD_PAYLOAD");
      this.list = list;
      this.listAt = this.now();
      this.legacy = null;
      return this.answer(2, "ok");
    } catch (error) {
      if (error && error.message === "HTTP_404") return this.fetchLegacy();
      return this.answer(2, "error");
    }
  }

  async fetchLegacy() {
    try {
      const data = await this.getJson(this.legacyEndpoint);
      if (!data || !Array.isArray(data.sponsors)) throw new Error("BAD_PAYLOAD");
      const seen = new Set();
      this.legacy = data.sponsors.slice(0, MAX_ITEMS).map(parseSponsor).filter((item) => item && !seen.has(item.id) && seen.add(item.id))
        .sort((a, b) => b.amount - a.amount);
      this.legacyAt = this.now();
      // Сервер откатился на старый: прежний список /v2 больше не показывается,
      // и его адреса открывать нельзя.
      this.list = null;
      return this.answer(1, "ok");
    } catch (error) {
      return this.answer(1, "error");
    }
  }
}

module.exports = SponsorsClient;
module.exports.parseSponsor = parseSponsor;
module.exports.parseList = parseList;
module.exports.safeLink = safeLink;
module.exports.supportUrl = supportUrl;
module.exports.botUrl = botUrl;
module.exports.SUPPORT_KEYS = SUPPORT_KEYS;
module.exports.SECTIONS = SECTIONS;
