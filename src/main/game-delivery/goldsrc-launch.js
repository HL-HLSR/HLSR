"use strict";

const nodeFs = require("fs");
const path = require("path");

const launchArgs = require("./launch-args");
const { assertInside } = require("./paths");
const { DeliveryError } = require("./errors");

// GoldSrc (Half-Life WON): мод описывается liblist.gam, steam_appid.txt лежит
// рядом с hl.exe, а сама игра стартует только через инжектор Bunnymod XT.

function isGoldSrc(config) {
  return !!config && config.engine === "goldsrc";
}

function insideInstall(installPath, relative, label) {
  return assertInside(installPath, path.join(installPath, String(relative).replace(/\//g, path.sep)), label);
}

function resolveFiles(installPath, config, fs = nodeFs) {
  const executable = insideInstall(installPath, config.executable, "Исполняемый файл игры");
  const gameDirectory = insideInstall(installPath, config.gameDirectory, "Каталог мода");
  const injector = insideInstall(installPath, config.injector, "Инжектор Bunnymod XT");

  if (!fs.existsSync(executable))
    throw new DeliveryError("GAME_EXECUTABLE_MISSING", `В установленной сборке нет ${config.executable}`, { executable });

  if (!fs.existsSync(path.join(gameDirectory, "liblist.gam")))
    throw new DeliveryError("GAME_DIRECTORY_MISSING", `В каталоге ${config.gameDirectory} нет liblist.gam`, { gameDirectory });

  if (!fs.existsSync(injector))
    throw new DeliveryError("GAME_EXECUTABLE_MISSING", `В установленной сборке нет ${config.injector}`, { injector });

  return { executable, gameDirectory, injector };
}

// Старый steam.dll берёт AppID из этого файла; без него игра не узнает,
// под какой лицензией её запустили.
function enforceSteamAppIdFile(installPath, config, fs = nodeFs, logger = null) {
  const file = insideInstall(installPath, config.steamAppIdFile, "steam_appid.txt");
  let current = null;
  try { current = fs.readFileSync(file, "utf8"); } catch (error) { current = null; }
  const normalized = String(current == null ? "" : current).replace(/^\uFEFF/, "").trim();
  if (current !== null && normalized === String(config.steamAppId)) return file;

  try {
    fs.writeFileSync(file, `${config.steamAppId}\n`);
  } catch (error) {
    throw new DeliveryError("STEAM_APP_ID_MISMATCH", `Не удалось записать steam_appid.txt: ${error.message}`, { file });
  }
  if (logger) logger.info(`[launch] ${config.steamAppIdFile} приведён к ${config.steamAppId}`);
  return file;
}

// Переключатели HL1. Source-флаги (-novid, -dx9, -insecure и т. п.) сюда не
// попадают: движок WON их не знает, а ролика при запуске в сборке нет.
// Список проверен по строкам, которые hl.exe и расшифрованный hw.dll ищут
// в командной строке.
const GOLDSRC_TOGGLES = {
  console: ["-console"],
  windowed: ["-windowed"],
  developer: ["-dev"],
  condebug: ["-condebug"],
  nojoy: ["-nojoy"],
  noaafonts: ["-noaafonts"],
};

// Порядок: доверенные аргументы, переключатели, разрешение и частота, строка
// игрока. Если игрок сам вписал -w/-h или -freq, его значения главнее.
function mergeArguments(config, settings) {
  const value = launchArgs.sanitizeSettings(settings).settings;
  const parsed = launchArgs.validateArgsString(value.launchArgs);
  const user = parsed.ok ? parsed.args : [];
  const userNames = new Set(user.map(launchArgs.paramNameOf).filter(Boolean));

  const result = config.arguments.slice();
  const seen = new Set(result.map((arg) => arg.toLowerCase()));
  const push = (token) => {
    const key = token.toLowerCase();
    if (token.startsWith("-")) {
      if (seen.has(key)) return;
      seen.add(key);
    }
    result.push(token);
  };

  for (const key of Object.keys(GOLDSRC_TOGGLES))
    if (value[key]) for (const token of GOLDSRC_TOGGLES[key]) push(token);

  if (value.width && value.height && !userNames.has("w") && !userNames.has("h"))
    result.push("-w", String(value.width), "-h", String(value.height));
  if (value.refreshRate && !userNames.has("freq"))
    result.push("-freq", String(value.refreshRate));

  for (const token of user) push(token);
  return result;
}

// RInput подменяет ввод мыши на raw input. Он внедряется в уже запущенный
// hl.exe отдельным процессом (RInput.exe ждёт появления игры сам), поэтому
// не мешает инжектору BXT. По умолчанию включён.
function resolveRInput(installPath, config, settings, fs = nodeFs) {
  if (!config.rinput) return null;
  if (!launchArgs.sanitizeSettings(settings).settings.rinput) return null;
  const executable = insideInstall(installPath, config.rinput.executable, "RInput");
  return {
    executable,
    processName: path.basename(String(config.executable)),
    missing: !fs.existsSync(executable),
  };
}

function environmentOf(config) {
  const env = {};
  const source = config.environment || {};
  for (const key of Object.keys(source))
    if (/^_*[A-Z][A-Z0-9_]*$/.test(key) && typeof source[key] === "string") env[key] = source[key];
  return env;
}

// Лог Bunnymod XT: что он нашёл в движке и client.dll. Без него сбой, когда
// инжектор отработал, а команды bxt_* так и не появились, не оставляет следов.
// Файл один на все запуски и удаляется перед каждым: BXT дописывает в конец.
const BXT_LOG_FILE = "bxt.log";

function prepareBxtLog(logsDir, fsImpl, logger) {
  if (!logsDir) return null;
  const file = path.join(logsDir, BXT_LOG_FILE);
  try {
    fsImpl.mkdirSync(logsDir, { recursive: true });
    fsImpl.rmSync(file, { force: true });
  } catch (error) {
    if (logger) logger.warn(`[launch] лог BXT не подготовлен: ${error.message}`);
    return null;
  }
  if (logger) logger.info(`[launch] лог BXT: ${file}`);
  return file;
}

module.exports = {
  isGoldSrc,
  resolveFiles,
  enforceSteamAppIdFile,
  GOLDSRC_TOGGLES,
  mergeArguments,
  resolveRInput,
  environmentOf,
  prepareBxtLog,
  BXT_LOG_FILE,
};
