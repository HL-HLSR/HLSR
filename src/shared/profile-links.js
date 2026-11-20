"use strict";

// Ссылки в профиле: на YouTube-канал и на Twitch-канал. Человек вводит как
// удобно — «@ник», «ник», «youtube.com/@ник», полный адрес с http или без,
// — а на сервер уходит полный https-адрес канала. Сервер
// (server/backend/src/Profile/Customization.php) принимает только https и
// только хосты youtube.com/youtu.be и twitch.tv, поэтому здесь адрес
// приводится к его виду, а всё остальное отклоняется сразу, ещё до сохранения.

const YOUTUBE_HOSTS = ["youtube.com", "www.youtube.com", "m.youtube.com"];
const TWITCH_HOSTS = ["twitch.tv", "www.twitch.tv", "m.twitch.tv"];

// Ник YouTube: 3–30 знаков, буквы, цифры, «_», «-», «.».
const YOUTUBE_HANDLE = /^[A-Za-z0-9_.-]{3,30}$/;
const YOUTUBE_CHANNEL_ID = /^UC[A-Za-z0-9_-]{22}$/;
const YOUTUBE_LEGACY = /^[A-Za-z0-9_.-]{1,100}$/;
// Ник Twitch: до 25 знаков, буквы, цифры и «_» (старые ники бывают короче 4).
const TWITCH_NAME = /^[A-Za-z0-9_]{1,25}$/;
// Служебные страницы Twitch, а не каналы.
const TWITCH_RESERVED = ["directory", "videos", "settings", "subscriptions", "inventory", "wallet", "drops",
  "search", "downloads", "jobs", "p", "store", "turbo", "prime", "login", "signup", "following", "friends"];

function parse(value, hosts) {
  const text = value.trim();
  if (!text) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(text) ? text : "https://" + text;
  let url;
  try { url = new URL(withScheme); } catch (error) { return null; }
  if ((url.protocol !== "https:" && url.protocol !== "http:") || url.username || url.password || url.port) return null;
  if (hosts.indexOf(url.hostname.toLowerCase()) === -1) return null;
  return url.pathname.split("/").filter(Boolean).map((part) => {
    try { return decodeURIComponent(part); } catch (error) { return part; }
  });
}

// Возвращает полный адрес канала, "" для пустого поля или null, если
// это не канал YouTube.
function normalizeYoutube(value) {
  const text = typeof value === "string" ? value.trim() : "";
  if (!text) return "";
  // «@ник» или просто «ник» (без «/» и не адрес самого YouTube).
  if (text[0] === "@" || (text.indexOf("/") === -1 && YOUTUBE_HOSTS.indexOf(text.toLowerCase()) === -1)) {
    const handle = text.replace(/^@/, "");
    return YOUTUBE_HANDLE.test(handle) ? "https://www.youtube.com/@" + handle : null;
  }
  const parts = parse(text, YOUTUBE_HOSTS);
  if (!parts || !parts.length) return null;
  const [first, second] = parts;
  if (first[0] === "@") {
    const handle = first.slice(1);
    return YOUTUBE_HANDLE.test(handle) ? "https://www.youtube.com/@" + handle : null;
  }
  if (first === "channel" && YOUTUBE_CHANNEL_ID.test(second || "")) return "https://www.youtube.com/channel/" + second;
  if ((first === "c" || first === "user") && YOUTUBE_LEGACY.test(second || "")) return "https://www.youtube.com/" + first + "/" + second;
  return null;
}

function normalizeTwitch(value) {
  const text = typeof value === "string" ? value.trim() : "";
  if (!text) return "";
  if (TWITCH_NAME.test(text.replace(/^@/, ""))) {
    const name = text.replace(/^@/, "");
    return TWITCH_RESERVED.indexOf(name.toLowerCase()) === -1 ? "https://www.twitch.tv/" + name : null;
  }
  const parts = parse(text, TWITCH_HOSTS);
  if (!parts || !parts.length || !TWITCH_NAME.test(parts[0])) return null;
  if (TWITCH_RESERVED.indexOf(parts[0].toLowerCase()) !== -1) return null;
  return "https://www.twitch.tv/" + parts[0];
}

function normalize(field, value) {
  if (field === "youtube") return normalizeYoutube(value);
  if (field === "twitch") return normalizeTwitch(value);
  return null;
}

// Подпись канала в профиле: ник вместо названия сервиса. У канала,
// сохранённого по ID (UC…), ника в адресе нет — тогда остаётся название
// сервиса.
function channelName(field, value) {
  const url = normalize(field, value);
  if (!url) return null;
  const parts = new URL(url).pathname.split("/").filter(Boolean);
  if (field === "twitch") return parts[0];
  if (parts[0][0] === "@") return parts[0];
  if (parts[0] === "c" || parts[0] === "user") return parts[1];
  return null;
}

module.exports = { normalize, normalizeYoutube, normalizeTwitch, channelName };
