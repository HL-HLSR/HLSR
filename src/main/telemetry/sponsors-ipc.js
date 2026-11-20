"use strict";

const { SECTIONS, SUPPORT_KEYS, botUrl } = require("./sponsors-client");
const { externalUrl } = require("./news-ipc");

// Окно «Спонсорство» присылает только раздел и номер записи (или ключ
// кнопки). Адрес берётся из последнего списка клиента: окно не может
// подсунуть свой.
function invalid() {
  return new Error("Неверная ссылка спонсора");
}

function bind(ipcMain, client, options) {
  const { openExternal, confirmLink } = options || {};

  const handles = {
    // Свой адрес записи: после подтверждения, как у ссылок новостей.
    "sponsor-open-link": async (target) => {
      if (!target || typeof target !== "object" || !SECTIONS.includes(target.section) || !Number.isInteger(target.index)) throw invalid();
      const list = client.lastList();
      const entry = list && list[target.section][target.index];
      if (!entry || !entry.link || entry.link.mode !== "url") throw invalid();
      let url;
      try { url = externalUrl(entry.link.url); } catch (error) { throw invalid(); }
      if (!await confirmLink(url.hostname, url.href)) return { status: "cancelled" };
      await openExternal(url.href);
      return { status: "ok" };
    },
    // Кнопки поддержки, без подтверждения. Оплата идёт через бота: он
    // сначала привязывает Telegram к Steam, потом даёт кнопку Tribute.
    // «Подписка» и «Донат» открывают бота с ?start=subscribe|donate, «bot» —
    // бота как есть или с кодом привязки (?start=<6 цифр> из
    // /v2/runner/telegram/code); адреса Tribute из links лаунчер не
    // открывает. «Привязать Telegram» в настройках профиля работает и без
    // открытого окна «Спонсорство»: списка ещё нет — main сначала
    // запрашивает его сам.
    "support-open": async (key, code) => {
      if (typeof key !== "string" || !SUPPORT_KEYS.includes(key)) throw invalid();
      if (code !== undefined && (key !== "bot" || typeof code !== "string" || !/^\d{6}$/.test(code))) throw invalid();
      if (!client.lastList()) await client.get();
      const list = client.lastList();
      const bot = list && Object.hasOwn(list.links, "bot") ? list.links.bot : null;
      const url = botUrl(bot, key === "bot" ? code || null : key);
      if (!url) throw invalid();
      await openExternal(url);
      return { status: "ok" };
    },
  };

  for (const channel of Object.keys(handles)) ipcMain.handle(channel, async (event, ...args) => {
    try { return await handles[channel](...args); }
    catch (error) { return { status: "error", code: "SPONSOR_LINK_INVALID" }; }
  });
}

module.exports = { bind };
