"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");

const httpClient = require("./http-client");
const { assertInside } = require("./paths");
const { DeliveryError, toDeliveryError } = require("./errors");

// Готовые HUD и прицелы для HL1 из ресурсов раздела на speedrun.com. Лицензии
// у паков нет, поэтому лаунчер их не раздаёт: скачивает с Google Drive автора
// и сверяет с закреплённым SHA-256. Если автор заменит файл, пак не ставится,
// пока его не проверят и не обновят хэш здесь.
//
// Перед каждым запуском папка спрайтов мода сначала возвращается в исходное
// состояние (оригиналы лежат в .hlsr/packs/backup), а затем в неё кладутся
// выбранные паки. Так смена или отключение пака ничего не оставляет за собой.

const SOURCE_PAGE = require("../../shared/links").LINKS.hl1Resources;
const DRIVE_DOWNLOAD = "https://drive.usercontent.google.com/download";
const DOWNLOAD_TIMEOUT_MS = 60000;
const STATE_DIR = ".hlsr/packs";

const { CHANGER_SHAPES, CHANGER_COLORS, HUD_PACKS, CROSSHAIR_PACKS } = require("../../shared/goldsrc-settings");

const PACKS = {
  "ag-hud": {
    kind: "hud",
    title: "Big AG HUD",
    author: "execut4ble",
    driveId: "1_nmTVsEVha-bbQwCGnC7eHfshvCSqIw7",
    size: 177578,
    sha256: "462e5714d1a12d65af8c1b1ec45874d91e99d78de91ef2e7c65676d0e037c1e3",
    root: "big AG HUD",
  },
  "execut4ble-crosshair": {
    kind: "crosshair",
    title: "Crosshair by execut4ble",
    author: "execut4ble",
    driveId: "1-73LVNJh89pSLGk7e6gmIiAurpIgqIAD",
    size: 5152,
    sha256: "43b9e3a1f79e84924c12b3d7c4c2fa9573cffe0e4dab7b5d09833c9db7ccd2c6",
    root: "crosshair by execut4ble",
  },
  "crosshair-changer": {
    kind: "crosshair",
    title: "Half-Life Crosshair Changer",
    author: "aarongeorge",
    driveId: "1cq8baRp4TQrwdl8fx0F3g3UW-L1uLY9-",
    size: 59902,
    sha256: "98594d4cc130187facb8efdfe619e5fed22d83c22b9da398b15cd9dde08d9fbd",
    root: "Half-Life-Crosshair-Changer-master/crosshairs",
  },
};

function defaultPacks() {
  return { hud: "default", crosshair: "default", changerShape: "Default", changerColor: "green" };
}

function oneOf(value, allowed, fallback) {
  return allowed.indexOf(value) !== -1 ? value : fallback;
}

function sanitizePacks(raw) {
  const input = raw && typeof raw === "object" ? raw : {};
  const base = defaultPacks();
  return {
    hud: oneOf(input.hud, HUD_PACKS, base.hud),
    crosshair: oneOf(input.crosshair, CROSSHAIR_PACKS, base.crosshair),
    changerShape: oneOf(input.changerShape, CHANGER_SHAPES, base.changerShape),
    changerColor: oneOf(input.changerColor, CHANGER_COLORS, base.changerColor),
  };
}

function catalog() {
  return {
    sourcePage: SOURCE_PAGE,
    packs: Object.keys(PACKS).map((id) => ({
      id,
      kind: PACKS[id].kind,
      title: PACKS[id].title,
      author: PACKS[id].author,
    })),
    changerShapes: CHANGER_SHAPES.slice(),
    changerColors: CHANGER_COLORS.slice(),
  };
}

function sha256Of(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex");
}

function inside(root, relative) {
  return assertInside(root, path.join(root, String(relative).replace(/\//g, path.sep)), "Пак");
}

function readState(file, fs) {
  try {
    const state = JSON.parse(fs.readFileSync(file, "utf8"));
    return state && typeof state.files === "object" && state.files ? state : { files: {} };
  } catch (error) {
    return { files: {} };
  }
}

function writeState(file, state, fs) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const temp = `${file}.tmp`;
  fs.writeFileSync(temp, JSON.stringify(state, null, 2));
  fs.renameSync(temp, file);
}

function readIfExists(file, fs) {
  try {
    return fs.readFileSync(file);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

// --- скачивание и распаковка ---

async function fetchPack(pack, cacheFile, client, fs) {
  const cached = readIfExists(cacheFile, fs);
  if (cached && cached.length === pack.size && sha256Of(cached) === pack.sha256) return;

  const url = `${DRIVE_DOWNLOAD}?id=${encodeURIComponent(pack.driveId)}&export=download`;
  const response = await client.request(url, { timeout: DOWNLOAD_TIMEOUT_MS });
  if (response.statusCode !== 200) {
    response.resume();
    throw new DeliveryError("HTTP_ERROR", `HTTP ${response.statusCode} для пака ${pack.title}`);
  }

  // Google Drive отдаёт HTML-страницу, если файл удалён или закрыт: она не
  // пройдёт ни по размеру, ни по хэшу.
  const body = await client.readBody(response, pack.size + 1);
  if (body.length !== pack.size || sha256Of(body) !== pack.sha256)
    throw new DeliveryError("CHECKSUM_MISMATCH", `Пак ${pack.title} на Google Drive изменился или недоступен`, {
      expected: pack.sha256,
      actual: sha256Of(body),
    });

  fs.mkdirSync(path.dirname(cacheFile), { recursive: true });
  fs.writeFileSync(cacheFile, body);
}

async function unpack(cacheFile, destination, archive, fs) {
  if (!archive) throw new DeliveryError("EXTRACT_FAILED", "Нет 7-Zip для распаковки пака");
  fs.rmSync(destination, { recursive: true, force: true });
  fs.mkdirSync(destination, { recursive: true });
  await archive.extract(cacheFile, destination).promise;
}

function listFiles(dir, fs) {
  return fs.readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isFile()).map((entry) => entry.name);
}

// --- что положить в папку спрайтов ---

const CROSSHAIR_LINE = /^\s*(crosshair|autoaim|zoom|zoom_autoaim)\s/i;

function linesOf(buffer) {
  return buffer.toString("latin1").replace(/^\xEF\xBB\xBF/, "").split(/\r?\n/).map((line) => line.replace(/\s+$/, ""));
}

// Пак execut4ble несёт свои weapon_*.txt, но иконки оружия в них размечены под
// Steam-версию: у мины в WON был бы чужой значок. Поэтому из файлов пака
// берутся только строки прицела, а иконки остаются из файлов игры.
function mergeCrosshairLines(base, pack) {
  const keep = linesOf(base).slice(1).filter((line) => line.trim() && !CROSSHAIR_LINE.test(line));
  const cross = linesOf(pack).slice(1).filter((line) => CROSSHAIR_LINE.test(line));
  const body = keep.concat(cross);
  return Buffer.from([String(body.length)].concat(body).join("\r\n") + "\r\n", "latin1");
}

function desiredFiles(selection, sources, readBase) {
  const files = {};

  if (selection.hud === "ag-hud") {
    const dir = sources["ag-hud"];
    for (const name of dir.names) files[name] = dir.read(name);
  }

  if (selection.crosshair === "execut4ble-crosshair") {
    const dir = sources["execut4ble-crosshair"];
    for (const name of dir.names) {
      if (/^weapon_.*\.txt$/i.test(name)) {
        const base = readBase(name);
        files[name] = base ? mergeCrosshairLines(base, dir.read(name)) : dir.read(name);
      } else {
        files[name] = dir.read(name);
      }
    }
  }

  if (selection.crosshair === "crosshair-changer") {
    const dir = sources["crosshair-changer"];
    files["crosshairs.spr"] = dir.read(`${selection.changerShape}/${selection.changerColor}.spr`);
  }

  return files;
}

// --- возврат к исходному состоянию и установка ---

// Возвращает оригиналы файлов, которые лаунчер подменял. Файл, который с тех
// пор изменился (например, его обновила сборка), уже чужой: его не трогаем,
// а старую копию оригинала выбрасываем.
function restore(spritesDir, backupDir, state, fs) {
  for (const name of Object.keys(state.files)) {
    const entry = state.files[name];
    const target = inside(spritesDir, name);
    const backup = inside(backupDir, name);
    const current = readIfExists(target, fs);

    if (current && sha256Of(current) === entry.sha256) {
      if (entry.backup && fs.existsSync(backup)) fs.copyFileSync(backup, target);
      else fs.rmSync(target, { force: true });
    }
    fs.rmSync(backup, { force: true });
  }
}

async function applyPacks(installPath, config, settings, options = {}) {
  const packsConfig = config && config.packs;
  if (!packsConfig) return { status: "skipped" };

  const fs = options.fs || nodeFs;
  const client = options.client || httpClient;
  const logger = options.logger || { info() {}, warn() {}, error() {} };
  const selection = sanitizePacks(settings && settings.packs);
  const packs = options.catalog || PACKS;

  const spritesDir = inside(installPath, packsConfig.spritesDir);
  const baseSpritesDir = inside(installPath, packsConfig.baseSpritesDir);
  const stateRoot = inside(installPath, STATE_DIR);
  const stateFile = path.join(stateRoot, "applied.json");
  const backupDir = path.join(stateRoot, "backup");

  const wanted = [];
  if (selection.hud !== "default") wanted.push(selection.hud);
  if (selection.crosshair !== "default") wanted.push(selection.crosshair);

  // Сначала скачиваем: если сети нет, игра остаётся с прошлыми паками, а не
  // без них.
  const sources = {};
  const tempRoot = path.join(stateRoot, `unpacked-${process.pid}`);
  try {
    for (const id of wanted) {
      const cacheFile = path.join(stateRoot, "cache", `${id}.zip`);
      await fetchPack(packs[id], cacheFile, client, fs);
      const unpacked = path.join(tempRoot, id);
      await unpack(cacheFile, unpacked, options.archive, fs);
      const root = inside(unpacked, packs[id].root);
      const names = id === "crosshair-changer" ? [] : listFiles(root, fs);
      sources[id] = { names, read: (name) => fs.readFileSync(inside(root, name)) };
    }

    fs.mkdirSync(spritesDir, { recursive: true });
    const state = readState(stateFile, fs);
    restore(spritesDir, backupDir, state, fs);
    writeState(stateFile, { files: {} }, fs);

    const readBase = (name) => readIfExists(inside(spritesDir, name), fs) || readIfExists(inside(baseSpritesDir, name), fs);
    const files = desiredFiles(selection, sources, readBase);

    // Оригиналы и запись о них сохраняются до подмены: если запуск оборвётся
    // посередине, следующий вернёт всё как было.
    const next = { selection, files: {} };
    for (const name of Object.keys(files)) {
      const target = inside(spritesDir, name);
      const backup = inside(backupDir, name);
      const hadOriginal = fs.existsSync(target);
      if (hadOriginal) {
        fs.mkdirSync(path.dirname(backup), { recursive: true });
        fs.copyFileSync(target, backup);
      }
      next.files[name] = { sha256: sha256Of(files[name]), backup: hadOriginal };
    }
    writeState(stateFile, next, fs);

    for (const name of Object.keys(files)) fs.writeFileSync(inside(spritesDir, name), files[name]);

    if (wanted.length)
      logger.info(`[launch] паки HL1: ${wanted.map((id) => packs[id].title).join(", ")}, файлов ${Object.keys(files).length}`);
    return { status: wanted.length ? "applied" : "default", packs: wanted };
  } catch (error) {
    const delivery = toDeliveryError(error, "NETWORK_ERROR");
    logger.warn(`[launch] паки HL1 не применены: ${delivery.code} ${delivery.message}`);
    return { status: "failed", packs: wanted, error: { code: delivery.code, message: delivery.message } };
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

module.exports = {
  PACKS,
  SOURCE_PAGE,
  CHANGER_SHAPES,
  CHANGER_COLORS,
  defaultPacks,
  sanitizePacks,
  catalog,
  mergeCrosshairLines,
  applyPacks,
};
