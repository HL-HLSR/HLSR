"use strict";

// Эпизоды Ghosting: Episode One (episodic\) и Episode Two (ep2\) ставятся в папку
// установленного ghosting-renewed рядом с ghostingmod\ и запускаются тем же hl2.exe
// с -game episodic|ep2 (docs/superpowers/specs/2026-11-14-ghosting-episodes-design.md).
// Маркеры лежат внутри папки эпизода: при обновлении Ghosting она переезжает целиком
// (InstallService.carryEpisodes).

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { DeliveryError } = require("./errors");
const { EPISODE_DIRS } = require("./mutable-paths");

const EPISODES = {
  ep1: { dir: "episodic", requires: [] },
  // Episode Two монтирует episodic\ и episodic\ep1_pak.vpk: без Episode One не запустится.
  ep2: { dir: "ep2", requires: ["ep1"] },
};

const MARKER = ".hlsr-episode.json";
const SYNC = ".hlsr-sync.json";
const GHOSTING_CFG = path.join("ghostingmod", "cfg");
// Файлы игрока, которые идут в эпизод всегда: его конфиг лаунчера и config.cfg игры (бинды).
const PLAYER_FILES = ["autoexec.cfg", "config.cfg"];

// Данные игрока в папке эпизода (как ghostingmod в mutable-paths USER_DATA, без cfg — его каждый
// запуск кладёт syncConfigIn) и демо в её корне: при переустановке и после краха подмены они
// переезжают rename из прежней копии (episode-install.swapIn, InstallService.unparkEpisodes).
const PLAYER_ENTRIES = ["save", "runs", "screenshots", "custom", "synclogs", "download", "downloadlists"];
const isPlayerEntry = (name) => PLAYER_ENTRIES.includes(String(name).toLowerCase()) || /\.dem$/i.test(name);

// Только строка: массив ["ep1"] из IPC hasOwnProperty приводит к "ep1".
const isEpisode = (id) => typeof id === "string" && Object.prototype.hasOwnProperty.call(EPISODES, id);

function episodeOf(id) {
  if (!isEpisode(id)) throw new DeliveryError("LAUNCH_CONFIG_INVALID", `Нет эпизода ${id}`, { id });
  return EPISODES[id];
}

const dirOf = (installPath, id) => path.join(installPath, episodeOf(id).dir);
const sha256Of = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

function readJson(file, fs) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    return null;
  }
}

function readMarker(installPath, id, fs = nodeFs) {
  return readJson(path.join(dirOf(installPath, id), MARKER), fs);
}

function status(installPath, { fs = nodeFs } = {}) {
  const result = {};
  for (const id of Object.keys(EPISODES)) {
    const marker = installPath ? readMarker(installPath, id, fs) : null;
    const installed = !!marker && typeof marker.version === "string" && fs.existsSync(path.join(dirOf(installPath, id), "gameinfo.txt"));
    result[id] = { installed, version: installed ? marker.version : null };
  }
  return result;
}

// Эпизоды, которые без этого не запустятся (ep1 → ep2).
const dependentsOf = (id) => Object.keys(EPISODES).filter((other) => EPISODES[other].requires.includes(id));

// Доверенная конфигурация запуска Ghosting с каталогом мода эпизода: -game ghostingmod → -game <dir>.
function launchConfig(config, id) {
  episodeOf(id);
  const args = Array.isArray(config.arguments) ? config.arguments.slice() : [];
  const at = args.indexOf("-game");
  if (at < 0 || at + 1 >= args.length)
    throw new DeliveryError("LAUNCH_CONFIG_INVALID", "В конфигурации запуска нет -game", { id });
  args[at + 1] = EPISODES[id].dir;
  return Object.assign({}, config, { gameDirectory: EPISODES[id].dir, arguments: args });
}

// Конфиг Ghosting в cfg эпизода перед запуском. Идут autoexec.cfg, config.cfg и cfg игрока —
// те, которых нет в сборке Ghosting (ghostingStock: имена файлов ghostingmod/cfg из её
// files.json, в нижнем регистре; null — сборка неизвестна, только PLAYER_FILES). Стоковые
// cfg Ghosting (chapter*.cfg HL2, skill.cfg) в эпизод не попадают: там свои.
function hashOf(file, fs) {
  try {
    return sha256Of(fs.readFileSync(file));
  } catch (error) {
    return null;
  }
}

function writeAtomic(target, bytes, fs) {
  const temp = `${target}.hlsr-episode-tmp`;
  try {
    fs.writeFileSync(temp, bytes);
    fs.renameSync(temp, target);
  } catch (error) {
    fs.rmSync(temp, { force: true });
    throw error;
  }
}

function syncConfigIn(installPath, id, { fs = nodeFs, ghostingStock = null } = {}) {
  const episodeDir = dirOf(installPath, id);
  if (!fs.existsSync(episodeDir)) return { copied: [], configSha256: null };
  // Прошлый запуск не вернул config.cfg (крах): сначала вернуть, потом перезаписывать.
  if (fs.existsSync(path.join(episodeDir, SYNC))) syncConfigBack(installPath, id, { fs });

  const stock = ghostingStock === null ? null : new Set([...ghostingStock].map((name) => String(name).toLowerCase()));
  const from = path.join(installPath, GHOSTING_CFG);
  const to = path.join(episodeDir, "cfg");
  let names = [];
  try {
    names = fs.readdirSync(from, { withFileTypes: true }).filter((entry) => entry.isFile()).map((entry) => entry.name);
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
  }
  const copied = names.filter((name) => {
    const lower = name.toLowerCase();
    if (!lower.endsWith(".cfg")) return false;
    return PLAYER_FILES.includes(lower) || (stock !== null && !stock.has(lower));
  });
  fs.mkdirSync(to, { recursive: true });
  // Файлы игрока — под именами, которые читает игра (Config.cfg → config.cfg).
  for (const name of copied) {
    const lower = name.toLowerCase();
    fs.copyFileSync(path.join(from, name), path.join(to, PLAYER_FILES.includes(lower) ? lower : name));
  }

  const configSha256 = hashOf(path.join(to, "config.cfg"), fs);
  // srcSha256 — config.cfg Ghosting в момент копии: вернуть можно, только если его с тех пор не меняли.
  const srcSha256 = hashOf(path.join(from, "config.cfg"), fs);
  writeAtomic(path.join(episodeDir, SYNC), JSON.stringify({ configSha256, srcSha256 }, null, 2), fs);
  return { copied, configSha256 };
}

// После выхода из эпизода: config.cfg, который игра переписала (бинды из меню), — обратно в
// Ghosting. Не вернётся, если игра его не меняла, файл пуст (игра упала на записи) или config.cfg
// Ghosting успели поменять (лаунчер, сама Ghosting) — тогда новее он. Без отметки ничего не делает.
function syncConfigBack(installPath, id, { fs = nodeFs } = {}) {
  const syncFile = path.join(dirOf(installPath, id), SYNC);
  if (!fs.existsSync(syncFile)) return { returned: false };
  const sync = readJson(syncFile, fs);
  let returned = false;
  let conflict = false;
  if (sync) {
    let bytes = null;
    try {
      bytes = fs.readFileSync(path.join(dirOf(installPath, id), "cfg", "config.cfg"));
    } catch (error) {
      bytes = null;
    }
    const target = path.join(installPath, GHOSTING_CFG, "config.cfg");
    if (bytes && bytes.length && sha256Of(bytes) !== sync.configSha256) {
      if (hashOf(target, fs) !== (sync.srcSha256 || null)) conflict = true;
      else {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        writeAtomic(target, bytes, fs);
        returned = true;
      }
    }
  }
  fs.rmSync(syncFile, { force: true }); // битая отметка тоже снимается: иначе висела бы вечно
  return { returned, conflict };
}

// Эпизоды, у которых осталась отметка синхронизации: игра упала или лаунчер закрылся раньше.
const pendingSync = (installPath, { fs = nodeFs } = {}) =>
  installPath ? Object.keys(EPISODES).filter((id) => fs.existsSync(path.join(dirOf(installPath, id), SYNC))) : [];

// syncConfigBack для ids (по умолчанию — всех с отметкой). Зовут на выходе игры, на старте
// лаунчера и перед запуском Ghosting, когда игра не запущена. Не бросает: сбой — в журнал.
function returnPending(installPath, { fs = nodeFs, logger = null, ids = null } = {}) {
  const log = logger || { info() {}, warn() {} };
  for (const id of ids || pendingSync(installPath, { fs })) {
    try {
      const { returned, conflict } = syncConfigBack(installPath, id, { fs });
      if (returned) log.info(`[episodes] ${id}: config.cfg эпизода вернулся в ghostingmod/cfg`);
      else if (conflict) log.warn(`[episodes] ${id}: config.cfg эпизода не вернулся — config.cfg Ghosting с тех пор менялся`);
    } catch (error) {
      log.warn(`[episodes] ${id}: config.cfg эпизода не вернулся: ${error.message}`);
    }
  }
}

// Удаляет эпизод и те, что без него не запустятся.
function remove(installPath, id, { fs = nodeFs } = {}) {
  episodeOf(id);
  const ids = dependentsOf(id).concat(id);
  const removed = [];
  for (const each of ids) {
    const dir = dirOf(installPath, each);
    if (!fs.existsSync(dir)) continue;
    fs.rmSync(dir, { recursive: true, force: true });
    removed.push(EPISODES[each].dir);
  }
  return { removed };
}

if (Object.values(EPISODES).map((e) => e.dir).join() !== EPISODE_DIRS.join())
  throw new Error("episodes.js и mutable-paths EPISODE_DIRS разошлись");

module.exports = {
  EPISODES,
  MARKER,
  SYNC,
  PLAYER_ENTRIES,
  isPlayerEntry,
  isEpisode,
  episodeOf,
  readMarker,
  status,
  dependentsOf,
  launchConfig,
  syncConfigIn,
  syncConfigBack,
  pendingSync,
  returnPending,
  remove,
};
