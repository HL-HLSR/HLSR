"use strict";

const config = require("./config");
const transport = require("./transport");
const nickStyles = require("../../shared/nick-styles");
const avatarFrames = require("../../shared/avatar-frames");

// Заготовки «подсветка ника + рамка аватара», из которых поддержавший проект
// выбирает свою в настройках профиля (StylePresetPicker.vue). Сервер отдаёт
// видимые заготовки по порядку: [{ id, title, nickCss, frameCss }]. Сервер на
// PHP заготовок не знает (404) — тогда список пуст, и выбора нет.
//
// CSS проходит те же проверки, что оформление из профиля (nick-styles.js,
// avatar-frames.js). Заготовку, чей непустой CSS их не прошёл, лаунчер не
// показывает: предпросмотр разошёлся бы с тем, что увидят другие игроки.
const CACHE_MS = 5 * 60 * 1000; // как max-age сервера (300 с)
const MAX_PRESETS = 100;
const TITLE_MAX = 48; // style_presets.title VARCHAR(48)
const ID_MAX = 4294967295; // style_presets.id INT UNSIGNED
// eslint-disable-next-line no-control-regex
const CONTROL = /[\x00-\x1f\x7f]/;

// Проверенный CSS, "" — эту часть заготовка не меняет, null — заготовка не
// годится (CSS не строка или непустой CSS не прошёл проверку).
function presetCss(raw, sanitize) {
  if (raw === undefined || raw === null) return "";
  if (typeof raw !== "string") return null;
  if (!raw.trim()) return "";
  return sanitize(raw) || null;
}

function parsePreset(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const id = Number.isSafeInteger(raw.id) && raw.id > 0 && raw.id <= ID_MAX ? raw.id : null;
  const title = typeof raw.title === "string" ? raw.title.trim() : "";
  if (!id || !title || Array.from(title).length > TITLE_MAX || CONTROL.test(title)) return null;
  const nick = presetCss(raw.nickCss, nickStyles.sanitizeCss);
  const frame = presetCss(raw.frameCss, avatarFrames.sanitizeCss);
  if (nick === null || frame === null) return null;
  // Вид — как nick и avatarFrame профиля: так их рисуют NickName и AvatarFrame.
  return { id, title, nick: { style: null, css: nick }, frame: { css: frame } };
}

// Тело ответа -> список заготовок без повторов или null, если это не список.
function parseList(raw) {
  if (!Array.isArray(raw)) return null;
  const seen = new Set();
  return raw.slice(0, MAX_PRESETS).map(parsePreset).filter((item) => item && !seen.has(item.id) && seen.add(item.id));
}

class StylePresetsClient {
  constructor(options) {
    const opts = options || {};
    this.endpoint = opts.endpoint || `${config.API_BASE}/v2/public/style-presets`;
    this.launcherVersion = opts.launcherVersion || "0.0.0";
    this.request = opts.request || transport.request;
    this.now = opts.now || (() => Date.now());
    this.presets = null;
    this.presetsAt = 0;
    this.pending = null;
  }

  // { status: "ok", presets } — список (на старом сервере пустой);
  // { status: "error", presets } — сбой: последний полученный список или
  // пустой. Не бросает.
  async get(options) {
    const force = options && options.force === true;
    if (!force && this.presets && this.now() - this.presetsAt < CACHE_MS) return { status: "ok", presets: this.presets };
    if (!this.pending) this.pending = this.fetch().finally(() => { this.pending = null; });
    return this.pending;
  }

  async fetch() {
    try {
      const response = await this.request(this.endpoint, {
        method: "GET",
        headers: {},
        timeoutMs: config.ONLINE.timeoutMs,
        maxResponseBytes: 256 * 1024,
        launcherVersion: this.launcherVersion,
      });
      let presets;
      if (response.status === 404) presets = [];
      else if (response.status < 200 || response.status >= 300) throw new Error(`HTTP_${response.status}`);
      else presets = parseList(JSON.parse(response.body));
      if (!presets) throw new Error("BAD_PAYLOAD");
      this.presets = presets;
      this.presetsAt = this.now();
      return { status: "ok", presets };
    } catch (error) {
      return { status: "error", presets: this.presets || [] };
    }
  }
}

module.exports = StylePresetsClient;
module.exports.parseList = parseList;
