"use strict";

// Эпизод Ghosting ставится по своему подписанному манифесту, который лежит рядом с
// каналом Ghosting: .../ghosting-renewed/channel/stable.json → .../ghosting-renewed/episodes/<id>/stable.json,
// архив 7z — рядом с манифестом. Подпись — ключом канала ghosting-renewed (channel-config).
// Саму установку ведёт GameDeliveryService.installEpisode.

const fs = require("fs");
const path = require("path");
const httpClient = require("./http-client");
const channelConfig = require("./channel-config");
const signing = require("./manifest-signing");
const episodes = require("./episodes");
const { DeliveryError } = require("./errors");

const GAME_ID = "ghosting-renewed";
const MAX_BODY_BYTES = 64 * 1024;
const TIMEOUT_MS = 15000;
const FIELDS = ["archive", "episode", "gameId", "publishedAt", "requires", "schemaVersion", "signature", "version"];
const ARCHIVE_FIELDS = ["fileName", "sha256", "size"];

const invalid = (message) => new DeliveryError("MANIFEST_INVALID", `Манифест эпизода: ${message}`);

// Архив эпизода зовётся ghosting-renewed-<id>-….7z: по имени находятся его загрузки в Downloads.
function episodeOfArchive(fileName) {
  const match = /^ghosting-renewed-(ep\d+)-[A-Za-z0-9._-]+\.7z$/.exec(String(fileName));
  return match && episodes.isEpisode(match[1]) ? match[1] : null;
}
const sameKeys = (value, keys) => Object.keys(value).sort().join() === keys.join();
const isObject = (value) => !!value && typeof value === "object" && !Array.isArray(value);

// null — канал Ghosting не настроен (сборка без профиля выпуска), эпизоды недоступны.
function manifestSource(id, env) {
  const config = channelConfig.resolve(GAME_ID, env);
  if (!config || !config.url) return null;
  return { url: new URL(`../episodes/${id}/stable.json`, config.url).href, publicKeys: config.publicKeys };
}

function validateManifest(data, id, publicKeys) {
  if (!isObject(data)) throw invalid("не JSON-объект");
  try {
    signing.check(data, { required: true, publicKeys });
  } catch (error) {
    throw new DeliveryError("SIGNATURE_INVALID", `Подпись манифеста эпизода не принята: ${error.message}`);
  }
  if (!sameKeys(data, FIELDS)) throw invalid(`поля ${Object.keys(data).join(", ")}`);
  if (data.schemaVersion !== 1) throw invalid(`schemaVersion ${data.schemaVersion}`);
  if (data.gameId !== GAME_ID) throw invalid(`gameId ${data.gameId}`);
  if (data.episode !== id) throw invalid(`описывает ${data.episode}, запрошен ${id}`);
  if (typeof data.version !== "string" || !/^\d+\.\d+\.\d+$/.test(data.version)) throw invalid(`version ${data.version}`);
  if (typeof data.publishedAt !== "string" || !/^\d{4}-\d\d-\d\dT/.test(data.publishedAt) || Number.isNaN(Date.parse(data.publishedAt)))
    throw invalid(`publishedAt ${data.publishedAt}`);
  if (JSON.stringify(data.requires) !== JSON.stringify(episodes.EPISODES[id].requires)) throw invalid(`requires ${JSON.stringify(data.requires)}`);

  const archive = data.archive;
  if (!isObject(archive) || !sameKeys(archive, ARCHIVE_FIELDS)) throw invalid("archive");
  if (episodeOfArchive(archive.fileName) !== id) throw invalid(`archive.fileName ${archive.fileName}`);
  if (!Number.isSafeInteger(archive.size) || archive.size <= 0) throw invalid(`archive.size ${archive.size}`);
  if (typeof archive.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(archive.sha256)) throw invalid("archive.sha256");
  return data;
}

async function fetchManifest(id, env) {
  const source = manifestSource(id, env);
  if (!source) throw new DeliveryError("CHANNEL_UNAVAILABLE", "Канал Ghosting не настроен: эпизоды недоступны", { id });

  const response = await httpClient.request(source.url, { timeout: TIMEOUT_MS });
  if (response.statusCode !== 200) {
    response.resume();
    throw new DeliveryError("CHANNEL_UNAVAILABLE", `Манифест эпизода ${id}: HTTP ${response.statusCode}`, {
      statusCode: response.statusCode,
    });
  }
  const body = await httpClient.readBody(response, MAX_BODY_BYTES);
  let data;
  try {
    data = JSON.parse(body.toString("utf8"));
  } catch (error) {
    throw invalid(`не JSON: ${error.message}`);
  }
  return { manifest: validateManifest(data, id, source.publicKeys), url: source.url };
}

// QA-канал на 127.0.0.1 отдаёт по http — это local-http (feature-flag его разрешает только вне production).
function providerDescriptor(manifest, manifestUrl) {
  const url = new URL(manifest.archive.fileName, manifestUrl).href;
  return new URL(url).protocol === "http:"
    ? { id: "qa-local", type: "local-http", status: "permanent", priority: 10, url }
    : { id: "hlsr-primary", type: "https-direct", status: "permanent", priority: 10, url };
}

// Стоковые cfg эпизода (chapter*.cfg, skill_episodic.cfg) — их syncConfigIn не перезаписывает.
function stockCfgOf(episodeDir) {
  try {
    return fs
      .readdirSync(path.join(episodeDir, "cfg"), { withFileTypes: true })
      .filter((entry) => entry.isFile() && /\.cfg$/i.test(entry.name))
      .map((entry) => entry.name.toLowerCase())
      .sort();
  } catch (error) {
    if (error && error.code === "ENOENT") return [];
    throw error;
  }
}

// Подмена папки эпизода. Прежняя уходит в park (InstallService.episodesParkPath): упадёт лаунчер
// посреди подмены — unparkEpisodes на старте вернёт её туда, где папки нет. Свежая (маркер уже
// внутри) встаёт одним rename, данные игрока переезжают в неё rename из прежней, и только потом
// прежняя удаляется. Ошибка — всё возвращается как было.
function swapIn({ installPath, id, fresh, park, marker, logger }) {
  // Служебные файлы лаунчера в папке эпизода — только его собственные, не из архива.
  fs.rmSync(path.join(fresh, episodes.SYNC), { force: true });
  fs.writeFileSync(path.join(fresh, episodes.MARKER), JSON.stringify(marker, null, 2), "utf8");
  const dir = episodes.EPISODES[id].dir;
  const target = path.join(installPath, dir);
  const parked = path.join(park, dir);
  const had = fs.existsSync(target);
  if (had) {
    // Отложенная раньше копия не вернулась (папка уже стояла) — в сторону, как в parkEpisodes.
    if (fs.existsSync(parked)) fs.renameSync(parked, `${parked}.stale-${Date.now().toString(36)}`);
    fs.mkdirSync(park, { recursive: true });
    fs.renameSync(target, parked);
  }

  let placed = false;
  const carried = [];
  try {
    fs.renameSync(fresh, target);
    placed = true;
    for (const name of had ? fs.readdirSync(parked).filter(episodes.isPlayerEntry) : []) {
      // Данные игрока главнее одноимённого из архива; SAVE и save на Windows — одно имя.
      for (const same of fs.readdirSync(target).filter((other) => other.toLowerCase() === name.toLowerCase()))
        fs.rmSync(path.join(target, same), { recursive: true, force: true });
      fs.renameSync(path.join(parked, name), path.join(target, name));
      carried.push(name);
    }
  } catch (error) {
    try {
      for (const name of carried) fs.renameSync(path.join(target, name), path.join(parked, name));
      if (placed) fs.renameSync(target, fresh);
      if (had) fs.renameSync(parked, target);
    } catch (back) {
      logger.error(`[game-delivery] эпизод ${dir} не вернулся на место, прежняя копия в ${parked}: ${back.message}`);
    }
    throw error;
  }

  if (!had) return;
  try {
    fs.rmSync(parked, { recursive: true, force: true });
  } catch (error) {
    logger.warn(`[game-delivery] прежняя копия эпизода ${dir} не удалена из ${parked}: ${error.message}`);
  }
}

module.exports = {
  GAME_ID,
  episodeOfArchive,
  manifestSource,
  validateManifest,
  fetchManifest,
  providerDescriptor,
  stockCfgOf,
  swapIn,
};
