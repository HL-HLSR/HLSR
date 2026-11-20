"use strict";

const achievements = require("../profile/achievements");
const nickStyles = require("../../shared/nick-styles");
const avatarFrames = require("../../shared/avatar-frames");
const customBadges = require("../../shared/custom-badges");
const anniversaryBadges = require("../../shared/anniversary-badges");

const GAMES = ["ghosting-renewed", "hl2-2707", "hl2-4260", "hl2-5288", "hl1-won"];
const BADGES = ["developer", "verified", "bughunter", "booster"].concat(anniversaryBadges.IDS);
const LAUNCHER_IDS = { "ghosting-renewed": "218", "hl2-2707": "220", "hl2-4260": "220", "hl2-5288": "220", "hl1-won": "70" };

function number(value) {
  return Number.isSafeInteger(value) && value >= 0 && value <= 1261440000 ? value : 0;
}

function text(value, max) { return typeof value === "string" ? value.slice(0, max) : ""; }

function date(value) {
  return typeof value === "string" && Number.isFinite(Date.parse(value)) ? value : null;
}

// boosterSince и supporterSince приходят в unix-секундах (а не строкой, как
// остальные даты): это начало буста и первой оплаты. Не число — null.
function since(value) {
  return Number.isSafeInteger(value) && value > 0 && value <= 253402300799 ? value : null;
}

// Страница забега на speedrun.com — /<игра>/runs/<id>. Сервер раньше
// отдавал /runs/<id> без игры, а такой страницы нет; для этих записей
// остаётся служебная ссылка, которую при открытии разрешает API.
const RUN_PAGE = /^https:\/\/www\.speedrun\.com\/[A-Za-z0-9_-]{1,64}\/runs?\/([a-z0-9]{8})$/i;

function runUrl(run) {
  const match = typeof run.url === "string" ? RUN_PAGE.exec(run.url) : null;
  return match && match[1] === run.id ? run.url : "https://www.speedrun.com/runs/" + run.id;
}

// Онлайн и текущая игра. Сервер считает человека в сети, пока лаунчер
// синхронизируется (раз в 30 с), а игру — пока идут пульсы игровой сессии.
function presence(raw) {
  const source = raw && typeof raw === "object" ? raw : {};
  const gameId = GAMES.indexOf(source.gameId) !== -1 ? source.gameId : null;
  return { online: source.online === true || gameId !== null, gameId };
}

// Рейтинг профиля: сумма лайков и дизлайков (может быть отрицательной) и
// пометка «сомнительный», которую ставит сервер.
function rating(raw) {
  const count = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : 0);
  if (!raw || typeof raw !== "object") return { score: 0, likes: 0, dislikes: 0, dubious: false };
  const likes = count(raw.likes), dislikes = count(raw.dislikes);
  return { score: likes - dislikes, likes, dislikes, dubious: raw.dubious === true };
}

// Свой голос за чужой профиль: 1, -1 или 0 и когда его можно поменять.
function vote(raw) {
  if (!raw || typeof raw !== "object") return { value: 0, nextAt: null };
  return { value: [1, -1].indexOf(raw.value) !== -1 ? raw.value : 0, nextAt: date(raw.nextAt) };
}

function plainObject(value) {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

// Права поддержавшего проект (только свой профиль, сервер на Go): styles —
// можно выбрать заготовку подсветки; subscriber — была ли подписка, активна
// ли она и до какого числа (until — конец последнего периода, бывает null).
// Ключа нет — права нет: так отвечают и сервер без поддержки, и сервер на PHP.
function perks(raw) {
  if (!plainObject(raw)) return { styles: false, subscriber: null };
  const sub = raw.subscriber;
  return { styles: raw.styles === true, subscriber: plainObject(sub) ? { active: sub.active === true, until: date(sub.until) } : null };
}

// Выбранная заготовка «подсветка ника + рамка» — её номер или null.
function stylePreset(value) {
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}

// Привязка Telegram к боту HLSR (только свой профиль): имя пользователя без
// «@» или null, если его нет. Ключа нет — не привязан.
const TELEGRAM_NAME = /^[A-Za-z0-9_]{1,64}$/;

function telegram(raw) {
  if (!plainObject(raw) || raw.linked !== true) return { linked: false, username: null };
  const name = typeof raw.username === "string" ? raw.username.trim() : "";
  return { linked: true, username: TELEGRAM_NAME.test(name) ? name : null };
}

// Привязка Discord (только свой профиль; не путать с details.discord — это
// имя, которое игрок вписал сам): имя до 64 символов и начало буста.
function discordLink(raw) {
  if (!plainObject(raw) || raw.linked !== true) return { linked: false, username: null, boosterSince: null };
  const name = typeof raw.username === "string" ? raw.username.trim() : "";
  return { linked: true, username: name && name.length <= 64 ? name : null, boosterSince: since(raw.boosterSince) };
}

function normalize(raw) {
  if (!raw || typeof raw !== "object" || !/^7656119\d{10}$/.test(raw.steamId64)) return null;
  const games = {};
  for (const id of GAMES) {
    const game = raw.games && raw.games[id];
    if (!game || typeof game !== "object") continue;
    games[id] = { gameId: id, totalPlaySeconds: number(game.totalPlaySeconds), launchCount: number(game.launchCount),
      lastLaunchAt: date(game.lastLaunchAt), lastExitAt: date(game.lastExitAt) };
  }
  const source = raw.details && typeof raw.details === "object" ? raw.details : {};
  const details = {};
  for (const key of ["status", "pronouns", "country", "discord", "youtube", "twitch", "avatarUrl"]) details[key] = text(source[key], 256);
  // Флаг есть только у тех, кто хоть раз его менял: по отсутствию лаунчер
  // понимает, что локальную настройку ещё надо отправить на сервер.
  if (typeof source.hideSteamId === "boolean") details.hideSteamId = source.hideSteamId;
  // Приватность в списках игроков: скрыть себя и скрыть, во что играешь.
  for (const key of ["hideFromPlayers", "hideGame"]) details[key] = source[key] === true;
  // Достижения, открытые событием (и секретные), и выданные сервером: поле
  // называется secrets с первой пасхалки.
  details.secrets = (Array.isArray(source.secrets) ? source.secrets : []).filter((id) => achievements.STORED_IDS.indexOf(id) !== -1);
  if (source.speedrun && /^[a-z0-9]{8}$/i.test(source.speedrun.id))
    details.speedrun = { id: source.speedrun.id, name: text(source.speedrun.name, 64) };
  if (source.pinnedRun && /^[a-z0-9]{8}$/i.test(source.pinnedRun.id) && Number.isFinite(source.pinnedRun.seconds))
    details.pinnedRun = { id: source.pinnedRun.id, seconds: Math.max(0, source.pinnedRun.seconds),
      game: text(source.pinnedRun.game, 96), category: text(source.pinnedRun.category, 96),
      place: Number.isSafeInteger(source.pinnedRun.place) && source.pinnedRun.place > 0 ? source.pinnedRun.place : null,
      url: runUrl(source.pinnedRun) };
  // Свои значки из админки приходят с описанием; значок без описания
  // показать нечем, и он отсеивается вместе с неизвестными встроенными.
  const custom = customBadges.normalize(raw.customBadges);
  // subscriber — подписка активна сейчас: значок у ника и голубая рамка
  // профиля. Сервер кладёт ключ в любую карточку, только когда это так.
  return { steamId64: raw.steamId64, personaName: text(raw.personaName, 64), registeredAt: date(raw.registeredAt),
    badges: (Array.isArray(raw.badges) ? raw.badges : []).filter((id) => BADGES.indexOf(id) !== -1 || custom.some((badge) => badge.id === id)),
    customBadges: custom, nick: nickStyles.normalize(raw.nick), subscriber: raw.subscriber === true, boosterSince: since(raw.boosterSince), supporterSince: since(raw.supporterSince), avatarFrame: avatarFrames.normalize(raw.avatarFrame), presence: presence(raw.presence), games, details, rating: rating(raw.rating), deletionAt: date(raw.deletionAt), generationId: text(raw.generationId, 32),
    perks: perks(raw.perks), stylePreset: stylePreset(raw.stylePreset), telegram: telegram(raw.telegram), discordLink: discordLink(raw.discordLink) };
}

// Строка списка игроков на главной (/v2/runner/players): карточка, место
// в рейтинге и число достижений, которое посчитал сервер. Игра в онлайне —
// с id лаунчера, как в state().
function listEntry(raw) {
  const profile = normalize(raw);
  if (!profile) return null;
  const count = (value) => (Number.isSafeInteger(value) && value > 0 ? value : null);
  profile.presence.launcherId = profile.presence.gameId ? LAUNCHER_IDS[profile.presence.gameId] : null;
  return Object.assign(profile, { rank: count(raw.rank), achievementCount: count(raw.achievementCount) || 0 });
}

// localSecrets — достижения, открытые на этом компьютере и ещё не дошедшие
// до сервера: достижение видно сразу, даже без сети. localPresence — свой
// онлайн: его лаунчер знает сам, без задержки сервера. localTimes — когда
// получены свои достижения (achievement-times.json), у чужих их нет.
// viewerSecrets — открытое у смотрящего, когда профиль чужой: его секретные,
// которых смотрящий не открыл, показываются замком.
function state(profile, localAccount, connected, localSecrets, localPresence, localTimes, viewerSecrets) {
  const empty = { account: { signedIn: false }, firstRunAt: null, achievements: [],
    summary: { unlocked: 0, total: achievements.ACHIEVEMENTS.length - achievements.BONUS_IDS.length, percent: 0 },
    stats: { totalSeconds: 0, gamesPlayed: 0, launches: 0, best: null }, badges: [], customBadges: [], nick: null, subscriber: false, boosterSince: null, supporterSince: null, avatarFrame: null, details: {}, deletionAt: null,
    rating: rating(null),
    presence: { online: false, gameId: null, launcherId: null },
    perks: perks(null), stylePreset: null, telegram: telegram(null), discordLink: discordLink(null) };
  if (!profile) return empty;
  const secrets = (profile.details.secrets || []).concat(Array.isArray(localSecrets) ? localSecrets : []);
  const all = achievements.evaluate({ games: profile.games, firstRunAt: profile.registeredAt, unlocked: secrets, times: localTimes });
  const list = achievements.order(achievements.mask(all, viewerSecrets || secrets));
  const best = achievements.bestGame(profile.games);
  const sameLocal = localAccount && localAccount.steamId64 === profile.steamId64;
  const accountId = BigInt(profile.steamId64) - BigInt("76561197960265728");
  // Свой онлайн — это связь с HLSR: без неё друзья не видят ни онлайна, ни
  // игры, хотя игра на этом компьютере и запущена.
  const shown = localPresence
    ? { online: localPresence.online === true, gameId: presence(localPresence).gameId }
    : presence(profile.presence);
  return { account: { signedIn: true, steamId64: profile.steamId64, steamId2: "STEAM_0:" + String(accountId % BigInt(2)) + ":" + String(accountId / BigInt(2)),
    personaName: profile.personaName || (sameLocal ? localAccount.personaName : "") || profile.steamId64,
    avatarDataUri: (sameLocal && localAccount.avatarDataUri) || profile.details.avatarUrl || null,
    connection: connected ? "connected" : "offline" },
    firstRunAt: profile.registeredAt, achievements: list, summary: achievements.summarize(all),
    stats: { totalSeconds: Object.values(profile.games).reduce((sum, game) => sum + game.totalPlaySeconds, 0),
      gamesPlayed: Object.values(profile.games).filter((game) => game.totalPlaySeconds > 0).length,
      launches: achievements.totalLaunches(profile.games),
      best: best ? Object.assign({}, best, { launcherId: LAUNCHER_IDS[best.gameId] }) : null },
    badges: profile.badges, customBadges: profile.customBadges || [], nick: profile.nick, subscriber: profile.subscriber === true, boosterSince: since(profile.boosterSince), supporterSince: since(profile.supporterSince), avatarFrame: profile.avatarFrame || null, details: profile.details, deletionAt: profile.deletionAt, games: profile.games,
    rating: profile.rating || rating(null),
    presence: Object.assign(shown, { launcherId: shown.gameId ? LAUNCHER_IDS[shown.gameId] : null }),
    // Разбор того же вида повторяется: сюда может прийти и сырой профиль.
    perks: perks(profile.perks), stylePreset: stylePreset(profile.stylePreset), telegram: telegram(profile.telegram), discordLink: discordLink(profile.discordLink) };
}

module.exports = { normalize, listEntry, state, rating, vote, date, runUrl, RUN_PAGE, GAMES };
