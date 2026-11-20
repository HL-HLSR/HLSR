"use strict";

const personalization = require("./ghosting-personalization");
const goldsrcPacks = require("./goldsrc-packs");
const goldsrcPersonalization = require("./goldsrc-personalization");
const liveSplitOverlay = require("../../shared/livesplit-overlay");
const gameBinds = require("../../shared/game-binds");

const MAX_ARGS_LENGTH = 512;

const FORBIDDEN_PARAMS = [
  "game",
  "steam",
  "applaunch",
  "appid",
  "basedir",
  "defaultgamedir",
];

const ALWAYS_ARGUMENTS = ["-insecure"];

const TOGGLE_ARGUMENTS = {
  console: ["-console"],
  novid: ["-novid"],
  borderless: ["-windowed", "-noborder"],
  dx9: ["-dx9"],
  nocrashdialog: ["-nocrashdialog"],
  noasync: ["-noasync"],
  noaafonts: ["-noaafonts"],
  high: ["-high"],
};

// Поля HL1 (GoldSrc). Аргументы из них собирает goldsrc-launch.js, Source-игры
// их не читают.
const GOLDSRC_FLAGS = ["windowed", "developer", "condebug", "nojoy"];
const WIDTH_LIMITS = [320, 7680];
const HEIGHT_LIMITS = [200, 4320];
const REFRESH_LIMITS = [24, 540];

function integerIn(value, limits) {
  const number = Number(value);
  return Number.isInteger(number) && number >= limits[0] && number <= limits[1] ? number : 0;
}

const LANGUAGE_RE = /^[a-z]{2,24}$/;
const LANGUAGE_AUTO = "auto";

function languageOf(input) {
  const value =
    input && typeof input.language === "string" ? input.language.trim().toLowerCase() : "";
  return LANGUAGE_RE.test(value) ? value : LANGUAGE_AUTO;
}

const MAX_AFFINITY_CPUS = 64;

function affinityMaskWithoutCpu0(cpuCount) {
  const count = Math.min(Math.max(Math.floor(Number(cpuCount) || 0), 0), MAX_AFFINITY_CPUS);

  if (count < 2) return null;

  const one = BigInt(1);
  const all = (one << BigInt(count)) - one;

  return (all & ~one).toString();
}

// Параметры Proton на Linux, как поле «Параметры запуска» в Steam:
// «PROTON_USE_WINED3D=1 mangohud %command% -novid». Переменные в начале
// строки уходят в окружение, слова до %command% — программа-обёртка вокруг
// umu-run, слова после — аргументы игры.
const PROTON_COMMAND_TOKEN = "%command%";
const ENV_ASSIGNMENT = /^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/s;
// Эти переменные задаёт сам лаунчер: без них игра уйдёт в чужой префикс или
// не запустится.
const PROTECTED_ENV = new Set([
  "WINEPREFIX", "PROTONPATH", "PROTON_VERB", "GAMEID", "STORE", "UMU_ID", "UMU_USE_STEAM",
  "SteamAppId", "SteamGameId",
]);
const PROTECTED_ENV_PREFIXES = ["STEAM_COMPAT_"];

function isProtectedEnv(name) {
  return PROTECTED_ENV.has(name) || PROTECTED_ENV_PREFIXES.some((prefix) => name.startsWith(prefix));
}

function parseProtonOptions(raw) {
  const parsed = parseArgsString(raw);
  if (!parsed.ok) return parsed;
  const env = {};
  const wrapper = [];
  const args = [];
  let i = 0;
  for (; i < parsed.args.length; i++) {
    const match = ENV_ASSIGNMENT.exec(parsed.args[i]);
    if (!match) break;
    if (isProtectedEnv(match[1]))
      return { ok: false, error: { code: "PROTECTED_ENV", param: match[1] } };
    env[match[1]] = match[2];
  }
  let afterCommand = false;
  for (; i < parsed.args.length; i++) {
    const token = parsed.args[i];
    if (token === PROTON_COMMAND_TOKEN) {
      if (afterCommand) return { ok: false, error: { code: "INVALID" } };
      afterCommand = true;
    } else if (afterCommand) {
      if (isForbidden(token)) return { ok: false, error: { code: "FORBIDDEN_PARAM", param: token } };
      args.push(token);
    } else {
      if (!wrapper.length && /^[-+]/.test(token))
        return { ok: false, error: { code: "WRAPPER_INVALID", param: token } };
      wrapper.push(token);
    }
  }
  return { ok: true, env, wrapper, args };
}

function defaultSettings() {
  return {
    launchArgs: "",
    protonOptions: "",
    console: false,
    novid: false,
    borderless: false,
    dx9: false,
    nocrashdialog: false,
    noasync: false,
    noaafonts: false,
    high: false,
    excludeCpu0: false,
    startTimer: false,
    // Vulkan через DXVK (vulkan-backend.js), не аргумент запуска.
    vulkan: false,
    language: LANGUAGE_AUTO,
    rinput: true,
    rinputMenuFix: false,
    windowed: false,
    developer: false,
    condebug: false,
    nojoy: false,
    width: 0,
    height: 0,
    refreshRate: 0,
    packs: goldsrcPacks.defaultPacks(),
    bxt: goldsrcPersonalization.defaultBxt(),
    binds: {},
    personalization: personalization.defaultPersonalization(),
    liveSplitOverlay: liveSplitOverlay.defaultOverlay(),
  };
}

function paramNameOf(token) {
  const match = /^(?:--|[-+/])([^=:]+)/.exec(token);
  if (!match) return null;
  return match[1].toLowerCase();
}

function isForbidden(token) {
  const name = paramNameOf(token);
  return name != null && FORBIDDEN_PARAMS.indexOf(name) !== -1;
}

function parseArgsString(raw) {
  if (raw == null) return { ok: true, args: [] };

  if (typeof raw !== "string") return { ok: false, error: { code: "INVALID_TYPE" } };

  if (raw.length > MAX_ARGS_LENGTH)
    return { ok: false, error: { code: "TOO_LONG", limit: MAX_ARGS_LENGTH } };

  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x1f\x7f]/.test(raw))
    return { ok: false, error: { code: "CONTROL_CHARS" } };

  const args = [];
  let current = "";
  let hasCurrent = false;
  let inQuotes = false;

  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];

    if (ch === '"') {
      inQuotes = !inQuotes;
      hasCurrent = hasCurrent || current.length > 0;
      continue;
    }

    if (ch === " " && !inQuotes) {
      if (hasCurrent && current.length > 0) args.push(current);
      current = "";
      hasCurrent = false;
      continue;
    }

    current += ch;
    hasCurrent = true;
  }

  if (inQuotes) return { ok: false, error: { code: "UNBALANCED_QUOTES" } };

  if (hasCurrent && current.length > 0) args.push(current);

  return { ok: true, args };
}

function validateArgsString(raw) {
  const parsed = parseArgsString(raw);
  if (!parsed.ok) return parsed;

  for (const token of parsed.args)
    if (isForbidden(token))
      return { ok: false, error: { code: "FORBIDDEN_PARAM", param: token } };

  return parsed;
}

function normalizeSettings(raw) {
  const input = raw && typeof raw === "object" ? raw : {};

  const launchArgs = input.launchArgs == null ? "" : input.launchArgs;
  const checked = validateArgsString(launchArgs);
  if (!checked.ok) return checked;

  const protonOptions = input.protonOptions == null ? "" : input.protonOptions;
  const proton = parseProtonOptions(protonOptions);
  if (!proton.ok) return { ok: false, error: Object.assign({ field: "protonOptions" }, proton.error) };

  return {
    ok: true,
    settings: Object.assign(settingsFieldsOf(input), {
      launchArgs: typeof launchArgs === "string" ? launchArgs.trim() : "",
      protonOptions: typeof protonOptions === "string" ? protonOptions.trim() : "",
    }),
  };
}

// Параметр или число: без консольных команд («+exec») и без путей.
const IMPORTABLE_ARG = /^-[a-z0-9_-]+$|^\d+(?:\.\d+)?$/i;

// Настройки из чужого файла (.hlsrbuild, .hlsrpack). Такой файл выбирают один раз, а параметры
// сработают при каждом запуске игры, поэтому команды Proton всегда остаются свои, а параметры
// запуска принимаются, только если в них нет команд и путей.
function importedSettings(raw, current) {
  const input = raw && typeof raw === "object" ? raw : {};
  const parsed = parseArgsString(input.launchArgs == null ? "" : input.launchArgs);
  const safe = parsed.ok && parsed.args.every((token) => IMPORTABLE_ARG.test(token));
  return Object.assign({}, input, {
    protonOptions: current.protonOptions,
    launchArgs: safe ? input.launchArgs : current.launchArgs,
  });
}

function settingsFieldsOf(input) {
  const flags = {};

  for (const key of Object.keys(TOGGLE_ARGUMENTS)) flags[key] = input[key] === true;

  flags.excludeCpu0 = input.excludeCpu0 === true;

  flags.startTimer = input.startTimer === true;

  flags.vulkan = input.vulkan === true;

  flags.language = languageOf(input);

  flags.rinput = input.rinput !== false;
  flags.rinputMenuFix = input.rinputMenuFix === true;
  for (const key of GOLDSRC_FLAGS) flags[key] = input[key] === true;
  // Разрешение имеет смысл только парой: одно число без другого отбрасываем.
  const width = integerIn(input.width, WIDTH_LIMITS);
  const height = integerIn(input.height, HEIGHT_LIMITS);
  flags.width = width && height ? width : 0;
  flags.height = width && height ? height : 0;
  flags.refreshRate = integerIn(input.refreshRate, REFRESH_LIMITS);
  flags.packs = goldsrcPacks.sanitizePacks(input.packs);
  flags.bxt = goldsrcPersonalization.sanitizeBxt(input.bxt);
  flags.binds = gameBinds.sanitizeOverrides(input.binds);

  try {
    flags.personalization = personalization.sanitizePersonalization(input.personalization);
  } catch (ignored) {
    flags.personalization = personalization.defaultPersonalization();
  }
  flags.liveSplitOverlay = liveSplitOverlay.sanitizeOverlay(input.liveSplitOverlay);

  return flags;
}

function sanitizeSettings(raw) {
  const input = raw && typeof raw === "object" ? raw : {};

  const settings = Object.assign(settingsFieldsOf(input), { launchArgs: "", protonOptions: "" });

  const checked = validateArgsString(input.launchArgs == null ? "" : input.launchArgs);
  let dropped = checked.ok ? null : checked.error;

  if (checked.ok && typeof input.launchArgs === "string")
    settings.launchArgs = input.launchArgs.trim();

  const proton = parseProtonOptions(input.protonOptions == null ? "" : input.protonOptions);
  if (proton.ok && typeof input.protonOptions === "string")
    settings.protonOptions = input.protonOptions.trim();
  else if (!proton.ok && !dropped) dropped = Object.assign({ field: "protonOptions" }, proton.error);

  return { settings, dropped };
}

function buildUserArguments(settings, knownArgs) {
  const value = sanitizeSettings(settings).settings;
  const seen = new Set((knownArgs || []).map((a) => String(a).toLowerCase()));
  const result = [];

  const push = (token) => {
    const key = token.toLowerCase();

    if (token.startsWith("-")) {
      if (seen.has(key)) return;
      seen.add(key);
    }

    result.push(token);
  };

  for (const key of Object.keys(TOGGLE_ARGUMENTS))
    if (value[key]) for (const token of TOGGLE_ARGUMENTS[key]) push(token);

  const parsed = validateArgsString(value.launchArgs);
  if (parsed.ok) for (const token of parsed.args) push(token);

  return result;
}

function mergeLaunchArguments(trustedArgs, settings) {
  const base = (trustedArgs || []).slice();
  const seen = new Set(base.map((arg) => String(arg).toLowerCase()));

  for (const token of ALWAYS_ARGUMENTS) {
    if (seen.has(token.toLowerCase())) continue;
    seen.add(token.toLowerCase());
    base.push(token);
  }

  return base.concat(buildUserArguments(settings, base));
}

module.exports = {
  importedSettings,
  MAX_ARGS_LENGTH,
  FORBIDDEN_PARAMS,
  ALWAYS_ARGUMENTS,
  TOGGLE_ARGUMENTS,
  GOLDSRC_FLAGS,
  LANGUAGE_AUTO,
  languageOf,
  MAX_AFFINITY_CPUS,
  affinityMaskWithoutCpu0,
  defaultSettings,
  paramNameOf,
  isForbidden,
  PROTON_COMMAND_TOKEN,
  isProtectedEnv,
  parseProtonOptions,
  parseArgsString,
  validateArgsString,
  normalizeSettings,
  sanitizeSettings,
  buildUserArguments,
  mergeLaunchArguments,
};
