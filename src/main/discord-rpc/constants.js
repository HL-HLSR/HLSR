"use strict";

const { LINKS } = require("../../shared/links");

// Приложение Discord подставляет webpack в сборку выпуска (HLSR_DISCORD_APP_ID); без него
// статус в Discord не показывается.
const APPLICATION_ID = process.env.HLSR_DISCORD_APP_ID || "";

const ASSETS = {
  large: "hlsr",
  menu: "background",
  smallIdle: "playing",
  smallRunning: "running",
};

const ALLOWED_SMALL_IMAGE_KEYS = ["running", "playing"];
const ALLOWED_SMALL_IMAGE_TEXTS = ["Run in progress", "Playing", "In the menu"];

const GAME_TITLE = "Ghosting Mod: Renewed";

const GAME_TEXT = {
  title: GAME_TITLE,
  menuState: "In the menu",
  playingState: "Playing",
  runningSmall: "Run in progress",
  fpsLabel: "FPS",
  gameLabel: "Game",
};

const STATES = {
  DISABLED: "disabled",
  DISCONNECTED: "disconnected",
  CONNECTING: "connecting",
  CONNECTED: "connected",
  RETRY_WAIT: "retry_wait",
  UNAVAILABLE: "unavailable",
  SHUTTING_DOWN: "shutting_down",
};

const CODES = {
  DISABLED: "RPC_DISABLED",
  CONNECTING: "RPC_CONNECTING",
  CONNECTED: "RPC_CONNECTED",
  DISCORD_UNAVAILABLE: "RPC_DISCORD_UNAVAILABLE",
  DISCONNECTED: "RPC_DISCONNECTED",
  UPDATE_FAILED: "RPC_UPDATE_FAILED",
  SHUTDOWN: "RPC_SHUTDOWN",
};

const BUTTON_KEYS = ["site", "discord"];

const TIMINGS = {
  DEBOUNCE_MS: 1500,
  RETRY_BASE_MS: 5000,
  RETRY_MAX_MS: 5 * 60 * 1000,
  RETRY_JITTER: 0.25,
  LOG_THROTTLE_MS: 60 * 1000,
};

function buttonUrls() {
  return BUTTON_KEYS.map((key) => ({ key, url: LINKS[key] || null })).filter((b) => b.url);
}

const CHANNELS = {
  get: "discord-rpc-get",
  set: "discord-rpc-set",
  state: "discord-rpc-state",
  // Интерфейс -> main: раздел, где сейчас игрок (presence-model ACTIVITY_KINDS).
  activity: "discord-rpc-activity",
};

module.exports = {
  CHANNELS,
  APPLICATION_ID,
  ASSETS,
  ALLOWED_SMALL_IMAGE_KEYS,
  ALLOWED_SMALL_IMAGE_TEXTS,
  GAME_TITLE,
  GAME_TEXT,
  STATES,
  CODES,
  BUTTON_KEYS,
  TIMINGS,
  buttonUrls,
};
