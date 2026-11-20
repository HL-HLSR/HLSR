"use strict";

const { ASSETS, GAME_TEXT, buttonUrls } = require("./constants");
const mapAssets = require("../game-presence/map-assets");

const KINDS = {
  LAUNCHER: "launcher",
  BROWSING: "browsing",
  READY: "ready",
  INSTALLING: "installing",
  UPDATING: "updating",
  REPAIRING: "repairing",
  PLAYING: "playing",
  GAME_MENU: "game_menu",
  GAME_MAP: "game_map",
  // Где игрок в лаунчере (раздел шапки или вкладка) и рендер демки — без игры.
  HOME: "home",
  PERSONALIZING: "personalizing",
  DEMOS: "demos",
  DEMO_RENDER: "demo_render",
  LIVESPLIT: "livesplit",
};

const GAME_KINDS = [KINDS.GAME_MENU, KINDS.GAME_MAP];

// Состояния без игры: в Discord — «HLSR Launcher» и само состояние.
const LAUNCHER_KINDS = [KINDS.LAUNCHER, KINDS.HOME, KINDS.PERSONALIZING, KINDS.DEMOS, KINDS.DEMO_RENDER, KINDS.LIVESPLIT];

// Раздел лаунчера (discord-rpc-activity из LauncherShell.vue) -> состояние.
const ACTIVITY_KINDS = {
  home: KINDS.HOME,
  personalization: KINDS.PERSONALIZING,
  demo: KINDS.DEMOS,
  livesplit: KINDS.LIVESPLIT,
};

const KIND_LIST = Object.keys(KINDS).map((k) => KINDS[k]);

const TEXT = {
  ru: {
    launcherDetails: "HLSR Launcher",
    [KINDS.LAUNCHER]: "В лаунчере",
    [KINDS.BROWSING]: "Выбирает сборку",
    [KINDS.READY]: "Готов к запуску",
    [KINDS.INSTALLING]: "Установка сборки",
    [KINDS.UPDATING]: "Обновление сборки",
    [KINDS.REPAIRING]: "Проверка файлов",
    [KINDS.PLAYING]: "В игре",
    [KINDS.HOME]: "На главной",
    [KINDS.PERSONALIZING]: "Настраивает игру",
    [KINDS.DEMOS]: "Смотрит демки",
    [KINDS.DEMO_RENDER]: "Рендерит демку",
    [KINDS.LIVESPLIT]: "Настраивает LiveSplit",
    buttons: { site: "Сайт HLSR", discord: "Discord" },
  },
  en: {
    launcherDetails: "HLSR Launcher",
    [KINDS.LAUNCHER]: "In the launcher",
    [KINDS.BROWSING]: "Selecting a build",
    [KINDS.READY]: "Ready to play",
    [KINDS.INSTALLING]: "Installing build",
    [KINDS.UPDATING]: "Updating build",
    [KINDS.REPAIRING]: "Repairing files",
    [KINDS.PLAYING]: "In game",
    [KINDS.HOME]: "On the home page",
    [KINDS.PERSONALIZING]: "Customizing the game",
    [KINDS.DEMOS]: "Watching demos",
    [KINDS.DEMO_RENDER]: "Rendering a demo",
    [KINDS.LIVESPLIT]: "Setting up LiveSplit",
    buttons: { site: "HLSR website", discord: "Discord" },
  },
};

const GAMES = {
  "ghosting-renewed": {
    title: "Half-Life 2: Ghosting Mod: Renewed",
  },
  "hl2-4260": {
    title: "Half-Life 2 (4260)",
  },
  "hl2-2707": {
    title: "Half-Life 2 (2707)",
  },
  "hl2-5288": {
    title: "Half-Life 2 (5288)",
  },
  "hl1-won": {
    title: "Half-Life (WON)",
  },
};

const GAME_ALIASES = {
  70: "hl1-won",
  218: "ghosting-renewed",
  220: "hl2-4260",
};

// Эпизоды Ghosting (game-delivery/episodes.js): запускаются как ghosting-renewed, в Discord — своё название.
const EPISODE_TITLES = {
  ep1: "HL2 Ghosting: Episode One",
  ep2: "HL2 Ghosting: Episode Two",
};

function locale(value) {
  return value === "en" ? "en" : "ru";
}

function gameKeyOf(gameId) {
  const key = String(gameId == null ? "" : gameId);
  if (Object.prototype.hasOwnProperty.call(GAMES, key)) return key;
  if (Object.prototype.hasOwnProperty.call(GAME_ALIASES, key)) return GAME_ALIASES[key];
  return null;
}

function gameOf(gameId) {
  const key = gameKeyOf(gameId);
  return key === null ? null : GAMES[key];
}

function normalize(state) {
  const source = state && typeof state === "object" ? state : {};
  const kind = KIND_LIST.indexOf(source.kind) !== -1 ? source.kind : KINDS.LAUNCHER;
  const game = gameOf(source.gameId);

  const needsGame = LAUNCHER_KINDS.indexOf(kind) === -1;
  let effectiveKind = needsGame && !game ? KINDS.LAUNCHER : kind;

  let map = null;
  if (effectiveKind === KINDS.GAME_MAP) {
    map = mapAssets.normalizeMapName(source.map);
    if (map === null) effectiveKind = KINDS.GAME_MENU;
  }

  const isGameKind = GAME_KINDS.indexOf(effectiveKind) !== -1;

  let sessionStartedAtMs = null;
  const sessionValue = Number(source.sessionStartedAtMs);
  if (Number.isFinite(sessionValue) && sessionValue > 0)
    sessionStartedAtMs = Math.floor(sessionValue);

  const fps = isGameKind ? mapAssets.normalizeFps(source.fps) : null;

  let running = false;
  let runStartedAtMs = null;
  if (effectiveKind === KINDS.GAME_MAP && source.running === true) {
    const value = Number(source.runStartedAtMs);
    if (Number.isFinite(value) && value > 0) {
      running = true;
      runStartedAtMs = Math.floor(value);
    }
  }

  const finalTime =
    effectiveKind === KINDS.GAME_MAP &&
    source.finalTime === true &&
    !running &&
    map === mapAssets.FINAL_TIME_MAP;

  const gameId = LAUNCHER_KINDS.indexOf(effectiveKind) !== -1 ? null : gameKeyOf(source.gameId);
  const episode =
    gameId === "ghosting-renewed" && Object.prototype.hasOwnProperty.call(EPISODE_TITLES, source.episode)
      ? source.episode
      : null;

  return {
    kind: effectiveKind,
    gameId,
    episode,
    sessionStartedAtMs,
    locale: locale(source.locale),
    map,
    fps,
    running,
    runStartedAtMs,
    finalTime,
  };
}

function same(a, b) {
  if (!a || !b) return a === b;
  return (
    a.kind === b.kind &&
    a.gameId === b.gameId &&
    a.episode === b.episode &&
    a.sessionStartedAtMs === b.sessionStartedAtMs &&
    a.locale === b.locale &&
    a.map === b.map &&
    a.fps === b.fps &&
    a.running === b.running &&
    a.runStartedAtMs === b.runStartedAtMs &&
    a.finalTime === b.finalTime
  );
}

function fpsSuffix(fps) {
  return fps === null ? "" : ` | ${GAME_TEXT.fpsLabel}: ${fps}`;
}

function toActivity(model, options) {
  const opts = options || {};
  const m = normalize(model);
  const t = TEXT[m.locale];
  const game = gameOf(m.gameId);
  const title = m.episode ? EPISODE_TITLES[m.episode] : GAME_TEXT.title;

  const activity = {
    details: m.episode ? title : game ? game.title : t.launcherDetails,
    state: t[m.kind],
    largeImageKey: ASSETS.large,
    largeImageText: opts.version ? "HLSR v" + String(opts.version) : "HLSR",
    instance: false,
  };

  if (m.kind === KINDS.GAME_MENU) {
    activity.details = `${GAME_TEXT.gameLabel}: ${title}`;
    activity.state = GAME_TEXT.menuState + fpsSuffix(m.fps);
    activity.largeImageKey = ASSETS.menu;
    activity.largeImageText = title;
    activity.smallImageKey = ASSETS.smallIdle;
    activity.smallImageText = GAME_TEXT.menuState;
  } else if (m.kind === KINDS.GAME_MAP) {
    activity.details = m.map;
    activity.state = GAME_TEXT.playingState + fpsSuffix(m.fps);
    activity.largeImageKey = mapAssets.largeImageKeyFor(m.map, m.finalTime);
    activity.largeImageText = mapAssets.isKnownMap(m.map) ? m.map : title;
    activity.smallImageKey = m.running ? ASSETS.smallRunning : ASSETS.smallIdle;
    activity.smallImageText = m.running ? GAME_TEXT.runningSmall : GAME_TEXT.playingState;
  }

  if (m.sessionStartedAtMs !== null) activity.startTimestamp = m.sessionStartedAtMs;

  const buttons = buttonUrls()
    .map((b) => ({ label: t.buttons[b.key], url: b.url }))
    .filter((b) => b.label && b.url)
    .slice(0, 2);
  if (buttons.length) activity.buttons = buttons;

  return activity;
}

const FORBIDDEN_ACTIVITY_KEYS = [
  "steamId",
  "steamID",
  "personaName",
  "userName",
  "installPath",
  "path",
  "args",
  "launchArgs",
  "installationId",
  "ip",
  "crashReason",
  "dump",
  "runFile",
  "config",
  "partyId",
  "matchSecret",
  "joinSecret",
  "spectateSecret",
];

function kindFromDelivery(signal) {
  const state = String((signal && signal.state) || "");

  if (state === "installing" || state === "downloading" || state === "extracting")
    return KINDS.INSTALLING;
  if (state === "updating" || state === "patching") return KINDS.UPDATING;
  if (state === "repairing" || state === "verifying") return KINDS.REPAIRING;
  if (state === "installed" || state === "ready") return KINDS.READY;
  if (signal && signal.gameId != null) return KINDS.BROWSING;
  return KINDS.LAUNCHER;
}

// Одна и та же игра: id лаунчера (218) и id сборки (ghosting-renewed) сравниваются через GAME_ALIASES.
function sameGame(a, b) {
  if (a == null || b == null) return false;
  const keyA = gameKeyOf(a);
  const keyB = gameKeyOf(b);
  return keyA !== null && keyB !== null ? keyA === keyB : String(a) === String(b);
}

module.exports = {
  kindFromDelivery,
  sameGame,
  ACTIVITY_KINDS,
  LAUNCHER_KINDS,
  KINDS,
  KIND_LIST,
  GAME_KINDS,
  GAME_ALIASES,
  EPISODE_TITLES,
  TEXT,
  GAMES,
  FORBIDDEN_ACTIVITY_KEYS,
  normalize,
  same,
  toActivity,
};
