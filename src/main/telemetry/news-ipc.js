"use strict";

const contract = require("../../shared/news-contract");

function externalUrl(raw) {
  if (typeof raw !== "string" || raw.length > 2048 || Array.from(raw).some((c) => c.charCodeAt(0) <= 32 || c.charCodeAt(0) === 127)) throw new Error("Неверный адрес новости");
  const url = new URL(raw);
  if (url.protocol !== "https:" || !url.hostname || url.username || url.password || url.port) throw new Error("Неверный адрес новости");
  return url;
}

function bind(ipcMain, { client, profile, confirmLink, openExternal }) {
  const handles = {
    "public-news-react": async (postId, reaction) => {
      if (!contract.validPostId(postId) || typeof reaction !== "string" || !Object.hasOwn(contract.REACTIONS, reaction)) throw new Error("Неизвестная реакция или пост");
      const reply = await profile.action("news/react", { postId, reaction });
      if (reply.postId !== postId || !reply.reactions || !Array.isArray(reply.mine)) throw new Error("Неверный ответ ленты");
      const reactions = contract.counts(reply.reactions), mine = contract.mine(reply.mine);
      client.updateReactions(postId, reactions);
      if (mine.length && typeof profile.achieve === "function") profile.achieve("community-voice");
      return { status: "ok", postId, reactions, mine };
    },
    "public-news-mine": async (ids) => {
      if (!Array.isArray(ids) || ids.length > 40 || !ids.every(contract.validPostId)) throw new Error("Неверный список постов");
      const reply = await profile.action("news/mine", { postIds: [...new Set(ids)] });
      const mine = {};
      for (const id of ids) mine[id] = contract.mine(reply.mine && reply.mine[id]);
      return { status: "ok", mine };
    },
    "public-news-open-link": async (raw) => {
      const url = externalUrl(raw);
      if (!await confirmLink(url.hostname, url.href)) return { status: "cancelled" };
      await openExternal(url.href);
      return { status: "ok" };
    },
  };
  for (const channel of Object.keys(handles)) ipcMain.handle(channel, async (event, ...args) => {
    try { return await handles[channel](...args); }
    catch (error) { return { status: "error", code: error.code || "NEWS_FAILED" }; }
  });
}

module.exports = { bind, externalUrl };
