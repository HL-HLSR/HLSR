"use strict";

const MAP_ASSET_KEYS = [
  "d1_canals_01",
  "d1_canals_01a",
  "d1_canals_02",
  "d1_canals_03",
  "d1_canals_05",
  "d1_canals_06",
  "d1_canals_07",
  "d1_canals_08",
  "d1_canals_09",
  "d1_canals_10",
  "d1_canals_11",
  "d1_canals_12",
  "d1_canals_13",
  "d1_eli_01",
  "d1_eli_02",
  "d1_town_01",
  "d1_town_01a",
  "d1_town_02",
  "d1_town_02a",
  "d1_town_04",
  "d1_town_05",
  "d1_trainstation_01",
  "d1_trainstation_02",
  "d1_trainstation_03",
  "d1_trainstation_04",
  "d1_trainstation_05",
  "d1_trainstation_06",
  "d2_coast_01",
  "d2_coast_03",
  "d2_coast_04",
  "d2_coast_05",
  "d2_coast_07",
  "d2_coast_09",
  "d2_coast_10",
  "d2_coast_11",
  "d2_coast_12",
  "d2_prison_01",
  "d2_prison_02",
  "d2_prison_03",
  "d2_prison_04",
  "d2_prison_05",
  "d2_prison_06",
  "d2_prison_07",
  "d2_prison_08",
  "d3_breen_01",
  "d3_c17_01",
  "d3_c17_02",
  "d3_c17_03",
  "d3_c17_04",
  "d3_c17_05",
  "d3_c17_06a",
  "d3_c17_06b",
  "d3_c17_07",
  "d3_c17_08",
  "d3_c17_09",
  "d3_c17_10a",
  "d3_c17_10b",
  "d3_c17_11",
  "d3_c17_12",
  "d3_c17_12b",
  "d3_c17_13",
  "d3_citadel_01",
  "d3_citadel_02",
  "d3_citadel_03",
  "d3_citadel_04",
  "d3_citadel_05",
];

const MAP_ASSET_SET = new Set(MAP_ASSET_KEYS);

const FINAL_TIME_MAP = "d3_breen_01";
const FINAL_TIME_KEY = "d3_breen_01_finaltime";

const ALL_ASSET_KEYS = MAP_ASSET_KEYS.concat([FINAL_TIME_KEY, "background", "hlsr"]);

const MAP_NAME_RE = /^[a-z0-9_]{1,64}$/;

const FPS_MAX = 1000;

const RAW_MAP_MAX_LENGTH = 200;

function normalizeMapName(raw) {
  if (typeof raw !== "string") return null;
  if (raw.length === 0 || raw.length > RAW_MAP_MAX_LENGTH) return null;

  let value = raw.trim().toLowerCase();
  value = value.replace(/\\/g, "/");

  if (value.startsWith("maps/")) value = value.slice("maps/".length);
  if (value.endsWith(".bsp")) value = value.slice(0, -".bsp".length);

  if (!MAP_NAME_RE.test(value)) return null;
  return value;
}

function isKnownMap(name) {
  return typeof name === "string" && MAP_ASSET_SET.has(name);
}

function largeImageKeyFor(mapName, finalTime) {
  if (!isKnownMap(mapName)) return "hlsr";
  if (finalTime === true && mapName === FINAL_TIME_MAP) return FINAL_TIME_KEY;
  return mapName;
}

function normalizeFps(raw) {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return null;
  if (raw < 0 || raw > FPS_MAX) return null;
  return Math.round(raw);
}

const ELAPSED_MAX_MS = 24 * 60 * 60 * 1000;

function normalizeElapsedMs(raw) {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return null;
  if (raw < 0 || raw > ELAPSED_MAX_MS) return null;
  return Math.floor(raw);
}

module.exports = {
  MAP_ASSET_KEYS,
  ALL_ASSET_KEYS,
  FINAL_TIME_MAP,
  FINAL_TIME_KEY,
  FPS_MAX,
  ELAPSED_MAX_MS,
  normalizeMapName,
  isKnownMap,
  largeImageKeyFor,
  normalizeFps,
  normalizeElapsedMs,
};
